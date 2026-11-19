# Phase 5 verification

Date: 2026-11-19. Environment: macOS arm64 (Apple M4 Pro), Nix shell, `CARGO_TARGET_DIR=target`, `CARGO_PROFILE_DEV_DEBUG=0`.
No source files were edited during this verification; everything was already green.

## Results

| Check         | Command                                                                | Result                                                                                                                                                                               |
| ------------- | ---------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Build         | `cargo build --workspace`                                              | exit 0 (`Finished dev profile`, only slackless-cli recompiled)                                                                                                                       |
| Tests         | `cargo test --workspace`                                               | exit 0: 92 passed, 0 failed, 2 ignored across 17 test suites (incl. doc-tests)                                                                                                       |
| Lint          | `cargo clippy --workspace --all-targets -- -D warnings`                | exit 0, no warnings                                                                                                                                                                  |
| GUI launch    | `target/debug/slackless-gui`, ran ~8 s then killed                     | process alive at 8 s; log empty; 0 `panic` lines                                                                                                                                     |
| Screenshot    | `screencapture -x`                                                     | `docs/research/screenshots/slackless-p5-dark.png` (full screen, 3840x2160) and `docs/research/screenshots/slackless-p5-dark-window.png` (cropped to the slackless window, 2400x1651) |
| CLI help      | `target/debug/slackless --help`                                        | exit 0 (output below)                                                                                                                                                                |
| CLI search    | `target/debug/slackless search hello --json`                           | exit 0: `{"query":"hello","count":0,"messages":[]}`                                                                                                                                  |
| Frozen crates | `find crates -name '*.rs' -newer docs/research/phase3-verification.md` | empty                                                                                                                                                                                |

## Screenshot vs reference (`slack-reference-dm.png`)

The full-screen capture also shows the real Slack app behind the slackless window. Use the cropped `-window` file for comparison.

What matches:

- Layout: a narrow workspace rail (AC/SP avatars, then Home / DMs with a badge / Activity / Files / To-dos / Admin icons, plus a `+` button), a sidebar, and the main pane.
- The top bar has back/forward/history buttons and a centered "Describe what you are looking for" search field.
- The sidebar has the workspace name with a chevron, settings and compose icons, and a "Find a conversation" field. It also has collapsible Channels and Direct messages sections with `#`/lock glyphs, the active DM highlighted in blue, and an unread badge on `bob`.
- The conversation header has a star, an avatar and the name, and huddle/bell/search/more icons. Below it are the Messages / Add canvas / Files & links / `+` tabs, with the Messages tab underlined.
- The message list has a "Today" day pill, author name + timestamp + body rows with square avatars, and a red "New" unread divider.
- The composer has a formatting toolbar (bold, italic, strike, lists, quote, code, link), a "Message" placeholder, and a bottom action row (+, Aa, emoji, @, video, mic, slash) with a send button.
- The dark palette is close to Slack's.

What still differs:

- Avatars are colored letter tiles, not photos; the composer placeholder is "Message" instead of "Message <name>".
- The sidebar lacks Slack's top nav items (Unreads, Threads, Huddles, Drafts & sent, Saved, Directories), custom sections (e.g. STATUS), the "More unreads" pill, and the user avatar/presence and theme toggle at the bottom of the rail.
- The workspace name is truncated ("Acme C..."), which suggests the sidebar is too narrow at this window size.
- The fixture DM shows no reactions, tenure badges ("6 years"), links or emoji, so those renderers cannot be judged from this screenshot.
- The text is visibly smaller than in Slack, and the composer toolbar has fewer controls (no underline, paragraph style, or the extra block buttons).
- The window has no native traffic-light colors (they are grey in the capture, probably because the window was not focused).

## `slackless --help`

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

`slackless search hello --json` (no local archive, no credentials):

```
{"query":"hello","count":0,"messages":[]}
```

## Frozen crates

`find crates -name '*.rs' -newer docs/research/phase3-verification.md` printed nothing, so no file under `crates/` has changed since Phase 3 was verified. This verification made no edits anywhere in the source tree.
