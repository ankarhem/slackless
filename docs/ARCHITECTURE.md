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
slackless/                      (virtual workspace root: [workspace] in /Cargo.toml)
├── crates/
│   ├── slackless-types         Pure data model + serde. No I/O.
│   ├── slackless-api           Private web-client REST (xoxc + d cookie), edge API, uploads.
│   ├── slackless-rtm           wss-primary WebSocket client: connect, events, reconnect.
│   ├── slackless-store         Durable local-first SQLite (rusqlite, bundled → FTS5).
│   ├── slackless-auth          Session acquisition (embedded sign-in) + Keychain storage.
│   ├── slackless-platform      Trait-based OS integration; macos impl via objc2.
│   └── slackless-core          Orchestrator: owns the tokio thread; wires the above;
│                               exposes a command/event API to any front-end; backfill.
├── apps/
│   ├── slackless-gui           GPUI Kit desktop app (smol↔tokio bridge + views).
│   └── slackless-cli           clap binary: search/read/send/reply/react/auth (--json).
└── skill/                      Agent SKILL.md for the CLI (shipped, installed separately).
```

Dependency direction (acyclic): `types` ← {`api`, `rtm`, `store`, `auth`, `platform`} ← `core`
← {`gui`, `cli`}. Nothing depends on a front-end. No Slack logic lives in `gui` that `cli`
cannot reach.

## Crate responsibilities

### slackless-types

Message, Channel/Conversation, User, Team, File, Reaction, the `rich_text` block model, and the
id/timestamp newtypes (`Ts`, `ChannelId`, …). serde (de)serialization of Slack's JSON. The
`#[serde(other)]`/catch-all discipline that makes the client **patchable** (unknown fields and
event subtypes are preserved/ignored, never fatal) lives here and is tested here.

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

- **Immutable event log**: every message/edit/reaction/delete ever observed, append-only,
  keyed `(workspace, channel, ts[, edit_ts])`. **Never deleted by retention.**
- **Materialized current-state views** the UI reads (latest revision per message, counts).
- **FTS5 virtual table** over message text (+ file names), maintained by insert/update triggers;
  powers local search with Slack-style modifiers mapped to FTS5 MATCH + WHERE columns.
- **Migrations** from day one (this is long-lived user data).
- Source of truth for rendering: API/RTM write here first; front-ends read from here.

### slackless-auth

First-party sign-in: the client obtains its **own** session via an embedded WebKit sign-in
(candidate: `wry`/WKWebView in a gpui child surface — to be spiked in phase 3), capturing its
own token + `d` cookie — not by reading another app's storage. Persists to the macOS Keychain
(`security-framework`/objc2). Multi-workspace. `auth.test` validation; `invalid_auth`/
`token_revoked` → re-auth signal.

### slackless-platform

Trait per capability, selected at compile time:

- `Notifier` — GPUI's `SystemNotification` is available (guide §14), so this mostly adapts it.
- `BadgeSetter` (dock unread badge) and `StatusItem` — **no GPUI API; macOS impl via `objc2`**
  (`NSApplication.dockTile.badgeLabel`, `NSStatusItem`), guide §14.
- `SecretStore` (Keychain), `DeepLinkHandler` (`slack://`, `https://app.slack.com/client`).
  Other-OS impls are stubs that compile and no-op until those ports begin.

### slackless-core

Owns the tokio runtime thread. Starts `api`+`rtm`, feeds every inbound event into `store`,
runs background history **backfill** (`MessageScrollerState::prepend` semantics), and exposes a
front-end-agnostic API: a `Command` sender (send message, open channel, react, search, …) and
an `Event` receiver (store changed, connection state, …) over `futures::mpsc`. `Client::start()`
returns those handles; the GUI drains them via `cx.spawn`, the CLI via its own tokio loop.

### apps/slackless-gui

GPUI Kit (`gpui-kit = "0.10.0"`). Holds the bridge from guide §13 and the views built on the
components the spike proved (`MessageScroller`, `Textarea`, resizable panels, `Popover`,
`ContextMenu`, `Notification`, `Theme`, `TitleBar`). Builds the component gaps from guide §16
(mrkdwn→`TextRun` renderer, autocomplete, emoji picker, unread dividers, reaction chips,
threads pane, cookie-auth image cache).

### apps/slackless-cli

`clap`. Commands: `search`, `read`, `send`, `reply`, `react`, `channels`, `dms`, `auth`
(status/login). Every command supports `--json`. Drives `slackless-core` directly on tokio.

## Build facts (from the verified spike)

- `gpui-kit = "0.10.0"` → pins `gpui-pre 0.3.12`; reach `gpui` via `gpui_kit::gpui`.
- **Required workaround:** add `pathfinder_simd = { version = "0.5", features = ["pf-no-simd"] }`
  as a direct dep (nightly aarch64 ICE in `pathfinder_geometry`); cargo feature unification
  applies it graph-wide.
- Metal shaders compile at runtime (`runtime_shaders` default) — **no full Xcode needed**;
  disabling default features would require `xcrun metal`.
- Toolchain: fenix nightly 1.102, apple-sdk 14.4, from the repo dev shell (`direnv exec .`).
- Build with a shared `CARGO_TARGET_DIR=<repo>/target` and `CARGO_PROFILE_DEV_DEBUG=0` (disk).

## Workspace-level choices

- Root `Cargo.toml` is a virtual manifest (`[workspace]`, members `crates/*` + `apps/*`).
  Shared deps in `[workspace.dependencies]`; one pinned toolchain.
- Error handling: `thiserror` in libraries, `anyhow` only in the two app binaries.
- Async: `tokio` in libs/CLI; the GUI never blocks the smol executor — all awaiting Slack work
  crosses the channel bridge.
- Patchability: unknown JSON fields/subtypes are non-fatal everywhere; raw method+params are
  centralized so a Slack change is a localized edit.

## Foundation build order (phase 3)

`types` first → then `api`, `rtm`, `store`, `auth`, `platform` in parallel (disjoint crates) →
`core` depends on all → front-ends last. The CLI can light up as soon as `core` can authenticate
and read, giving an early headless test surface before the GUI.
