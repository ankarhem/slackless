# Phase 6 wave G verification

Wave G covers Later (saved messages and their reminders), pinned messages and the bookmarks bar, the Activity feed, and the OS layer behind them: the macOS dock badge and system notifications. I checked it on 2026-11-19 in fixture mode, with no network calls and no credentials. Every command ran with `CARGO_TARGET_DIR=/Users/ankarhem/repos/ankarhem/slackless/target CARGO_PROFILE_DEV_DEBUG=0`.

## Gates

| Gate           | Command                                                                                                   | Exit | Result                                                                                                                                  |
| -------------- | --------------------------------------------------------------------------------------------------------- | ---- | --------------------------------------------------------------------------------------------------------------------------------------- |
| Build          | `cargo build --workspace`                                                                                 | 0    | `Finished dev profile`                                                                                                                  |
| Test           | `cargo test --workspace`                                                                                  | 0    | 92 passed, 0 failed, 2 ignored. Every `test result:` line is `ok`                                                                       |
| Clippy         | `cargo clippy --workspace --all-targets -- -D warnings`                                                   | 0    | No warnings (`Checking slackless-gui ... Finished`)                                                                                     |
| Launch         | `SLACKLESS_GUI_DEMO=<later\|pins\|activity\|notify> ./target/debug/slackless-gui`, 6 s each, then SIGTERM | 0    | All four processes were still running after 6 s and stopped with SIGTERM. Nothing was logged to stdout or stderr, so there was no panic |
| Changed crates | `find crates -name '*.rs' -newer docs/research/phase3-verification.md`                                    | 0    | Only `crates/slackless-platform/src/macos.rs` and `crates/slackless-platform/src/lib.rs`                                                |

The crate check passes: `slackless-platform` is the only crate that changed under `crates/`, and the other crates are untouched. The same `find` over `apps/` lists the GUI modules and the CLI files that earlier phases changed, which is expected because `apps/**` is open for edits.

I needed no fixes in `apps/**` or `slackless-platform`, so this verification changed only docs. The test count is the same as wave E (92), so wave G added no unit tests. In particular, `later.rs` `Remind::at` (the due-time arithmetic) and the `activity.rs` fixture derivation have no tests.

## Visual check

No screenshots were taken. The launches show only that each demo path starts and stays up without a panic. As in waves D and E, the rendering rests on reading the code, not on a screen capture. To check it by eye on an unlocked session, run:

- `SLACKLESS_GUI_DEMO=later`: Later is open in place of the message column.
- `SLACKLESS_GUI_DEMO=pins`: the pins panel is open in the right column.
- `SLACKLESS_GUI_DEMO=activity`: the rail's Activity destination is selected.
- `SLACKLESS_GUI_DEMO=notify`: posts a test system notification.

The `notify` demo needs a note. In an unbundled `cargo run` build, the notification goes through `osascript`. A launch shows that the call did not crash, but not that a banner appeared.

## What wave G contains

| Module                                               | Surface                                                                                                                                                                                                                                                                                                                                                                                                                                    | Talks to Slack?                                                                           |
| ---------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------- |
| `apps/slackless-gui/src/later.rs` (548 lines)        | `LaterView` replaces the message column while open. It has In progress, Archived and Completed tabs. Each item can be completed, archived, moved back to in progress, removed, or given or cleared a reminder. The reminder presets (`Remind::ALL`) are 20 minutes, 1 hour, 3 hours, tomorrow at 9:00 and next week. Each workspace is seeded once from its fixtures with one item due today, one overdue, one completed and one archived. | No. Items are kept in memory, and `saved.*` is not called.                                |
| `apps/slackless-gui/src/actions.rs`                  | "Save for later" and the "Remind me about this" submenu on transcript and thread messages emit a `SaveRequest`. `app.rs` `save_for_later` passes it to `LaterView::save`.                                                                                                                                                                                                                                                                  | No.                                                                                       |
| `apps/slackless-gui/src/app.rs`                      | `fire_reminders` runs every 10 s (`REMINDER_POLL`) and calls `LaterView::take_due`. `post_notification` uses GPUI's `SystemNotification` in bundled builds and the platform `Notifier` in unbundled builds, and also shows an in-app toast. `sync_badge` passes the sidebar's badge count to `BadgeSetter`, only when it changes, and clears the badge at zero. `OpenActivity` is bound to Cmd+Shift+A.                                    | No.                                                                                       |
| `apps/slackless-gui/src/pins.rs` (230 lines)         | `bookmarks_bar` sits under the conversation tabs: an "N Pinned" chip that toggles the pins panel, then the conversation's bookmarks, each opening its URL. `pins_panel` lists the pinned messages with the shared `message_row` in the right column, exclusive with the thread and profile panels. Fixture pins are messages with files or at least two reactions, at most four.                                                           | No. `pins.list` and `bookmarks.list` are not called, and there is no pin or unpin action. |
| `apps/slackless-gui/src/activity.rs` (403 lines)     | `ActivityFeed` in the sidebar's list column lists @-mentions, reactions to your messages and replies in your threads. It has All, Mentions, Reactions and Threads filters with unread counts, per-item read state, and "Mark all as read". An item opens its conversation, or its thread for replies. The unread count drives the rail badge and the dock badge.                                                                           | No. The feed is derived from fixtures, and `activity.views` is a live-capture gap.        |
| `apps/slackless-gui/src/sidebar.rs`                  | The Later row shows the in-progress count. `badge_count` returns unread Activity items plus unread DM messages.                                                                                                                                                                                                                                                                                                                            | No.                                                                                       |
| `crates/slackless-platform/src/macos.rs` (192 lines) | `BadgeSetter` calls `NSApplication.dockTile.setBadgeLabel` through `objc2`, and moves to the main queue when called from another thread. `Notifier` returns early in bundled builds, leaving the notification to GPUI's `SystemNotification`. In unbundled builds, where `UNUserNotificationCenter` would abort, it runs a detached `osascript display notification` and reaps the child process on a thread.                              | Local OS only.                                                                            |
| `crates/slackless-platform/src/lib.rs`               | The trait table now documents both notifier backends and the dock-badge backend.                                                                                                                                                                                                                                                                                                                                                           | n/a                                                                                       |

