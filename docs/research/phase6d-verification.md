# Phase 6 wave D verification

Wave D is preferences, appearance, profile, status, DND and the quick switcher. It was checked on 2026-11-19 in fixture mode, with no network calls and no credentials. Every command ran with `CARGO_TARGET_DIR=/Users/ankarhem/repos/ankarhem/slackless/target CARGO_PROFILE_DEV_DEBUG=0`.

## Gates

| Gate          | Command                                                                                        | Exit | Result                                                                                         |
| ------------- | ---------------------------------------------------------------------------------------------- | ---- | ---------------------------------------------------------------------------------------------- |
| Build         | `cargo build --workspace`                                                                      | 0    | `Finished dev profile`                                                                         |
| Test          | `cargo test --workspace`                                                                       | 0    | 92 passed, 0 failed, 2 ignored, across all test binaries                                       |
| Clippy        | `cargo clippy --workspace --all-targets -- -D warnings`                                        | 0    | No warnings (`Checking slackless-gui ... Finished`)                                            |
| Launch        | `caffeinate -u -t 2; SLACKLESS_GUI_DEMO=prefs ./target/debug/slackless-gui` (8 s), then `kill` | 0    | The process ran (pid found by `pgrep`) and stopped with SIGTERM. Nothing was logged to stderr. |
| Frozen crates | `find crates -name '*.rs' -newer docs/research/phase3-verification.md`                         | 0    | Empty output, so `crates/` is untouched                                                        |

No fixes were needed in `apps/**`. Wave D landed green, and this verification changed only docs.

## Screenshot: locked display

- `docs/research/screenshots/slackless-p6-prefs.png` (3840x2160) is the full-screen `screencapture -x` the task asked for. **It is not black, but it shows only the macOS lock screen.** The session was locked during the run, and `caffeinate -u` wakes the display but does not unlock it. The file is kept as a record of the attempt; it is not evidence of the UI.
- I also tried a capture by window id (`screencapture -o -l <id>`), the method that produced `slackless-final-window.png` in final-verification. It failed because the CoreGraphics on-screen window list returned no `slackless` window while the session was locked. No window capture exists.
- To capture the evidence later on an unlocked session, run `SLACKLESS_GUI_DEMO=<mode> ./target/debug/slackless-gui`. The modes are `prefs` (Preferences, opened on Appearance & Themes), `profile`, `status`, `switcher` and `menu` (app.rs, demo hook).

Visual rendering of the wave D views is therefore **unverified** in this report. The rows below rest on the modules, the build and the clippy pass, as listed in each row's Evidence cell.

## What wave D contains (apps/slackless-gui/src)

| Module           | Surface                                                                                                                                                                                                                                                                   | Applied by the app?                                                                                                                                                                                               |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `preferences.rs` | Preferences dialog (Cmd+, or the user menu) with 9 sections: Notifications, Navigation, Home, Appearance & Themes, Messages & media, Language & region, Accessibility, Mark as read, Advanced                                                                             | Applied: Light/Dark/System and the colour palette (`theme::activate_with`), Enter-to-send (composer and thread), and the notification-schedule summary (user menu). Every other control is stored in memory only. |
| `theme.rs`       | `Palette`: Default, Aubergine, Clementine, Banana, Jade, Lagoon, Barbra, Mood Indigo, and Custom (preset sidebar and accent swatches)                                                                                                                                     | Yes, live                                                                                                                                                                                                         |
| `profile.rs`     | Profile pane in the right panel (exclusive with threads): avatar, name, pronouns, title, status, presence, local time, email and phone, and "About me" fields. It has Set a status and Message actions.                                                                   | Read-only                                                                                                                                                                                                         |
| `status.rs`      | `StatusPicker` (12 emoji, text, "remove after", 5 suggested statuses); `UserMenu` (status, away/active, pause notifications for 30 minutes to "until next week", resume, schedule, profile, preferences); `self_avatar` (presence dot, away ring, `z` badge while paused) | Kept in memory                                                                                                                                                                                                    |
| `switcher.rs`    | Cmd+K quick switcher over joined channels and DMs: name-prefix matches first, then substring matches; Up/Down to move; Enter or click to open                                                                                                                             | Yes                                                                                                                                                                                                               |

