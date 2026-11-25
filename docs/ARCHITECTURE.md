# slackless — architecture

Grounded in the phase-1 research: `docs/research/gpui-kit-guide.md` (verified against a
compiling spike), the `docs/research/api-*.md` references, and `docs/research/extra-features.md`
(durable local store, FTS5 search, CLI + skill). Target: aarch64-apple-darwin first; all
OS-specific code behind traits so Windows/Linux can follow.

## The one constraint that shapes everything

GPUI's async runtime is **smol-based, not tokio** (guide §13). A tokio HTTP/WebSocket stack
cannot run on it. Verified working pattern: the Slack networking core owns a **dedicated OS
thread running a tokio runtime**, and talks to any front-end over runtime-agnostic
`futures::channel::mpsc` channels — commands in, events out. The GUI drains events with
`cx.spawn`; the CLI, already tokio, drains them directly.

Consequence: **all Slack logic lives in tokio-native library crates; the front-ends are thin.**
The GUI adds only the smol↔tokio bridge; the CLI needs no bridge at all. This also makes the
CLI a headless verification surface for the whole core without building the GPUI app.

## Workspace layout

```
slackless/                      (virtual workspace root: Rust 2024 edition in /Cargo.toml)
├── crates/
│   ├── slackless-types         Pure data model + serde + time/net helpers. No I/O.
│   ├── slackless-api           Private web-client REST (xoxc + d cookie), edge API, uploads.
│   ├── slackless-rtm           wss-primary WebSocket client: connect, events, reconnect.
│   ├── slackless-store         Durable local-first zlib-compressed SQLite (V10 schema + FTS5).
│   ├── slackless-auth          Session acquisition (embedded WebKit sign-in) + Keychain storage.
│   ├── slackless-platform      Trait-based OS integration (`DesktopPlatform`) + macOS AppKit overlays.
│   ├── slackless-huddle        Headless Chime WebRTC media, clips, and `NativeVideoPlayer` (no AppKit).
│   └── slackless-core          Orchestrator + `domain/` logic; owns the tokio thread; wires the above;
│                               exposes a command/event API to any front-end; backfill.
├── apps/
│   ├── slackless-gui           Thin GPUI Kit desktop app (smol↔tokio bridge + modular views).
│   └── slackless-cli           Thin clap binary: search/read/send/watch/huddle/canvas/list/auth (--json).
└── skill/                      Agent SKILL.md for the CLI (shipped, installed separately).
```

Dependency direction (acyclic): `types` ← {`api`, `rtm`, `store`, `auth`, `platform`, `huddle`} ← `core`
← {`gui`, `cli`}. Nothing depends on a front-end. No Slack domain logic lives in `gui` that `cli`
cannot reach, and no AppKit GUI linkage lives in `slackless-huddle`.

## Crate responsibilities

### slackless-types

Message, Channel/Conversation, User, Team, File, Reaction, the `rich_text` block model, and the
id/timestamp newtypes (`Ts`, `ChannelId`, …). serde (de)serialization of Slack's JSON plus shared
pure utilities (`de`, `time`, `net`, `endpoints`). The `#[serde(other)]`/catch-all discipline that
makes the client **patchable** (unknown fields and event subtypes are preserved/ignored, never
fatal) lives here and is tested here.

### slackless-api

`reqwest` client. Auth model: `Cookie: d=<xoxd…>` + `token=<xoxc…>` in the form body (see
`api-auth.md`). One typed method per endpoint across the 12 researched areas, cursor pagination,
rate-limit (429/`Retry-After`) handling, multipart upload flow
(`getUploadURLExternal`→PUT→`completeUploadExternal`), edge-API host. Returns `types`. Every
endpoint keeps the raw method string + params in one place so a changed endpoint is a one-line
patch.

### slackless-rtm

`tokio-tungstenite` to `wss://wss-primary.slack.com/...`. Builds the URL, handles
hello/reconnect_url/goodbye, ping/pong, presence/typing frames, and parses the full event
catalog (`api-realtime.md`) into typed `types` events. Reconnect with backoff + resync hook.