## Matrix changes (docs/feature-parity.md)

I changed 15 rows. The summary is now done 77, partial 84, planned 554 and deferred 841, for 1556 in total. Before this wave it was 76 / 76 / 560 / 844. The deferral reason "Undocumented API, live capture needed" went from 803 to 800 rows.

| Row                                     | Before   | After   | Basis                                                                                          |
| --------------------------------------- | -------- | ------- | ---------------------------------------------------------------------------------------------- |
| F-CORE-040 Activity view shortcut       | partial  | done    | Cmd+Shift+A and the rail tile open `ActivityFeed`                                              |
| F-CORE-060 Pin and unpin messages       | planned  | partial | Pins panel and "N Pinned" chip; no pin or unpin action                                         |
| F-CORE-144 Activity feed                | partial  | partial | Evidence now points to `activity.rs`; still fixture-derived                                    |
| F-SHELL-046 Activity feed contents      | planned  | partial | Mentions, reactions and thread replies only                                                    |
| F-CORE-149 Banner notifications         | partial  | partial | Now fired by Later reminders; still not by realtime events                                     |
| F-SHELL-173 OS notification integration | partial  | partial | Bundled and unbundled paths are wired; no realtime feed                                        |
| F-CORE-152 App icon badge (Mac)         | partial  | partial | Wired to the unread model; the badge toggle (F-SHELL-051) is missing                           |
| F-SHELL-178 Badge triggers              | planned  | partial | DMs, mentions, reactions and thread replies count; keywords, apps and overdue reminders do not |
| F-CORE-157 Save a message or file       | partial  | partial | Saves now land in Later; no `saved.add` and no file saves                                      |
| F-CORE-158 Saved section of Home        | planned  | partial | Later view and sidebar row                                                                     |
| F-CORE-159 Open saved item in context   | planned  | partial | Opens the workspace and conversation; does not scroll to the message                           |
| F-CORE-160 Clear saved items            | planned  | partial | Per-item remove, complete and archive; no bulk clear                                           |
| F-CORE-163 To-do reminders              | deferred | partial | Reminder presets plus a due notification; the wire format is still a live-capture gap          |
| F-CORE-164 Complete and filter to-dos   | deferred | partial | Later complete and archive with tabs; these are Later items, not Slack's newer To-dos          |
| F-CORE-167 Set a reminder on a message  | deferred | partial | "Remind me about this" submenu; no `date_due` is sent                                          |

Four rows stay deferred: F-CORE-161, F-CORE-162, F-CORE-165 and F-CORE-166, which are To-dos-specific. The "API coverage gaps" entry for F-CORE-161..167 stays, because the `saved.*` wire details are still uncaptured. F-SHELL-149 (separate Activity and To-dos windows) and F-SHELL-276 (Activity keyboard keys) stay planned.

## Follow-ups (not fixed here)

- `docs/research/phase6f-verification.md` does not exist, but the matrix cites it as evidence on many wave F rows. Either restore it or re-point those rows.
- No unit tests cover `Remind::at` or the activity fixture derivation. Both are pure functions and cheap to test.
- Nothing calls `saved.*`, `pins.*`, `bookmarks.*` or `activity.*`. The pause and schedule from wave D are also not checked before a notification is posted.
