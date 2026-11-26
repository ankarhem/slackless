//! End-to-end process integration tests spawning the real compiled `slackless` CLI binary
//! against an isolated temporary SQLite archive (`SLACKLESS_DATA_DIR`).
//!
//! Covers:
//! - Archive CLI JSON/Markdown output contracts and non-zero structured error envelopes.
//! - Live stdio MCP JSON-RPC protocol (`initialize`, `tools/list`, `tools/call`, `resources/list`)
//!   including `--read-only` write-tool enforcement.
//! - Live commands against a local wiremock Slack API, signed in with fake credentials.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use slackless_core::auth::{Session, SessionStore};
use slackless_core::store::Store;
use slackless_core::types::{ChannelId, Conversation, Message, Team, TeamId, Ts, User, UserId};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

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
    // `HOME` (and on Linux `XDG_CONFIG_HOME`) locates the stored sessions: no test reads those
    // of the machine running it.
    cmd.env("SLACKLESS_DATA_DIR", dir.data())
        .env("HOME", &dir.path)
        .env_remove("XDG_CONFIG_HOME");
    cmd
}

/// Signs the isolated `HOME` in to `team_id` with fake credentials whose API base is `server`,
/// so live commands run their real code paths without reaching Slack.
fn sign_in(dir: &TempArchiveDir, team_id: &str, server: &MockServer) {
    let config = if cfg!(target_os = "macos") {
        dir.path.join("Library/Application Support")
    } else {
        dir.path.join(".config")
    };
    SessionStore::with_path(config.join("slackless/sessions.json"))
        .save(&[Session::new(
            server.uri(),
            team_id,
            "xoxc-e2e",
            "xoxd-e2e",
            "U0E2E",
        )])
        .unwrap();
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
        // A blank message body (`-` reads the empty stdin) would make Slack answer `no_text`
        // after a round trip.
        vec!["send", "C0X", "-"],
        vec!["reply", "C0X", "1700000000.000100", " "],
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

/// With nothing to act on, reads succeed with empty results while archive actions fail with the
/// error envelope: `ok: false` always comes with exit 1, `error`, and `message`.
#[test]
fn soft_empty_reads_succeed_and_archive_actions_fail_with_the_error_envelope() {
    let dir = TempArchiveDir::new("soft-empty");
    let backup = dir.path.join("backup.sqlite");
    let backup = backup.to_str().unwrap();
    // No stored session and no archive: no workspace is known. `T0NONE` has no archive.
    for (args, error) in [
        (vec!["read", "C0X"], None),
        (vec!["stats"], None),
        (vec!["archive", "status"], None),
        (vec!["vacuum"], Some("no_workspace")),
        (vec!["archive", "check"], Some("no_workspace")),
        (vec!["archive", "backup", backup], Some("no_workspace")),
        (vec!["summarize", "C0X"], Some("no_workspace")),
        (vec!["summarize", "--catchup"], Some("no_workspace")),
        (vec!["--workspace", "T0NONE", "vacuum"], Some("no_archive")),
        (
            vec!["--workspace", "T0NONE", "archive", "check"],
            Some("no_archive"),
        ),
        (
            vec!["--workspace", "T0NONE", "archive", "backup", backup],
            Some("no_archive"),
        ),
        (
            vec!["--workspace", "T0NONE", "summarize", "C0X"],
            Some("no_archive"),
        ),
    ] {
        let (code, json) = run_json(&dir, &args);
        match error {
            None => assert!(
                code == Some(0) && json["ok"] != false,
                "{args:?}: exit {code:?}: {json}"
            ),
            Some(error) => {
                assert_eq!(
                    (code, &json["ok"], &json["error"]),
                    (Some(1), &json!(false), &json!(error)),
                    "{args:?}: {json}"
                );
                assert!(json["message"].is_string(), "{args:?}: {json}");
            }
        }
    }
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

/// Spawns `slackless mcp <args>` (stderr captured) and completes the `initialize` handshake.
fn start_mcp(dir: &TempArchiveDir, args: &[&str]) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut child = cli(dir)
        .arg("mcp")
        .args(args)
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
    (child, stdin, stdout)
}

#[test]
fn mcp_stdio_server_handshake_tools_resources_and_read_only_enforcement() {
    let dir = TempArchiveDir::new("mcp");
    dir.seed_workspace("T0MCP");
    let (mut child, mut stdin, mut stdout) = start_mcp(&dir, &["--read-only"]);

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
/// Live reads and sends mirror what Slack returned into the local archive. When that write
/// fails, the command still answers, and the failure is reported on stderr (never on stdout,
/// which carries the JSON or the MCP protocol).
#[tokio::test]
async fn live_commands_report_local_mirror_failures_on_stderr() {
    let dir = TempArchiveDir::new("mirror");
    let server = MockServer::start().await;
    let root = json!({"type": "message", "user": "U0E2E", "text": "live root",
        "ts": "1700000100.000100", "thread_ts": "1700000100.000100"});
    for (method, body) in [
        (
            "conversations.history",
            json!({"ok": true, "messages": [root], "has_more": false}),
        ),
        (
            "conversations.replies",
            json!({"ok": true, "messages": [root], "has_more": false}),
        ),
        (
            "chat.postMessage",
            json!({"ok": true, "channel": "C0LIVE", "ts": "1700000200.000100",
                "message": {"type": "message", "user": "U0E2E", "text": "sent", "ts": "1700000200.000100"}}),
        ),
    ] {
        Mock::given(path(format!("/api/{method}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }
    sign_in(&dir, "T0LIVE", &server);
    // A directory where the archive file belongs: opening it for writing fails.
    std::fs::create_dir_all(dir.data().join("T0LIVE.sqlite")).unwrap();

    let out = cli(&dir)
        .args(["--json", "thread", "C0LIVE", "1700000100.000100", "--live"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "thread --live failed: {stderr}");
    let thread: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(thread["count"], 1);
    assert!(
        stderr.contains("could not mirror"),
        "thread --live: {stderr:?}"
    );

    let (child, mut stdin, mut stdout) = start_mcp(&dir, &[]);
    for (id, name, arguments) in [
        (
            2,
            "read_channel",
            json!({"channel": "C0LIVE", "live": true}),
        ),
        (
            3,
            "read_thread",
            json!({"channel": "C0LIVE", "thread_ts": "1700000100.000100", "live": true}),
        ),
        (
            4,
            "send_message",
            json!({"channel": "C0LIVE", "text": "sent"}),
        ),
    ] {
        let call = mcp_rpc(
            &mut stdin,
            &mut stdout,
            &json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": {"name": name, "arguments": arguments}}),
        );
        assert!(
            call["result"]["isError"] != true && call["result"]["content"][0]["text"].is_string(),
            "{name}: {call}"
        );
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr.matches("could not mirror").count(),
        2,
        "MCP live reads: {stderr:?}"
    );
    assert!(
        stderr.contains("could not record the echo"),
        "MCP send_message: {stderr:?}"
    );
}

/// `backfill` runs the core history walk: a channel whose history is already inside a stored
/// synced span costs one request to refresh, instead of walking every page again.
#[tokio::test]
async fn backfill_skips_pages_inside_a_synced_span() {
    let dir = TempArchiveDir::new("backfill");
    dir.seed_workspace("T0BF");
    {
        let mut store = Store::open(dir.data().join("T0BF.sqlite")).unwrap();
        slackless_core::write_synced_spans(
            &mut store,
            &TeamId::from("T0BF"),
            &HashMap::from([(
                ChannelId::from("C0ENG"),
                slackless_core::Synced {
                    newest: Ts::from("1700000200.000100"),
                    oldest: Ts::from("1700000100.000100"),
                    complete: true,
                },
            )]),
        )
        .unwrap();
    }
    let server = MockServer::start().await;
    // Slack keeps offering an older page; the walk must stop at the first one, which overlaps.
    Mock::given(path("/api/conversations.history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "has_more": true,
            "response_metadata": {"next_cursor": "older"},
            "messages": [
                {"type": "message", "user": "U0BOB", "text": "newest", "ts": "1700000200.000100"},
                {"type": "message", "user": "U0ALICE", "text": "known", "ts": "1700000150.000100"}
            ]
        })))
        .mount(&server)
        .await;
    sign_in(&dir, "T0BF", &server);

    let (code, json) = run_json(&dir, &["backfill", "C0ENG", "--pages", "0"]);
    assert_eq!(code, Some(0), "{json}");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        1,
        "a channel inside a complete synced span is refreshed with one page, got {} requests",
        requests.len()
    );
    assert_eq!(json["channels"][0]["pages"], 1, "{json}");
    assert_eq!(json["channels"][0]["reached_end"], true, "{json}");
}