### slackless-store (extra requirement X-1 / X-2)

`rusqlite` with the **`bundled`** feature (ships a SQLite that includes **FTS5**). SQLite is
blocking, so the store runs on its own thread (or `spawn_blocking`) behind an async handle.

- **V10 schema (`PRAGMA user_version = 10`)** with monotonic, idempotent migrations and future-schema
  guarding.
- **Immutable event log** (`message_revision`): every message/edit/delete ever observed,
  append-only, keyed `(workspace_id, channel_id, ts, edit_ts)`, zlib-compressed JSON payloads.
  **Never deleted by retention.**
- **Materialized current-state tables** (`message_current` with an inline `has_reaction` flag
  replacing the write-amplifying `reaction` table, plus channels, users, canvases, huddle
  transcripts, and read cursors) with trimmed non-redundant indexes.
- **FTS5 virtual tables** (`message_fts` over message text + file names, and `huddle_transcript_fts`
  over huddle captions), maintained by insert/update/delete triggers; powers local search with
  Slack-style modifiers mapped to FTS5 MATCH + WHERE columns.
- Source of truth for rendering: API/RTM write here first; front-ends read from here.

### slackless-auth

First-party sign-in: the client obtains its **own** session via an embedded WebKit sign-in
capturing its own token + `d` cookie — not by reading another app's storage. Persists to the
macOS Keychain (`security-framework`). Multi-workspace. `auth.test` validation; `invalid_auth`/
`token_revoked` → re-auth signal.

### slackless-platform

All OS/AppKit integration lives here behind compile-time traits and safe wrappers, keeping GUI
windowing decoupled from headless media/networking crates:

- **`DesktopPlatform` supertrait** combining `Notifier`, `BadgeSetter`, `IconSetter`, `AboutPanel`,
  `FullScreenItem`, `SoundPlayer`, `StatusBarItem`, `AttentionRequester`, and `SpellChecker`,
  selected via `current()` (macOS `objc2` impl) or `noop()` (in-memory headless test backend).
- **AppKit overlay & floating panel UI** (`src/macos_overlay.rs` + `native/macos/appkit_overlay.m`):
  `DesktopDrawOverlay` (`OverlayDrawStroke`, `OverlayCursorState`, `OverlayPenEvent`) and
  `configure_floating_panel`, compiled with `-fobjc-arc` on macOS and stubbed as safe no-ops on
  non-macOS targets.
- **Cross-platform utilities**: `app_data_dir` / `app_cache_dir` / `app_config_dir` /
  `archive_store_path`, pure `DeepLink` / `parse_deep_link`, structured file logging, mouse
  navigation / screen-lock monitors, and POSIX `SIGTERM`/`SIGINT` self-pipe termination monitor.

### slackless-huddle

Headless Amazon Chime WebRTC (`webrtc 0.21` / `rtc 0.21`) media stack (`audio/`, `session/`,
`rtc.rs`, `video.rs`, `av1.rs`, `clips/`, `content_share.rs`, `video_player.rs`, `backend/`),
with **zero AppKit linkage**:

