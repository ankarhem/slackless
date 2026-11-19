# Extra requirements (beyond Slack parity)

Source: user decision, 2026-11-19. These are BINDING requirements for slackless,
in addition to Slack desktop feature parity. The architecture phase must design
the store and search around them.

## X-1 — Durable local history (history never disappears)

slackless keeps a **permanent local copy of every message, thread reply, reaction,
edit, and file metadata it ever observes** — from both the realtime websocket feed
and `conversations.history` / `conversations.replies` backfill.

- The **local store is the source of truth for rendering.** The UI reads channel and
  thread history from the local store, not straight from a live API response. API/WS
  results are written into the store first, then the UI renders from it.
- **Nothing is ever hard-deleted by retention/eviction.** Slack hides old history
  (free-workspace ~90-day limit; server scrollback caps), so without a local archive
  the user's history effectively disappears. This store makes it permanent.
- A Slack `message_deleted` / `message_changed` event is recorded as a new revision
  (tombstone + prior content retained) so the archive keeps what was there; the UI
  renders the current state (e.g. "deleted") but the local copy is not lost. A purge
  is only ever an explicit, user-initiated action. (Confirm this exact deletion policy
  with the user before shipping.)
- Dedupe/identity key: `(workspace_id, channel_id, ts)`; edits keyed by `(…, ts, edit_ts)`.
- On channel open and on every relevant WS event: upsert into the store. A background
  task opportunistically backfills gaps (walks `conversations.history` cursors) so the
  archive fills in over time, not just forward from install.

## X-2 — Local full-text search (SQLite FTS5)

A local SQLite database with an **FTS5** virtual table indexing all stored message
text (and, where cheap, file names and canvas/snippet text).

- Search runs **locally against FTS5** — instant, offline-capable, and covers the full
  archive even when the server no longer returns that history.
- Map Slack's search modifiers onto FTS5 + structured columns where feasible:
  `in:`, `from:`, `to:`, `before:`, `after:`, `on:`, `has:` (link/reaction/file),
  `is:thread`. Free text goes to the FTS5 MATCH; structured filters are WHERE clauses.
- Keep Slack's server-side `search.modules` too: server search reaches content not yet
  synced locally; local FTS5 owns everything in the archive. The UI can merge/prefer
  local results and fall back to server for the rest.

## X-3 — CLI front-end + agent skill

slackless is also exposed as a **command-line binary** (`slackless`) so an agent (or a
power user) can **search, read, and write messages headlessly**, without the GUI.

- Commands (minimum): `search <query>` (local FTS5 + optional server search),
  `read <channel|dm> [--thread <ts>] [--limit N] [--since …]`, `send <channel|dm> <text>`,
  `reply <channel> <thread_ts> <text>`, `react <channel> <ts> <emoji>`, plus `channels`,
  `dms`, and `auth` (sign-in / status). Every command supports `--json` as well as human
  output so an agent can parse results.
- The CLI reuses the **same core crates** as the GUI (auth, API client, realtime + store,
  search). No Slack logic lives in the GUI crate that the CLI cannot also reach.
- Ships with an **accompanying agent skill** (a `SKILL.md`) documenting how an agent drives
  the CLI: commands, flags, JSON shapes, and auth setup. The skill ships inside the repo
  (e.g. `skill/`); installing it into the user's agent skills path is a separate,
  user-approved step.
- Bonus: this is a **headless verification surface** — the API client, store, and FTS5
  search are exercisable through the CLI without building the GPUI app.

## Architectural consequences (for phase 2)

- The Foundation "store/cache" crate is a **durable local-first SQLite store**, append-only
  for messages, acting as the source of truth for the UI (not a disposable cache).
- Pick the SQLite layer in phase 2. `rusqlite` with the `bundled` feature ships a SQLite
  that includes FTS5; `sqlx` (sqlite) is the async alternative — decide against the
  gpui/tokio threading model documented in the GPUI Kit guide.
- Schema must separate the **immutable event log** (every message/edit/reaction observed)
  from **materialized current-state views** the UI reads, with the FTS5 index maintained
  by triggers on insert/update.
- Define a schema-migration story from day one, since this DB is long-lived user data.
- Workspace layout puts all Slack logic in library crates (core / api / store / search) with
  TWO thin front-ends depending on them: `slackless-gui` (GPUI Kit) and `slackless-cli`
  (clap). The GUI crate holds no business logic the CLI cannot also reach.
