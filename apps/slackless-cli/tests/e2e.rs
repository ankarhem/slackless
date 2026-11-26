//! End-to-end process integration tests spawning the real compiled `slackless` CLI binary
//! against an isolated temporary SQLite archive (`SLACKLESS_DATA_DIR`).
//!
//! Covers:
//! - Archive CLI JSON/Markdown output contracts and non-zero structured error envelopes.
//! - Live stdio MCP JSON-RPC protocol (`initialize`, `tools/list`, `tools/call`, `resources/list`)
//!   including `--read-only` write-tool enforcement.

#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use slackless_core::store::Store;
use slackless_core::types::{ChannelId, Conversation, Message, Team, TeamId, Ts, User, UserId};

static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

struct TempArchiveDir {
    path: PathBuf,
}

impl TempArchiveDir {
    fn new(tag: &str) -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "slackless-cli-e2e-{tag}-{}-{id}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("data")).unwrap();
        Self { path }
    }

    /// `SLACKLESS_DATA_DIR` of every spawned binary: archives live at `<data>/<TEAM_ID>.sqlite`.
    fn data(&self) -> PathBuf {
        self.path.join("data")
    }

    fn seed_workspace(&self, team_id: &str) {
        let db = self.data().join(format!("{team_id}.sqlite"));
        let mut store = Store::open(&db).unwrap();
        let ws = TeamId::from(team_id);
        store
            .upsert_team(
                &ws,
                &Team {
                    id: ws.clone(),
                    name: "CLI Test Corp".into(),
                    domain: "clitest".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .upsert_users(
                &ws,
                &[
                    User {
                        id: UserId::from("U0ALICE"),
                        name: "alice".into(),
                        real_name: Some("Alice Eng".into()),
                        ..Default::default()
                    },
                    User {
                        id: UserId::from("U0BOB"),
                        name: "bob".into(),
                        real_name: Some("Bob Ops".into()),
                        ..Default::default()
                    },
                ],
            )
            .unwrap();
        store
            .upsert_conversation(
                &ws,
                &Conversation {
                    id: ChannelId::from("C0ENG"),
                    name: Some("engineering".into()),
                    is_channel: true,
                    is_member: true,
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .upsert_message(
                &ws,
                &ChannelId::from("C0ENG"),
                &Message {
                    ts: Ts::from("1700000100.000100"),
                    user: Some(UserId::from("U0ALICE")),
                    text: "Deploying release v2.4 to production".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .upsert_message(
                &ws,
                &ChannelId::from("C0ENG"),
                &Message {
                    ts: Ts::from("1700000150.000100"),
                    user: Some(UserId::from("U0BOB")),
                    text: "Canary metrics look healthy".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .upsert_message(
                &ws,
                &ChannelId::from("C0ENG"),
                &Message {
                    ts: Ts::from("1700000200.000100"),
                    user: Some(UserId::from("U0BOB")),
                    text: "Thread reply confirmed".into(),
                    thread_ts: Some(Ts::from("1700000100.000100")),
                    ..Default::default()
                },
            )
            .unwrap();
    }
}

impl Drop for TempArchiveDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn cli(dir: &TempArchiveDir) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_slackless"));
    // `HOME` locates the stored sessions: no test reads those of the machine running it.
    cmd.env("SLACKLESS_DATA_DIR", dir.data())
        .env("HOME", &dir.path);
    cmd
}

/// Runs `slackless --json <args>` with empty stdin: the exit code and the one JSON value printed.
fn run_json(dir: &TempArchiveDir, args: &[&str]) -> (Option<i32>, Value) {
    let out = cli(dir)
        .arg("--json")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json = serde_json::from_str(stdout.trim()).unwrap_or_else(|err| {
        panic!(
            "{args:?}: stdout is not one JSON value ({err}): {stdout:?}; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.code(), json)
}

/// Arguments the CLI refuses with `invalid_args` (exit 1) before it opens an archive, reads
/// stored sessions, or talks to Slack.
#[test]
fn invalid_arguments_fail_locally_with_invalid_args() {
    let dir = TempArchiveDir::new("invalid-args");
    let clip = dir.path.join("clip.wav");
    std::fs::write(&clip, b"RIFF").unwrap();
    let clip = clip.to_str().unwrap();
    for args in [
        // `--workspace` names `<data>/<TEAM_ID>.sqlite`: `../escape` would create, then
        // vacuum, a database outside the data dir.
        vec![
            "--workspace",
            "../escape",
            "transcribe",
            "F0X",
            "--file",
            clip,
        ],
        vec!["--workspace", "../escape", "vacuum"],
        vec!["--workspace", "", "stats"],
    ] {
        let (code, json) = run_json(&dir, &args);
        assert_eq!(
            (code, &json["error"]),
            (Some(1), &json!("invalid_args")),
            "{args:?}: {json}"
        );
        assert!(json["message"].is_string(), "{args:?}: {json}");
    }
    assert!(
        !dir.path.join("escape.sqlite").exists(),
        "a path-like --workspace created a database outside the data dir"
    );
}

#[test]
fn cli_archive_commands_json_contract_export_and_error_envelopes() {
    let dir = TempArchiveDir::new("archive");
    dir.seed_workspace("T0CLI");

    // 1. `channels --json` lists seeded channel with stable fields.
    let out = cli(&dir)
        .args(["--json", "--workspace", "T0CLI", "channels"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "channels failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let channels: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(channels["count"], 1);
    let arr = channels["channels"]
        .as_array()
        .expect("channels JSON array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], "C0ENG");
    assert_eq!(arr[0]["name"], "engineering");

    // 2. `read C0ENG --json` returns messages in chronological order with resolved user names.
    let out = cli(&dir)
        .args(["--json", "--workspace", "T0CLI", "read", "C0ENG"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let read_env: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(read_env["count"], 2);
    let arr = read_env["messages"]
        .as_array()
        .expect("read messages array");
    assert_eq!(arr.len(), 2);
    assert!(arr.iter().any(|m| {
        m["text"]
            .as_str()
            .is_some_and(|t| t.contains("Deploying release v2.4"))
    }));

    // 3. `search Deploying --json` hits FTS5 index over the seeded archive.
    let out = cli(&dir)
        .args(["--json", "--workspace", "T0CLI", "search", "Deploying"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let hits: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(hits["count"], 1);
    assert_eq!(hits["messages"][0]["ts"], "1700000100.000100");

    // 4. `stats --json` reports accurate counts.
    let out = cli(&dir)
        .args(["--json", "--workspace", "T0CLI", "stats"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stats: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(stats["messages"], 3);
    assert_eq!(stats["conversations"], 1);
    assert_eq!(stats["users"], 2);

    // 5. `export C0ENG --format md` produces markdown containing both authors & bodies.
    let out = cli(&dir)
        .args(["--workspace", "T0CLI", "export", "C0ENG", "--format", "md"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let md = String::from_utf8(out.stdout).unwrap();
    assert!(md.contains("Deploying release v2.4"));
    assert!(md.contains("Canary metrics look healthy"));

    // 6. Invalid `--since` under `--json` exits non-zero with structured `{"ok": false, ...}`.
    let out = cli(&dir)
        .args([
            "--json",
            "--workspace",
            "T0CLI",
            "read",
            "C0ENG",
            "--since",
            "not-a-date",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(err["ok"], false);
    assert!(err["error"].is_string());
    assert!(err["message"].is_string());
}

fn mcp_rpc(stdin: &mut impl Write, stdout: &mut impl BufRead, req: &Value) -> Value {
    writeln!(stdin, "{req}").unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(!line.is_empty(), "MCP server closed stdout unexpectedly");
    serde_json::from_str(&line)
        .unwrap_or_else(|e| panic!("invalid JSON-RPC response {line:?}: {e}"))
}

#[test]
fn mcp_stdio_server_handshake_tools_resources_and_read_only_enforcement() {
    let dir = TempArchiveDir::new("mcp");
    dir.seed_workspace("T0MCP");

    let mut child = cli(&dir)
        .args(["mcp", "--read-only"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    // 1. JSON-RPC `initialize`.
    let init = mcp_rpc(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "e2e-test", "version": "1.0.0" }
            }
        }),
    );
    assert_eq!(init["id"], 1);
    assert!(
        init["result"]["serverInfo"]["name"].is_string(),
        "unexpected initialize result: {init}"
    );

    // 2. Send `notifications/initialized` (no response expected).
    writeln!(
        stdin,
        "{}",
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
    )
    .unwrap();
    stdin.flush().unwrap();

    // 3. `tools/list`: read tools present, mutating tools MUST be absent in `--read-only` mode.
    let list = mcp_rpc(
        &mut stdin,
        &mut stdout,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }),
    );
    let tools = list["result"]["tools"]
        .as_array()
        .expect("tools array in tools/list");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"list_workspaces"), "names={names:?}");
    assert!(names.contains(&"list_channels"), "names={names:?}");
    assert!(names.contains(&"search_messages"), "names={names:?}");
    assert!(names.contains(&"read_channel"), "names={names:?}");
    assert!(
        !names.contains(&"send_message")
            && !names.contains(&"reply_thread")
            && !names.contains(&"add_reaction"),
        "--read-only must hide write tools: {names:?}"
    );

    // 4. `tools/call` -> `list_workspaces` returns our isolated `T0MCP`.
    let ws_call = mcp_rpc(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "list_workspaces", "arguments": {} }
        }),
    );
    let ws_text = ws_call["result"]["content"][0]["text"]
        .as_str()
        .expect("list_workspaces text payload");
    assert!(ws_text.contains("T0MCP"), "got: {ws_text}");

    // 5. `tools/call` -> `search_messages` finds our seeded message over stdio MCP.
    let search_call = mcp_rpc(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "search_messages",
                "arguments": { "workspace": "T0MCP", "query": "Canary" }
            }
        }),
    );
    let search_text = search_call["result"]["content"][0]["text"]
        .as_str()
        .expect("search_messages text payload");
    assert!(
        search_text.contains("Canary metrics look healthy"),
        "got: {search_text}"
    );

    // 6. Calling a mutating tool directly under `--read-only` MUST be refused.
    let write_call = mcp_rpc(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "send_message",
                "arguments": { "workspace": "T0MCP", "channel": "C0ENG", "text": "blocked" }
            }
        }),
    );
    let refused = write_call.get("error").is_some()
        || write_call["result"]["isError"].as_bool() == Some(true);
    assert!(
        refused,
        "--read-only must refuse direct send_message call: {write_call}"
    );

    // 7. Additional read-only MCP tools: `list_channels`, `read_channel`, `read_thread`,
    // `export_conversation`, `get_catchup_summary`, `list_transcripts`.
    let ch_call = mcp_rpc(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": { "name": "list_channels", "arguments": { "workspace": "T0MCP" } }
        }),
    );
    assert!(
        ch_call["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("C0ENG")
    );

    let read_call = mcp_rpc(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {
                "name": "read_channel",
                "arguments": { "workspace": "T0MCP", "channel": "C0ENG" }
            }
        }),
    );
    assert!(
        read_call["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("Deploying release v2.4")
    );

    let thread_call = mcp_rpc(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "tools/call",
            "params": {
                "name": "read_thread",
                "arguments": {
                    "workspace": "T0MCP",
                    "channel": "C0ENG",
                    "thread_ts": "1700000100.000100"
                }
            }
        }),
    );
    assert!(
        thread_call["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("Thread reply confirmed")
    );

    let exp_call = mcp_rpc(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/call",
            "params": {
                "name": "export_conversation",
                "arguments": { "workspace": "T0MCP", "channel": "C0ENG", "format": "markdown" }
            }
        }),
    );
    assert!(
        exp_call["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("Deploying release v2.4")
    );

    let catchup_call = mcp_rpc(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "tools/call",
            "params": { "name": "get_catchup_summary", "arguments": { "workspace": "T0MCP" } }
        }),
    );
    assert!(catchup_call["result"]["content"][0]["text"].is_string());

    // 8. `resources/list` responds cleanly.
    let res_list = mcp_rpc(
        &mut stdin,
        &mut stdout,
        &json!({ "jsonrpc": "2.0", "id": 11, "method": "resources/list", "params": {} }),
    );
    assert!(
        res_list["result"]["resources"].is_array(),
        "resources/list should return array: {res_list}"
    );

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(
        status.success(),
        "MCP server did not exit cleanly on EOF: {status}"
    );
}