- **Split modular macOS native media bridge (`native/macos/`)**:
  - `cvpixel.m`: zero-copy `420f` IOSurface `CVPixelBuffer` (`MacVideoSurface`) & VTPixelTransfer/vImage BGRA readback.
  - `vt_h264.m` / `vt_av1.m`: VideoToolbox hardware H.264 encode/decode and AV1 hardware decode.
  - `screen_capture.m`: ScreenCaptureKit screen/window capture and `SCScreenshotManager` thumbnails.
  - `camera_vision.m`: AVFoundation camera capture + Vision person segmentation background blur.
  - `vpio_audio.m` / `audio_player.m` / `audio_tap.m`: CoreAudio `VoiceProcessingIO`, `AVAudioEngine`/`AVAudioPlayer` routed audio playback, and system audio loopback taps.
  - `video_player.m` + `video_player.rs`: **`NativeVideoPlayer`** (`AVPlayer` + `AVPlayerItemVideoOutput` producing zero-copy `420f` IOSurface `MacVideoSurface` frames for GPUI's Metal renderer).
- **Portable fallbacks**: non-macOS targets use `cpal`/`sonora`/`nokhwa`/`xcap`/`openh264`/`rav1d` behind backend traits, and `NativeVideoPlayer::open` returns `HuddleError::Unsupported`.

### slackless-core

Owns the tokio runtime thread. Starts `api`+`rtm`, feeds every inbound event into `store`,
runs background history **backfill** (`MessageScrollerState::prepend` semantics), and exposes a
front-end-agnostic API: a `Command` sender (send message, open channel, react, search, …) and
an `Event` receiver (store changed, connection state, …) over `futures::mpsc`. `Client::start()`
returns those handles; the GUI drains them via `cx.spawn`, the CLI via its own tokio loop.

Shared front-end-agnostic Slack domain logic lives under **`slackless_core::domain`**:
- `canvas`: canvas Markdown / Quill block conversion.
- `huddles`: huddle state helpers and summary formatting.
- `lists`: Slack Lists parsing and schema helpers.
- `slash`: slash-command parsing and dispatch helpers.
- `updater`: release manifest parsing and verification.
- `webvtt`: WebVTT subtitle parsing and export.

### apps/slackless-gui

Thin GPUI Kit desktop front-end (`gpui-kit = "0.11.0"`, `gpui-pre = "0.3.14"`). Holds the
smol↔tokio bridge from guide §13 and modularized views (`app/{chrome,events,dialogs,navigation}`,
`canvas/`, `composer/`, `emoji/`, `files/`, `lists/`, `messages/`, `preferences/sections/`,
`search/`, `sidebar/`, `popout/`, `blockkit/`). Delegates OS overlays to `slackless-platform`,
media/video playback to `slackless-huddle`, and all Slack state/domain rules to `slackless-core`.

### apps/slackless-cli

Thin `clap` binary (`cli.rs`, `commands/`, `live/`). Commands: `search`, `read`, `send`, `reply`,
`react`, `channels`, `dms`, `unread`, `watch`, `huddle`, `canvas`, `list`, `saved`, `backfill`,
`download`, `stats`, `vacuum`, `auth`. Every command supports `--json`. Drives `slackless-core`
and `slackless-store` directly on tokio.

## Build & quality facts

- `gpui-kit = "0.11.0"` → pins `gpui-pre 0.3.14`; reach `gpui` via `gpui_kit::gpui`.
- Workspace runs on **Rust 2024 edition** with `dbg_macro`, `todo`, `unimplemented`, and non-test
  `unwrap_used` denied via `[workspace.lints.clippy]` + `clippy.toml` (`allow-unwrap-in-tests = true`).
- Supply-chain security is checked via `cargo deny check` (`deny.toml`, zero ignored advisories).
- Metal shaders compile at runtime (`runtime_shaders` default) — **no full Xcode needed**;
  disabling default features would require `xcrun metal`.
- Common workflows live in `justfile`: `just validate`, `just test`, `just bench`, `just run`, `just install`.
- Build with `CARGO_PROFILE_DEV_DEBUG=0` inside `nix develop` (or `direnv`, which sets
  `NIX_CC_WRAPPER_SUPPRESS_TARGET_WARNING=1`) for fast, low-disk dev builds.

## Workspace-level choices

- Root `Cargo.toml` is a virtual manifest (`[workspace]`, members `crates/*` + `apps/*`).
  Shared deps in `[workspace.dependencies]`; one pinned toolchain.
- Error handling: `thiserror` in libraries, `anyhow` only in the two app binaries.
- Async: `tokio` in libs/CLI; the GUI never blocks the smol executor — all awaiting Slack work
  crosses the channel bridge.
- Patchability: unknown JSON fields/subtypes are non-fatal everywhere; raw method+params are
  centralized so a Slack change is a localized edit.
