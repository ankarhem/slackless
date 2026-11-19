# Final verification

Run on 2026-11-19 against the working tree after phases 1-5 (A+B). Everything ran in fixture mode, with no network access, credentials or live workspace. No source files were changed during this pass: every gate was green on the first run, so nothing under `apps/**` needed fixing.

Build environment: `CARGO_TARGET_DIR=<repo>/target CARGO_PROFILE_DEV_DEBUG=0`.

## Gate results

| Gate          | Command                                                                | Exit | Result                                                                                        |
| ------------- | ---------------------------------------------------------------------- | ---- | --------------------------------------------------------------------------------------------- |
| Build         | `cargo build --workspace`                                              | 0    | `Finished dev profile [unoptimized] target(s)`                                                |
| Test          | `cargo test --workspace`                                               | 0    | 92 passed, 0 failed, 2 ignored across 17 test result blocks (unit, integration and doc-tests) |
| Clippy        | `cargo clippy --workspace --all-targets -- -D warnings`                | 0    | `Checking slackless-gui v0.1.0 ... Finished`, no warnings                                     |
| Frozen crates | `find crates -name '*.rs' -newer docs/research/phase3-verification.md` | 0    | Empty output: no crate source changed after phase 3                                           |
| GUI launch    | `caffeinate -u -t 2; target/debug/slackless-gui` (8 s), then kill      | 0    | Process ran (pid seen with `pgrep`), window rendered, `pkill` exit 0                          |
| CLI           | `target/debug/slackless --help`                                        | 0    | See below                                                                                     |

### CLI help output

```
Headless Slack client CLI over the local archive

Usage: slackless [OPTIONS] <COMMAND>

Commands:
  search    Full-text search the local archive (modifiers: in: from: has: before: after:)
  read      Print recent messages from a conversation (channel id, #name or @handle)
  send      Post a message to a conversation (needs a stored session)
  reply     Reply in a thread (needs a stored session)
  react     Add an emoji reaction to a message (needs a stored session)
  channels  List channels and private groups known to the local archive
  dms       List direct messages and group DMs known to the local archive
  auth      Account and session management
  help      Print this message or the help of the given subcommand(s)

Options:
      --json                 Emit machine-readable JSON
      --workspace <TEAM_ID>  Workspace (team id) to act on; defaults to the first stored session, else the only local archive
  -h, --help                 Print help
```

## Screenshots

| File                                                               | What it shows                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| ------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `docs/research/screenshots/slackless-final-window.png` (2400x1656) | **The evidence capture.** It is the slackless-gui window, captured by window id with `screencapture -o -l <id>`, and it is not black. It shows the dark theme, the workspace rail (Home, DMs badge 1, Activity, Files, To-dos, Admin), the sidebar (Unreads 1, Channels with a huddle indicator on #general, Direct messages with presence, and an unread badge on bob), and the DM with alice. That DM has the Messages/Canvas/List/Files & links tabs, a "Today" divider, a red "New" unread line, a PDF file card, an image attachment, and the formatting composer. |
| `docs/research/screenshots/slackless-final.png` (3840x2160)        | The full-screen `screencapture` that the task asked for. **It is not black, but it shows only the macOS lock screen.** The session was locked during the run, and `caffeinate -u` wakes the display but does not unlock it. Keep it as a record of the attempt; it is not evidence of the UI.                                                                                                                                                                                                                                                                           |

The window shot has one visual nit: the `dashboard-mockup.png` attachment renders as an empty dark frame. No fixture image bytes are loaded, so the image never decodes. It is cosmetic and covered by the "partial" file/media rows; this pass did not change it.

Earlier per-phase captures are still in the same folder: `slackless-p4.png`, `slackless-p5-dark*.png`, `slackless-p5b*.png` and `slackless-p5c.png`.

## Parity matrix check (`docs/feature-parity.md`)

I parsed every `| F-...` row programmatically.

- 1556 feature rows use the 8-column matrix. Every one has a non-empty **Status** and a non-empty **Evidence** cell, and every row that is not `done` carries a reason in Evidence (for example, "Deferred (live capture): ..." with a link to the gap table).
- The other 73 `| F-...` lines belong to the 4-column "API coverage gaps" table, which lists what must be discovered per row group. They are not matrix rows.
- The per-status counts from the rows match the doc's "Status summary" table exactly:

| Status    | Rows     |
| --------- | -------- |
| done      | 47       |
| partial   | 38       |
| planned   | 620      |
| deferred  | 851      |
| **total** | **1556** |

## Deferred, and why

851 rows are `deferred`. They cannot be built from fixtures or from documented APIs, for three reasons:

1. **Live traffic capture needed (805 rows).** The private web-client call is unobserved, known only from LEADS, or failed verification. Each needs a DevTools/HAR capture against app.slack.com (or the desktop app behind a proxy) before implementation, and that is out of scope here: no network access and no credentials. The main areas:
   - Workflow Builder authoring
   - Slack AI and Slackbot (summaries, recaps, AI search answers, translation)
   - Agentforce, agent sessions and code channels
   - Slack Connect management and invitations
   - Lists field and view editing, CSV import and automations
   - clip transcripts and captions (`transcription.status`, the "generate transcript" call)
   - huddle pre-flight, preferences, reactions and captions
   - calendar and Drive account connection

   The full list, with the rows each covers, is in `feature-parity.md#api-coverage-gaps`.

2. **WebRTC / Chime media stack (34 rows).** Native huddle audio, video and screen share are huddle stages 2-4 in `api-huddles.md`. They need the Chime protobuf signaling socket, DTLS-SRTP with relay-only ICE, and TURN credential refresh. Until those exist, only the join-in-browser fallback is possible.
3. **Quip collab protocol (12 rows).** Live canvas and list co-editing, comment anchors, text reactions and full-fidelity layout exist only in the undocumented Quip edit-document protocol (`api-canvases-lists.md`). Canvases can be read-rendered from fixtures, but not edited live.

The 620 `planned` rows are not blocked. Their APIs are known well enough to build; they just have not been started. The 38 `partial` rows say in their Evidence cell what is missing.

## Verdict

All gates are green: build 0, test 0 (92 passed), clippy 0 with `-D warnings`, and the frozen crates are untouched. The GUI launches and renders the dark Slack-style shell from fixtures, and the CLI runs. The parity matrix is complete and consistent with its summary. The remaining work is the deferred categories above, plus the planned and partial backlog.
