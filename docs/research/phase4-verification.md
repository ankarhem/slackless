# Phase 4 (Core UI) verification

Verified by the lead on 2026-11-19 with commands and a screenshot captured this session.
(The DAG's threads node stuck while trying to add reaction-chips/actions to the thread panel and
was cancelled; `threads.rs` already held a complete thread panel from the skeleton, and the lead
ran the verification directly. 5/7 nodes completed; crates/ Foundation untouched.)

## Gate (aarch64-apple-darwin)

| Command                                                 | Result                                       |
| ------------------------------------------------------- | -------------------------------------------- |
| `cargo build --workspace`                               | **exit 0**                                   |
| `cargo clippy --workspace --all-targets -- -D warnings` | **exit 0**                                   |
| `cargo test --workspace`                                | **exit 0**                                   |
| launch `target/debug/slackless-gui` (fixture mode) ~8s  | **no panic / empty stderr**                  |
| `screencapture`                                         | **exit 0** -> `screenshots/slackless-p4.png` |

Satisfies criteria (1) and (2) for the full workspace incl. the GUI; criterion (3) partially
(sidebar, channel history, composer rendered; see gaps).

## What the screenshot shows (screenshots/slackless-p4.png)

A running slackless window with: workspace rail (avatars + add); sidebar with Home/DMs(4)/
Activity(1)/Later nav, a Channels section (`# general` selected, `# random`/`# engineering`/
`# design` with unread badges) and Direct messages with avatars; a message pane with the
`# general` header + topic, **date dividers** (Nov 18 / Nov 19), author-grouped messages with
avatars + timestamps + an "(edited)" tag, **bold inline text**, and **reaction chips**
(coffee 3, tada 1, clap 4); and a composer with a B/I/S/code/link toolbar + placeholder + send.

## Gaps -> feature-parity phase

- **Theme:** rendered in LIGHT theme; the reference (`screenshots/slack-reference-dm.png`) is
  DARK. Need a Slack-Dark theme (and the full theme system) wired as the default to match.
- **Missing chrome:** global search bar and the conversation-header (star / huddle / search /
  details) are not yet in the window; thread panel exists in code but no thread is open in this
  shot; reaction-add + hover message-actions menu in the thread panel are not finished.
- These are the targets for Phase 5 visual-polish + parity work.

## Files

Core UI lives in apps/slackless-gui/src/: app.rs (root 3-pane), sidebar.rs, messages.rs,
composer.rs, threads.rs, fixtures.rs, bridge.rs, main.rs (~2.8k lines).