None of these call Slack yet. `users.prefs.set`, `users.profile.set`, `presence.set` and `dnd.setSnooze` are follow-ups, and settings reset at launch.

## Feature-parity rows updated

All rows cite the module and this report. "Fixture" means the behavior works in memory against the fixtures.

| ID          | Feature                       | Was      | Now                        | Why not done (if partial)                                                    |
| ----------- | ----------------------------- | -------- | -------------------------- | ---------------------------------------------------------------------------- |
| F-CORE-011  | Preferences window            | planned  | partial                    | It is a dialog rather than a separate window; nothing is persisted or synced |
| F-CORE-025  | Quick switcher                | partial  | partial (evidence updated) | Local filter only; no edge search or frecency                                |
| F-CORE-074  | Enter sends                   | done     | done (evidence extended)   | -                                                                            |
| F-CORE-150  | Notification schedule and DND | planned  | partial                    | The notifier does not enforce the pause or schedule; no `dnd.*`              |
| F-SHELL-001 | Open own profile pane         | planned  | done                       | -                                                                            |
| F-SHELL-003 | Standard profile fields       | planned  | partial                    | Read-only                                                                    |
| F-SHELL-007 | Custom profile fields         | planned  | partial                    | Read-only; no `team.profile.get`                                             |
| F-SHELL-009 | Profile sections              | planned  | partial                    | Read-only                                                                    |
| F-SHELL-012 | Local time on profile         | planned  | done                       | -                                                                            |
| F-SHELL-013 | Set status                    | planned  | done                       | -                                                                            |
| F-SHELL-014 | Status emoji                  | planned  | done                       | -                                                                            |
| F-SHELL-015 | Suggested statuses            | deferred | partial                    | Only Slack's defaults; workspace suggestions still need a capture            |
| F-SHELL-016 | Status expiration             | planned  | partial                    | The choice is stored but the status is never cleared                         |
| F-SHELL-022 | Edit or clear status          | planned  | done                       | -                                                                            |
| F-SHELL-027 | Set yourself away/active      | deferred | partial                    | UI toggle only; the `presence.set` semantics still need a capture            |
| F-SHELL-028 | Pause notifications           | planned  | partial                    | The notifier does not check the pause                                        |
| F-SHELL-029 | Resume notifications          | planned  | done                       | -                                                                            |
| F-SHELL-030 | Snooze indicator              | planned  | done                       | -                                                                            |
| F-SHELL-041 | Notifications tab             | planned  | partial                    | Only the schedule summary is used                                            |
| F-SHELL-062 | Navigation tab                | planned  | partial                    | Stored only                                                                  |
| F-SHELL-075 | Theme gallery                 | planned  | done                       | -                                                                            |
| F-SHELL-077 | Custom theme colors           | planned  | partial                    | Preset swatches only; no free colour entry                                   |
| F-SHELL-082 | Dark mode                     | done     | done (evidence extended)   | -                                                                            |
| F-SHELL-083 | Font                          | planned  | partial                    | Stored only                                                                  |

Some individual preferences have a control that is stored but changes nothing in the UI: density, name display, jumbomoji, emoticon conversion, inline media, link previews, language, time zone, zoom, animation, mark-as-read behavior, launch at login and hardware acceleration. Those rows stay **planned**, because a control that changes nothing is not part of the feature.

Status summary after the update: done 55 (was 47), partial 51 (was 38), planned 601 (was 620), deferred 849 (was 851), total 1556 (unchanged). I recounted these from the Status column of every `| F-` row, and they match the table in `docs/feature-parity.md`.

## Crates confirmation

`find /Users/ankarhem/repos/ankarhem/slackless/crates -name '*.rs' -newer /Users/ankarhem/repos/ankarhem/slackless/docs/research/phase3-verification.md` printed nothing, both before and after this verification. No file under `crates/` was modified.
