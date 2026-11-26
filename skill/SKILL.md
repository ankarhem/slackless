---
name: slackless-cli
description: This skill should be used when an AI agent needs to search, read, send, reply, or react in Slack through the slackless CLI, a headless client that reads a local archive offline and posts through a stored desktop-app session.
---

# slackless CLI

## Overview

`slackless` is the command-line side of the slackless desktop Slack client. It is built for agents and scripts: every command prints a single JSON line under `--json` (or NDJSON for streaming commands) and tab-separated text otherwise. Nothing needs a live Slack connection to start.

Two kinds of commands exist.

- **Offline archive commands** (`search`, `read`, `thread`, `channels`, `dms`, `users`, `digest`, `threads`, `revisions`, `export`, `stats`, `vacuum`, `huddle devices`, `huddle cameras`, `huddle screens`, `auth status`) work offline against the local SQLite archive (or local host media devices). Archive read commands always exit 0; with no known workspace they print empty results (plus a stderr hint in human mode) instead of failing.
- **Live Slack commands** (`send`, `reply`, `edit`, `delete`, `react`, `unreact`, `upload`, `thread --live`, `mark-read`, `mark-thread`, `pins`, `pin`, `unpin`, `schedule`, `scheduled`, `saved`, `files`, `download`, `canvas`, `list`, `status`, `unread`, `watch`, `backfill`, `huddle join|link|invite`) talk to Slack using a stored session (`slackless auth login` or the desktop app). Every REST command is one deterministic invocation; `send`, `reply`, `edit`, `schedule`, and `canvas comment` accept `-` as `<TEXT>` to read multi-line bodies from stdin. `watch` opens a live websocket and streams NDJSON events until interrupted (or `--count N`); `huddle join` connects natively to Amazon Chime audio until interrupted or `--duration N`. `slackless update` checks for and installs new Slackless releases from GitHub Releases (updating both `Slackless.app` and the CLI).

- **MCP server** (`slackless mcp`) exposes the local archive and live messaging actions over standard JSON-RPC 2.0 stdio for MCP-compatible AI clients (Claude Desktop, Claude Code, Cursor, etc.).

## Setup and auth

