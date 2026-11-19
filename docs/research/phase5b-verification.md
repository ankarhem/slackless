# Phase 5b (wave B) verification

Date: 2026-11-19. Host: macOS arm64 (M4 Pro). All cargo commands ran from the repo root with
`CARGO_TARGET_DIR=/Users/ankarhem/repos/ankarhem/slackless/target CARGO_PROFILE_DEV_DEBUG=0`.
No edits were needed in `apps/**` because everything was green on the first run.

## Results

| Step             | Command                                                                | Exit | Notes                                                               |
| ---------------- | ---------------------------------------------------------------------- | ---- | ------------------------------------------------------------------- |
| Build            | `cargo build --workspace`                                              | 0    | `Finished dev profile ... in 3.41s` (only slackless-gui recompiled) |
| Test             | `cargo test --workspace`                                               | 0    | Every suite reported `test result: ok`, with 0 failures             |
| Lint             | `cargo clippy --workspace --all-targets -- -D warnings`                | 0    | `Finished dev profile ... in 0.96s`, no warnings                    |
| Launch + capture | `./target/debug/slackless-gui`, wait 8s, `screencapture -x`, kill      | 0    | Rendered and captured (not black). Process killed with SIGTERM.     |
| Crates frozen    | `find crates -name '*.rs' -newer docs/research/phase3-verification.md` | 0    | **Empty output**: no file under `crates/` changed                   |

## Screenshots

- Default launch: `docs/research/screenshots/slackless-p5b.png` (3840x2160 full screen; the
  slackless window sits over the real Slack app). It shows the DM with `alice`.
- Channel browser (`SLACKLESS_GUI_DEMO=browser`): `docs/research/screenshots/slackless-p5b-browser.png`
- Search (`SLACKLESS_GUI_DEMO=search:deploy`): `docs/research/screenshots/slackless-p5b-search.png`

The first browser and search captures came out fully black because the display was asleep.
I woke it with `caffeinate -u` and captured again. The files listed above are the good
second captures.

## Wave-B surfaces

| Surface         | Status  | Reason                                                                                                                                                                                                                                                                   |
| --------------- | ------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| DMs / presence  | RENDERS | The `alice` DM is open, with a green presence dot on the header and sidebar avatars. 1:1 DMs (`alice`, `bob`) and the group DM `cara, dan` (stacked avatars) are listed under Direct messages. The rail's DMs item has a badge.                                          |
| Channel browser | RENDERS | The "Browse channels" dialog shows a search field, "8 channels", and rows with name, member count and purpose. Rows show either a lock (private) or `#`, and each has Join or a "Joined" check. Only the demo hook opened it; clicking "Add channels" was not exercised. |
| Files / media   | RENDERS | `Q4-plan.pdf` shows as a file card (PDF, 2.3 MB, download icon). The `dashboard-mockup.png` image thumbnail appears inline with a collapsible caption.                                                                                                                   |
| Search          | RENDERS | The page reads `Results for "deploy"` with 2 results. Each has a channel/date header, rich text (bold, inline code, link), a code block and reactions, plus a close button. It stands in for the message column.                                                         |
| Unreads         | RENDERS | The sidebar has an Unreads shortcut with count 1, unread badges on `bob` and on the DMs/Activity rail items, and a red "New" divider in the conversation. A dedicated Unreads page from clicking that row was not exercised, because only the launch state was captured. |

## Crates untouched

`find /Users/ankarhem/repos/ankarhem/slackless/crates -name '*.rs' -newer /Users/ankarhem/repos/ankarhem/slackless/docs/research/phase3-verification.md`
returned nothing. Wave B did not modify any frozen crate.