Sign-in is interactive: `slackless auth login` (or the desktop app's first launch) opens Slack's own sign-in page in a window, and a human completes it (email code, SSO, 2FA). Do not run it unattended; ask the user to sign in when `auth status` reports `signed_in: false`. The CLI and the desktop app share the stored session. macOS may ask once to let the `slackless` binary read the Keychain item.

- Credentials live in the platform secret store, one session per team id.
- The archive lives at `~/Library/Application Support/slackless/<TEAM_ID>.sqlite` on macOS and `~/.local/share/slackless/<TEAM_ID>.sqlite` on Linux. It fills up as the desktop app syncs or via `slackless backfill`, so a fresh workspace reads as empty.
- Start with `slackless auth status --json`. It lists every stored session with its team id, the signed-in user, whether secrets are present, and the archive path with a message count. The `team_id` values are what you pass to `--workspace`.

### Global flags & workspace selection

- `--json` — emit machine-readable JSON (works anywhere on the command line).
- `--workspace TEAM_ID` — workspace team id to act on. When omitted:
  - Offline archive commands fall back to the first stored session, then to the sole archive on disk. When nothing is known, or several archives exist and no session disambiguates them, local commands return empty results rather than guess.
  - Live commands use the stored session of the flagged team id, or the first stored session. That session must have credentials, or the command fails with `not_signed_in`.

Channel arguments (`<CHANNEL>`) across commands accept a channel id (`C0123ABC`, `D0123ABC`, `G0123ABC`), a `#name` or bare name (case-insensitive), or an `@handle` for the DM with that user.

## Commands

### Offline archive commands

#### search

```sh
slackless search <QUERY> [--limit N] [--all-workspaces] [--format md|json] [--out PATH]
```

Full-text search of the local archive. Pass `--all-workspaces` to search across every locally archived workspace and merge hits newest first. Pass `--format md|json` and/or `--out PATH` (single-workspace mode) to export matches as Markdown or pretty JSON.

The query is one quoted string with Slack-style modifiers:

- `in:#eng`, `in:eng`, or `in:C0123ABC` restricts to a channel (repeats OR together); `-in:#alerts` excludes a channel.
- `from:@ana` or `from:U0123ABC` restricts to an author (repeats OR together); `-from:@bot` excludes an author.
- `has:link`, `has:file`, `has:reaction`, `has:pin` require that element.
- `before:2026-01-31` and `after:2026-01-01` are UTC day bounds, both exclusive.
- Trailing `*` acts as a prefix wildcard (`deploy*`); `-word` and `-"exact phrase"` exclude matches.
- Everything else is free text, and a quoted phrase stays one term. Different modifiers combine with AND. Unknown `foo:bar` tokens or invalid dates stay free text rather than erroring.

`--limit N` caps results (default `20`).

JSON output (single workspace):

```json
{ "query": "launch in:#eng", "count": 1, "messages": ["<MessageRow>"] }
```

JSON output (`--all-workspaces` adds `workspace` to each `MessageRow`):

```json
{
  "query": "launch",
  "all_workspaces": true,
  "workspaces_searched": ["T0123ABC", "T9999XYZ"],
  "count": 1,
  "messages": [{ "workspace": "T0123ABC", "channel": "C0123ABC", "ts": "..." }]
}
```

JSON output (`--out PATH --json`):

```json
{ "ok": true, "path": "matches.json", "count": 1 }
```

#### read

```sh
slackless read <CHANNEL> [--limit N] [--since WHEN]
```

Print recent messages from a conversation (default `--limit 50`). You get the newest page, printed oldest first. `--since WHEN` filters to messages at or after a cutoff: compact duration (`30m`, `24h`, `7d`, `2w`), UTC date (`YYYY-MM-DD`), or unix epoch seconds.

JSON output:

```json
{
  "channel": {
    "id": "C0123ABC",
    "name": "eng",
    "kind": "public_channel",
    "user": null
  },
  "count": 1,
  "messages": ["<MessageRow>"]
}
```

`channel.name`, `channel.kind`, and `channel.user` are omitted when the archive does not know the conversation.

#### thread

```sh
slackless thread <CHANNEL> <THREAD_TS> [--live] [--limit N]
```

Read a thread's root message and all replies, oldest first. Offline against the local archive by default; pass `--live` to fetch fresh replies via `conversations.replies` (with `--limit N`, default `200`, Slack max `999`) and mirror them into the local SQLite archive.

JSON output:

```json
{
  "channel": "C0123ABC",
  "thread_ts": "1739280000.000100",
  "live": false,
  "count": 2,
  "messages": ["<MessageRow>"]
}
```

#### channels

```sh
slackless channels
```

List public and private channels known to the archive, alphabetical by name.

JSON output:

```json
{
  "count": 1,
  "channels": [
    {
      "id": "C0123ABC",
      "name": "eng",
      "kind": "public_channel",
      "is_archived": false,
      "is_member": true,
      "topic": "Deploy talk"
    }
  ]
}
```

#### dms

```sh
slackless dms
```

List direct messages (`im`) and group DMs (`mpim`) known to the archive, most recently active first.

JSON output:

```json
{
  "count": 1,
  "dms": [
    {
      "id": "D0123ABC",
      "user": "U0123ABC",
      "user_name": "ana",
      "kind": "im",
      "name": null
    }
  ]
}
```

#### users

```sh
slackless users [QUERY] [--include-deleted]
```

List or search workspace users offline from the local archive. Optional `QUERY` matches case-insensitive substrings across user id, handle, display name, or real name (leading `@` stripped). Pass `--include-deleted` to include deactivated accounts.

JSON output:

```json
{
  "count": 1,
  "users": [
    {
      "id": "U0123ABC",
      "name": "ana",
      "display_name": "Ana",
      "real_name": "Ana Lovelace",
      "is_bot": false,
      "deleted": false
    }
  ]
}
```

#### digest

```sh
slackless digest [--since WHEN] [--all-workspaces] [--limit N]
```

Build an offline activity summary grouped by active conversation over a time window (`--since`, default `24h`; accepts `30m`, `24h`, `7d`, `2w`, `YYYY-MM-DD`, or unix seconds), sorted by message count then recency. `--limit N` defaults to `25` per workspace; `--all-workspaces` aggregates across all archived workspaces.

JSON output:

```json
{
  "since": "24h",
  "cutoff_secs": 1739193600,
  "all_workspaces": false,
  "total_messages": 12,
  "count": 1,
  "conversations": [
    {
      "workspace": "T0123ABC",
      "channel": "C0123ABC",
      "label": "#eng",
      "message_count": 12,
      "author_count": 4,
      "latest_ts": "1739280000.000100",
      "latest_author": "Ana",
      "latest_text": "Deploy is green"
    }
  ]
}
```

#### threads

```sh
slackless threads [--channel CHANNEL] [--limit N]
```

List locally archived thread roots with replies (`reply_count > 0`), newest first (default `--limit 30`).

JSON output:

```json
{ "count": 1, "threads": ["<MessageRow>"] }
```

#### revisions

```sh
slackless revisions <CHANNEL> <TS>
```

Show the immutable append-only local revision history (original, edits, and deletions) for one message.

JSON output:

```json
{
  "channel": "C0123ABC",
  "ts": "1739280000.000100",
  "count": 2,
  "revisions": [
    { "edit_ts": "", "deleted": false, "text": "teh original" },
    { "edit_ts": "1739280050.000000", "deleted": false, "text": "the edited" }
  ]
}
```

#### export

```sh
slackless export <CHANNEL> [--format md|json] [--limit N] [--out PATH]
```

Export a conversation's archived messages (default `--format md`, `--limit 10000`, oldest first) to stdout or `--out PATH`.

JSON output (stdout via `--json` or `--format json`):

```json
{
  "workspace": "T0123ABC",
  "channel": { "id": "C0123ABC", "label": "#eng" },
  "count": 1,
  "messages": ["<MessageRow>"]
}
```

JSON output (`--out PATH --json`):

```json
{ "ok": true, "path": "eng.json", "count": 1 }
```

#### stats

```sh
slackless stats [--breakdown]
```

Show local SQLite archive size and row counts. With `--breakdown`, includes per-table counts (`tables`) and the top 10 largest channels (`top_channels`).

JSON output:

```json
{
  "workspace": "T0123ABC",
  "db_path": "/Users/you/Library/Application Support/slackless/T0123ABC.sqlite",
  "db_bytes": 1048576,
  "messages": 4200,
  "revisions": 4350,
  "conversations": 85,
  "users": 120,
  "offline_queued": 0,
  "tables": [{ "table": "message_current", "rows": 4200 }],
  "top_channels": [{ "id": "C0123ABC", "name": "eng", "messages": 1500 }]
}
```

#### vacuum

```sh
slackless vacuum
```

Compress legacy uncompressed JSON rows, checkpoint WAL, compact unused pages (`VACUUM`), and optimize FTS5 indexes.

JSON output:

```json
{
  "ok": true,
  "workspace": "T0123ABC",
  "before_bytes": 2097152,
  "after_bytes": 1048576,
  "reclaimed_bytes": 1048576
}
```

---

### Live messaging, reactions & uploads

#### send

```sh
slackless send <CHANNEL> <TEXT>
```

Post a message (`chat.postMessage`) and mirror the echo into the local archive. Pass `-` as `<TEXT>` to read the body from stdin.

JSON output:

```json
{ "ok": true, "channel": "C0123ABC", "ts": "1739280000.000100" }
```

#### reply

```sh
slackless reply <CHANNEL> <THREAD_TS> <TEXT>
```

Reply in a thread. `<THREAD_TS>` is the thread root ts from `read`, `search`, or `threads`. Pass `-` as `<TEXT>` to read from stdin.

JSON output:

```json
{
  "ok": true,
  "channel": "C0123ABC",
  "thread_ts": "1739280000.000100",
  "ts": "1739280123.000200"
}
```

#### edit

```sh
slackless edit <CHANNEL> <TS> <TEXT>
```

Edit an existing message (`chat.update`) and mirror the updated message into the local archive. Pass `-` as `<TEXT>` to read from stdin.

JSON output:

```json
{ "ok": true, "channel": "C0123ABC", "ts": "1739280000.000100" }
```

#### delete

```sh
slackless delete <CHANNEL> <TS>
```

Delete a message (`chat.delete`).

JSON output:

```json
{
  "ok": true,
  "channel": "C0123ABC",
  "ts": "1739280000.000100",
  "deleted": true
}
```

#### react / unreact

```sh
slackless react <CHANNEL> <TS> <EMOJI>
slackless unreact <CHANNEL> <TS> <EMOJI>
```

Add (`reactions.add`) or remove (`reactions.remove`) an emoji reaction. `<EMOJI>` accepts a shortcode with or without colons (`tada` or `:tada:`).

JSON output:

```json
{ "ok": true, "channel": "C0123ABC", "ts": "1739280000.000100", "name": "tada" }
{ "ok": true, "channel": "C0123ABC", "ts": "1739280000.000100", "name": "tada", "removed": true }
```

#### upload

```sh
slackless upload <CHANNEL> <FILES...> [-c|--comment TEXT] [--thread-ts TS]
```

Upload one or more local files into a conversation or thread via Slack's v2 external upload flow.

JSON output:

```json
{
  "ok": true,
  "channel": "C0123ABC",
  "thread_ts": null,
  "count": 1,
  "files": [
    {
      "id": "F0123ABC",
      "name": "report.pdf",
      "title": "report.pdf",
      "mimetype": "application/pdf",
      "size": 12345,
      "permalink": "https://acme.slack.com/files/..."
    }
  ]
}
```

---

### Read cursors, pins, scheduling & saved items

#### mark-read

```sh
slackless mark-read <CHANNEL> [TS]
```

Advance a conversation's read cursor (`conversations.mark`). `[TS]` defaults to the newest locally archived non-deleted message ts in `<CHANNEL>` (fails with `missing_ts` if the archive has none).

JSON output:

```json
{ "ok": true, "channel": "C0123ABC", "ts": "1739280000.000100" }
```

#### mark-thread

```sh
slackless mark-thread <CHANNEL> <THREAD_TS> [TS] [--unread]
```

Mark a followed thread read (`read: true`) or unread (`--unread`, `read: false`) via `subscriptions.thread.mark`. `[TS]` defaults to `<THREAD_TS>` (or `"0"` when `--unread` is passed without `[TS]`).

JSON output:

```json
{
  "ok": true,
  "channel": "C0123ABC",
  "thread_ts": "1739280000.000100",
  "ts": "1739280456.000300",
  "read": true
}
```

#### pins / pin / unpin

```sh
slackless pins <CHANNEL>
slackless pin <CHANNEL> <TS>
slackless unpin <CHANNEL> <TS>
```

List (`pins.list`), add (`pins.add`), or remove (`pins.remove`) pinned items in a conversation.

JSON output:

```json
{
  "channel": "C0123ABC",
  "count": 1,
  "pins": [
    {
      "type": "message",
      "created": 1739280010,
      "created_by": "U0123ABC",
      "created_by_name": "Ana",
      "ts": "1739280000.000100",
      "user": "U0123ABC",
      "text": "Launch checklist",
      "permalink": "https://acme.slack.com/archives/C0123ABC/p1739280000000100"
    }
  ]
}
```

`pin` / `unpin` JSON output:

```json
{ "ok": true, "channel": "C0123ABC", "ts": "1739280000.000100", "pinned": true }
```

#### schedule / scheduled

```sh
slackless schedule <CHANNEL> <TEXT> --at <WHEN> [--thread-ts TS]
slackless scheduled [--channel CHANNEL]
```

Queue a future message (`chat.scheduleMessage`) or list pending scheduled messages (`chat.scheduledMessages.list`). Pass `-` as `<TEXT>` to read from stdin. `--at` accepts a relative offset (`+90s`, `+15m`, `+2h`, `+1d`, `+2w`) or a future unix epoch timestamp.

JSON output (`schedule`):

```json
{
  "ok": true,
  "scheduled_message_id": "Q0123ABC",
  "channel": "C0123ABC",
  "post_at": 1739283600
}
```

JSON output (`scheduled`):

```json
{
  "count": 1,
  "scheduled_messages": [
    {
      "id": "Q0123ABC",
      "channel": "C0123ABC",
      "post_at": 1739283600,
      "date_created": 1739280000,
      "text": "Reminder: standup"
    }
  ]
}
```

#### saved

```sh
slackless saved list [--filter in_progress|completed|archived] [--limit N]
slackless saved add <CHANNEL> <TS> [--due UNIX_SECS]
slackless saved done <CHANNEL> <TS>
slackless saved delete <CHANNEL> <TS>
```

List (`--filter` default `in_progress`, `--limit` default `50`), add (`saved.add`), complete (`saved.update`), or remove (`saved.delete`) Later / Saved items.

JSON output (`saved list`):

```json
{
  "filter": "in_progress",
  "count": 1,
  "items": [
    {
      "channel": "C0123ABC",
      "ts": "1739280000.000100",
      "state": "inprogress",
      "date_due": 1739366400,
      "text": "Review PR"
    }
  ]
}
```

`add` returns `{ "ok": true, "channel", "ts", "date_due" }`; `done` returns `{ "ok": true, "channel", "ts", "state": "completed" }`; `delete` returns `{ "ok": true, "channel", "ts" }`.

---

### Files, canvases & lists

#### files

```sh
slackless files [--channel CHANNEL] [--types TYPES] [--limit N]
```

List workspace or channel files via `files.list` (e.g. `--types canvas`, `images`, `pdfs`; `--limit` 1..=200, default `50`).

JSON output:

```json
{
  "count": 1,
  "files": [
    {
      "id": "F0123ABC",
      "name": "report.pdf",
      "title": "Q4 Report",
      "filetype": "pdf",
      "size": 12345,
      "user": "U0123ABC",
      "permalink": "https://acme.slack.com/files/..."
    }
  ]
}
```

#### download

```sh
slackless download <FILE_ID> [-o|--out PATH]
```

Download any Slack file by ID (`files.info` + authenticated download). Defaults to `./<filename>` when `--out` is omitted.

JSON output:

```json
{
  "ok": true,
  "file_id": "F0123ABC",
  "name": "report.pdf",
  "bytes": 12345,
  "path": "./report.pdf"
}
```

#### canvas

```sh
slackless canvas read <FILE_ID> [--html]
slackless canvas create <TITLE> [--markdown MD] [--channel CHANNEL]
slackless canvas edit <FILE_ID> <MARKDOWN> [--section SECTION_ID] [--append]
slackless canvas info <CANVAS_ID>
slackless canvas share <CANVAS_ID> --access <read|write|owner> [--channel CHANNEL...] [--user USER_ID...] [--post]
slackless canvas unshare <CANVAS_ID> [--channel CHANNEL...] [--user USER_ID...]
slackless canvas delete <CANVAS_ID>
slackless canvas comments <CANVAS_ID> [--resolved | --all]
slackless canvas comment <CANVAS_ID> <TEXT> [--thread TS]
slackless canvas resolve <CANVAS_ID> [--thread TS | --all] [--reopen]
```

- `read`: fetches markdown via `canvases.getContent` (falling back to HTML export, or forcing HTML with `--html`) -> `{ "file_id", "title", "format", "content" }`.
- `create`: creates a standalone or channel-tabbed canvas (`canvases.create`) -> `{ "ok": true, "canvas_id", "title" }`.
- `edit`: replaces the whole canvas, replaces `--section SECTION_ID`, or appends with `--append` (`canvases.edit`) -> `{ "ok": true, "canvas_id" }`.
- `info`: fetches canvas metadata via `files.info` -> `{ "canvas_id", "title", "permalink", "access", "editable", "org_or_workspace_access", "canvas_printing_enabled", "creator", "editors": [...], "quip_thread_id", "comment_channel_id" }`.
- `share`: grants `read`, `write`, or `owner` access via `canvases.access.set`. `--channel` and `--user` are repeatable and mutually exclusive (XOR). Pass `--post` (channels only) to also post the canvas permalink to each shared channel -> `{ "ok": true, "canvas_id", "access", "channels": [...], "users": [...], "posted": false, "posted_channels": [...], "permalink": null }`.
- `unshare`: revokes access via `canvases.access.delete` (`--channel` and `--user` are repeatable and mutually exclusive) -> `{ "ok": true, "canvas_id", "channels": [...], "users": [...] }`.
- `delete`: permanently deletes a canvas (`canvases.delete`) -> `{ "ok": true, "canvas_id", "deleted": true }`.
- `comments`: lists threaded canvas comments from the canvas's derived comment channel (`C…`). Shows open threads by default, resolved only with `--resolved`, or all with `--all` -> `{ "canvas_id", "comment_channel_id", "filter", "count", "threads": [{ "root_ts", "author", "text", "resolved", "reply_count", "replies": [{ "ts", "author", "text" }] }] }`.
- `comment`: posts a new canvas comment thread or replies to `--thread TS`. Pass `-` as `<TEXT>` to read from stdin -> `{ "ok": true, "canvas_id", "comment_channel_id", "thread_ts", "ts" }`.
- `resolve`: resolves one thread (`--thread TS`, `quip.thread.archive`), reopens one resolved thread (`--thread TS --reopen`, `quip.thread.unarchive`), or resolves all open threads (`--all`, `quip.thread.archiveAll`) -> `{ "ok": true, "canvas_id", "comment_channel_id", "thread_ts", "all", "resolved" }`.

#### list

```sh
slackless list read <LIST_ID> [--format table|csv|json]
slackless list create <NAME> [--description DESC] [--channel CHANNEL]
slackless list add-item <LIST_ID> <TITLE> [--column COL_ID]
slackless list delete-item <LIST_ID> <ROW_ID>
slackless list add-message <LIST_ID> <CHANNEL> <TS>
```

Read, export (`table`, `csv`, or `json`; `--json` forces JSON), create, and mutate Slack Lists:

- `read` JSON: `{ "file_id", "title", "description", "columns": [{ "id", "name", "kind", "is_primary" }], "count", "items": [{ "id", "fields": { ... } }] }`.
- `create` JSON: `{ "ok": true, "list_id", "name" }`.
- `add-item` JSON: `{ "ok": true, "list_id", "row_id", "title" }`.
- `delete-item` JSON: `{ "ok": true, "list_id", "row_id" }`.
- `add-message` JSON: `{ "ok": true, "list_id", "channel", "ts", "record_id" }`.

---

### Live status, unread, watch, backfill & huddles

#### status

```sh
slackless status show
slackless status set <TEXT> [--emoji EMOJI] [--expires-in SECS]
slackless status clear
```

Inspect (`users.profile.get`), set (`users.profile.set`; `--expires-in 0` = no expiry), or clear your Slack custom status.

JSON output:

- `show`: `{ "ok": true, "user", "status_text", "status_emoji", "status_expiration" }`
- `set`: `{ "ok": true, "status_text", "status_emoji", "status_expiration" }`
- `clear`: `{ "ok": true, "cleared": true }`

#### unread

```sh
slackless unread
```

Live unread summary from `client.counts` across DMs, group DMs, channels (suppressing muted conversations with 0 mentions using cached archive prefs), and followed threads.

JSON output:

```json
{
  "ok": true,
  "workspace": "T0123ABC",
  "unread_conversations": 1,
  "total_mentions": 2,
  "threads_unread": true,
  "threads_mentions": 1,
  "items": [
    {
      "id": "C0123ABC",
      "label": "#eng",
      "kind": "channel",
      "mentions": 2,
      "has_unreads": true,
      "latest": "1739280000.000100"
    }
  ]
}
```

#### watch

```sh
slackless watch [--channel CHANNEL] [--count N]
```

Stream live Slack `message`, `reaction_added`, and `thread_marked` events over the realtime websocket. Under `--json`, emits one JSON object per line (NDJSON). Pass `--count N` to exit cleanly after `N` matching events.

NDJSON shapes:

```json
{ "type": "message", "channel": "C0123ABC", "ts": "...", "thread_ts": null, "user": "U0123ABC", "text": "..." }
{ "type": "reaction_added", "channel": "C0123ABC", "ts": "...", "user": "U0123ABC", "reaction": "tada" }
{ "type": "thread_marked", "channel": "C0123ABC", "thread_ts": "...", "last_read": "...", "active": true }
```

#### backfill

```sh
slackless backfill <CHANNEL> [--pages N] [--page-size N]
```

Walk `conversations.history` cursors politely (Tier-3 paced with automatic 429 retry) and upsert messages into the local SQLite archive. Defaults: `--pages 10` (`0` = walk all history to the beginning), `--page-size 200` (Slack max `200`).

JSON output:

```json
{
  "ok": true,
  "workspace": "T0123ABC",
  "channel": "C0123ABC",
  "label": "#eng",
  "pages": 10,
  "messages": 2000,
  "reached_end": false
}
```

#### huddle

```sh
slackless huddle devices
slackless huddle cameras
slackless huddle screens
slackless huddle join <CHANNEL> [--muted] [--duration SECS] [--input NAME] [--output NAME] [--react EMOJI]
slackless huddle link <CHANNEL>
slackless huddle invite <CHANNEL> <USER>
```

Native Amazon Chime huddles from the terminal:

- `devices`, `cameras`, `screens` (offline): enumerate local microphones/speakers (`{ "inputs", "outputs" }`), cameras (`{ "cameras": [{ "index", "name" }] }`), and screen-share monitors/windows (`{ "sources": [{ "id", "name", "monitor", "width", "height" }] }`).
- `join`: connects to Slack `rooms.join` + Chime DTLS-SRTP media until Ctrl+C or `--duration SECS`; under `--json`, streams NDJSON state/data events (`{ "event": "state", "phase", "channel", "call_id", "muted", "packets_sent", "packets_received", "pongs_sent", "error", "udp_blocked" }`).
- `link`: fetches shareable huddle URL (`rooms.getLink`) -> `{ "ok": true, "channel", "url" }`.
- `invite`: rings a workspace member (`rooms.notifyMember`) -> `{ "ok": true, "channel", "user" }`.

---

### Model Context Protocol (MCP) server

#### mcp

```sh
slackless mcp [--workspace TEAM_ID]
```

Runs a lightweight JSON-RPC 2.0 stdio MCP server (`initialize`, `notifications/initialized`, `ping`, `tools/list`, `tools/call`). Pass `--workspace TEAM_ID` to set a default workspace when multiple archives exist.

Exposed tools:

| Tool              | Mode          | Summary                                                                                          |
| ----------------- | ------------- | ------------------------------------------------------------------------------------------------ |
| `list_workspaces` | Offline       | Stored sessions and local SQLite archives with message counts and default workspace              |
| `list_channels`   | Offline       | Channels and/or DMs for a workspace (`kind`: `all`/`channels`/`dms`, optional `filter`)          |
| `search_messages` | Offline       | Local FTS5 search over messages, file names, and audio/video transcripts (single or all-workspace) |
| `read_channel`    | Offline/Live  | Recent or windowed channel/DM messages (`limit`, `since`, `before`, `include_deleted`, `live`)   |
| `read_thread`     | Offline/Live  | Thread root and replies (`channel`, `thread_ts`, optional `live`, `limit`)                       |
| `get_transcript`  | Offline       | Fetch stored audio/video file transcript by `file_id`                                            |
| `send_message`    | Live          | Post a message (`chat.postMessage`) with pre-send secret detection and local archive echo        |
| `reply_thread`    | Live          | Reply in a thread (`chat.postMessage` + `thread_ts`, optional `reply_broadcast`)                 |
| `add_reaction`    | Live          | Add an emoji reaction (`reactions.add`)                                                          |

Example MCP client configuration (Claude Desktop, Cursor, Claude Code):

```json
{
  "mcpServers": {
    "slackless": {
      "command": "slackless",
      "args": ["mcp"]
    }
  }
}
```

---

### Auth & self-update

#### auth

```sh
slackless auth status
slackless auth login [DOMAIN]
slackless auth logout [--workspace TEAM_ID]
```

- `auth status` (offline): lists every stored session and its local archive.
- `auth login [DOMAIN]`: opens Slack's sign-in page (picker, or `DOMAIN.slack.com`) in a window for interactive sign-in, stores credentials, and prints `auth status`.
- `auth logout`: forgets the `--workspace` session (or every stored session without `--workspace`) and prints `auth status`.

JSON output (`auth status`):

```json
{
  "signed_in": true,
  "workspaces": [
    {
      "team_id": "T0123ABC",
      "workspace_url": "https://acme.slack.com",
      "user_id": "U0123ABC",
      "has_secrets": true,
      "archive_path": "/Users/you/Library/Application Support/slackless/T0123ABC.sqlite",
      "archive_exists": true,
      "messages": 48211
    }
  ]
}
```

#### update

```sh
slackless update [--check] [--force] [--restart]
```

Checks GitHub Releases for a newer Slackless release. Without `--check`, downloads and SHA-256-verifies the macOS `.app.tar.gz`, atomically replaces `Slackless.app` (with `.prev` rollback), refreshes the CLI symlink, and optionally relaunches `Slackless.app` (`--restart`).

JSON output:

```json
{
  "ok": true,
  "current": "2026.11.24-02",
  "latest": "2026.11.24-03",
  "update_available": true,
  "updated": false,
  "release_url": "https://github.com/ankarhem/slackless/releases/tag/2026.11.24-03"
}
```

When installed (`updated: true`), also includes `"restarted"`, `"app_path"`, and `"cli_path"`.

---

## The message row

`search`, `read`, `thread`, `threads`, and `export` return the same shape. Fields are always present; optional ones are `null` rather than omitted.

| Field         | Type             | Meaning                                                               |
| ------------- | ---------------- | --------------------------------------------------------------------- |
| `channel`     | string           | channel id the message lives in                                       |
| `ts`          | string           | the message's permanent id; copy it verbatim into `reply` and `react` |
| `user`        | string or null   | author's user id                                                      |
| `user_name`   | string or null   | author's display name, resolved from the archive                      |
| `bot_id`      | string or null   | set for bot authors                                                   |
| `text`        | string           | message text                                                          |
| `thread_ts`   | string or null   | set when the message is a thread reply                                |
| `subtype`     | string or null   | Slack subtype such as `channel_join`                                  |
| `deleted`     | boolean          | tombstone kept by the archive                                         |
| `edited`      | boolean          | message was edited after posting                                      |
| `reply_count` | number or null   | replies in the thread, when known                                     |
| `reactions`   | array            | `{name, count}` aggregates                                            |
| `files`       | array of strings | file names or titles                                                  |
| `permalink`   | string or null   | Slack URL for the message                                             |

## Errors and exit codes

Success exits 0, and so do local archive commands that found nothing to read. Any failure exits 1 and prints one of these shapes:

```json
{ "ok": false, "error": "<code>", "message": "<what to do about it>" }
```

Human mode prints `slackless: <message>` on stderr instead. Error codes:

| Code                    | When it happens                                                                                |
| ----------------------- | ---------------------------------------------------------------------------------------------- |
| `not_signed_in`         | a live command ran with no stored session holding credentials                                  |
| `client_error`          | building the Slack API/RTM client failed                                                       |
| `session_rejected`      | Slack rejected the stored session; ask the user to run `slackless auth login`, then retry      |
| `request_failed`        | the Slack API call failed for another reason                                                   |
| `invalid_args`          | malformed CLI argument (e.g. invalid `--since`, `--at`, canvas flags, or missing upload files) |
| `missing_ts`            | `mark-read` omitted `[TS]` and the local archive has no messages for the channel               |
| `disconnected`          | `watch` websocket disconnected                                                                 |
| `no_download_url`       | `download` target file has no private download URL                                             |
| `write_failed`          | `download` could not write destination file                                                    |
| `no_store_path`         | `backfill` could not resolve the local archive path                                            |
| `update_check_failed`   | `update` could not query GitHub Releases                                                       |
| `update_install_failed` | `update` download, checksum verification, or bundle swap failed                                |
| `command_failed`        | anything else, such as an unexpected I/O or runtime error                                      |

## Worked examples

```sh
# Search across all workspaces or export a single workspace search
slackless search "launch in:#eng from:@ana has:file" --json
slackless search "postmortem after:2026-01-01 before:2026-02-01" --all-workspaces --limit 50 --json

# Read recent messages, a thread, or build a 24h digest
slackless channels --json
slackless read "#eng" --since 24h --limit 20 --json
slackless thread "#eng" "1739280000.000100" --live --json
slackless digest --since 7d --all-workspaces --json

# Post multi-line text via stdin, reply, react, schedule
printf 'Line 1\nLine 2\n' | slackless send "#eng" - --json
slackless reply "#eng" "1739280000.000100" "checklist looks good" --json
slackless react "#eng" "1739280000.000100" tada --json
slackless schedule "#eng" "Standup in 5m" --at "+30m" --json
```

## Notes for agents

- Always pass `--json` and parse the output line(s). Tab-separated mode exists for human eyes.
- Quote arguments in the shell. Text and queries contain spaces, and a leading `#` starts a shell comment.
- Use `-` as `<TEXT>` with `send`, `reply`, `edit`, `schedule`, or `canvas comment` to pass multi-line or shell-tricky bodies via stdin.
- Treat `ts` values as opaque identifiers and copy them verbatim into `reply`, `edit`, `delete`, `react`, `unreact`, `pin`, `unpin`, `thread`, `revisions`, and `mark-read`.
- Empty local results with exit 0 usually mean the archive has not synced yet; check `auth status --json` for `archive_exists` and `messages`, or run `slackless backfill <CHANNEL>`.
- A `session_rejected` error is fixed by the user signing in again (`slackless auth login`), not by retrying.
