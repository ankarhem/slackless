---
name: slackless-cli
description: This skill should be used when an AI agent needs to search, read, send, reply, or react in Slack through the slackless CLI, a headless client that reads a local archive offline and posts through a stored desktop-app session.
---

# slackless CLI

## Overview

`slackless` is the command-line side of the slackless desktop Slack client. It is built for agents and scripts: every command prints a single JSON line under `--json` and tab-separated text otherwise. Nothing needs a live Slack connection to start.

Two kinds of commands exist.

Offline archive commands (`search`, `read`, `channels`, `dms`, `threads`, `revisions`, `export`, `stats`, `vacuum`, `auth status`) work offline against the local SQLite archive, the same file the desktop app fills while it syncs. They always exit 0. With no known workspace they print empty results (plus a stderr hint in human mode) instead of failing.

Live Slack commands (`send`, `reply`, `react`, `mark-thread`, `saved`, `files`, `download`, `canvas`, `list`, `status`, `unread`, `watch`, `huddle`) talk to Slack using a stored session (`slackless auth login` or the desktop app). Every REST command is one deterministic invocation; `watch` opens a live websocket and streams NDJSON events until interrupted (or `--count N`); `huddle join` connects natively to Amazon Chime audio until interrupted or `--duration N`.

## Setup and auth

Sign-in is interactive: `slackless auth login` (or the desktop app's first launch) opens Slack's own sign-in page in a window, and a human completes it (email code, SSO, 2FA). Do not run it unattended; ask the user to sign in when `auth status` reports `signed_in: false`. The CLI and the desktop app share the stored session. macOS may ask once to let the `slackless` binary read the Keychain item.

- Credentials live in the platform secret store, one session per team id.
- The archive lives at `~/Library/Application Support/slackless/<TEAM_ID>.sqlite` on macOS and `~/.local/share/slackless/<TEAM_ID>.sqlite` on Linux. It fills up as the desktop app syncs, so a fresh workspace reads as empty.
- Start with `slackless auth status --json`. It lists every stored session with its team id, the signed-in user, whether secrets are present, and the archive path with a message count. The `team_id` values are what you pass to `--workspace`.

### Choosing a workspace

`--workspace TEAM_ID` is a global flag; it works on every command. When it is omitted:

- Local commands fall back to the first stored session, then to the single archive on disk. When nothing is known, or several archives exist and no session disambiguates them, local commands return empty results rather than guess.
- Write commands use the stored session of the flagged team id, or the first stored session. That session must have credentials, or the command fails with a `not_signed_in` error.

## Commands

### search

```
slackless search <QUERY> [--limit N]
```

Full-text search of the local archive. The query is one quoted string with Slack-style modifiers:

- `in:#eng`, `in:eng`, or `in:C0123ABC` restricts to one channel; repeat it for alternatives, which OR together.
- `from:@ana` or `from:U0123ABC` restricts to an author; repeats OR together too.
- `has:link`, `has:file`, `has:reaction`, `has:pin` require that element.
- `before:2026-01-31` and `after:2026-01-01` are UTC day bounds, both exclusive.
- Everything else is free text, and a quoted phrase stays one term. Different modifiers combine with AND. An unknown `foo:bar` token is treated as free text, so a typo degrades the query instead of erroring.

`--limit N` caps results; the default is 20.

JSON output:

```json
{ "query": "launch in:#eng", "count": 1, "messages": ["<MessageRow>"] }
```

Human output is one line per message: `channel<TAB>ts<TAB>author: text`.

### read

```
slackless read <CHANNEL> [--limit N]
```

Print recent messages from a conversation. `<CHANNEL>` accepts an id (`C0123ABC`, `D0123ABC`, `G0123ABC`), a `#name` or bare name (case-insensitive), or an `@handle` for the DM with that user. When the archive cannot resolve the argument, everything after a leading `#` is used as a raw channel id.

`--limit N` caps results; the default is 50. You get the newest page, printed oldest first, so it reads top-down like Slack. Messages that belong to a thread carry a `thread_ts`; filter on it when following a discussion.

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

`channel.name`, `channel.kind`, and `channel.user` come back null when the archive does not know the conversation.

Human output is one line per message: `channel<TAB>ts<TAB>author: text`.

### send

```
slackless send <CHANNEL> <TEXT>
```

Post a message. `<CHANNEL>` resolves like in `read`, and `<TEXT>` is a single argument, so quote it. This needs a stored session. On success the sent message is also recorded in the local archive, so a later `read` shows it offline.

JSON output:

```json
{ "ok": true, "channel": "C0123ABC", "ts": "1739280000.000100" }
```

### reply

```
slackless reply <CHANNEL> <THREAD_TS> <TEXT>
```

Post into a thread. `<THREAD_TS>` is the ts of the thread root, exactly as `read` or `search` reported it. `<TEXT>` is a single quoted argument. Needs a stored session, like `send`.

JSON output:

```json
{
  "ok": true,
  "channel": "C0123ABC",
  "thread_ts": "1739280000.000100",
  "ts": "1739280123.000200"
}
```

### react

```
slackless react <CHANNEL> <TS> <EMOJI>
```

Add a reaction to one message. `<TS>` is the message ts from `read`. `<EMOJI>` is a shortcode with or without colons: `tada` and `:tada:` both work. Needs a stored session.

JSON output:

```json
{ "ok": true, "channel": "C0123ABC", "ts": "1739280000.000100", "name": "tada" }
```

### channels

```
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

Human output is `id<TAB>#name (flags)<TAB>kind<TAB>topic`, with `(archived)` and `(not a member)` flags when they apply.

### dms

```
slackless dms
```

List DMs and group DMs known to the archive, most recently active first.

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

`user` is the peer's user id, `user_name` its resolved display name when the archive has one. For a group DM (`kind: "mpim"`), `name` holds the group's name and `user` is null.

Human output is `id<TAB>peer<TAB>user_id<TAB>kind`.

### stats / vacuum / threads / revisions / export (offline archive)

```
slackless stats [--breakdown]
slackless vacuum
slackless threads [--channel CHANNEL] [--limit N]
slackless revisions <CHANNEL> <TS>
slackless export <CHANNEL> [--format md|json] [--limit N] [--out PATH]
```

- `stats` prints SQLite database size, message/revision/conversation/user counts, offline queue count, and (with `--breakdown`) per-table counts and the top 10 largest channels.
- `vacuum` checkpoints WAL, compacts pages (`VACUUM`), and optimizes FTS5 indexes.
- `threads` lists locally archived thread roots that have replies, newest first.
- `revisions` prints the immutable append-only edit & deletion history recorded for one message.
- `export` exports a conversation's archived messages as Markdown or JSON (stdout or `--out PATH`).

### mark-thread

```
slackless mark-thread <CHANNEL> <THREAD_TS> [TS] [--unread]
```

Mark a followed thread read (`read: true`) or unread (`--unread`, `read: false`) via `subscriptions.thread.mark`. `[TS]` defaults to `<THREAD_TS>`.

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

### saved

```
slackless saved list [--filter in_progress|completed|archived] [--limit N]
slackless saved add <CHANNEL> <TS> [--due UNIX_SECS]
slackless saved done <CHANNEL> <TS>
slackless saved delete <CHANNEL> <TS>
```

List, create, complete, or remove Later / Saved items.

### files

```
slackless files [--channel CHANNEL] [--types TYPES] [--limit N]
```

List workspace or channel files via `files.list` (for example `--types canvas` for canvases).

### canvas

```
slackless canvas read <FILE_ID> [--html]
slackless canvas create <TITLE> [--markdown MD] [--channel CHANNEL]
slackless canvas edit <FILE_ID> <MARKDOWN> [--section SECTION_ID] [--append]
```

- `read` fetches markdown via `canvases.getContent` (falling back to HTML export, or forcing HTML with `--html`).
- `create` creates a standalone or channel-tabbed canvas via `canvases.create`.
- `edit` updates a canvas via `canvases.edit` (replaces the whole document, replaces `--section SECTION_ID`, or appends with `--append`).

### list

```
slackless list read <LIST_ID> [--format table|csv|json]
slackless list create <NAME> [--description DESC] [--channel CHANNEL]
slackless list add-item <LIST_ID> <TITLE> [--column COL_ID]
slackless list delete-item <LIST_ID> <ROW_ID>
slackless list add-message <LIST_ID> <CHANNEL> <TS>
```

Read, export, create, and mutate Slack Lists (`fetch_list`, `lists.create`, `lists.records.create`, `lists.records.delete`, `lists.addToList`).

### watch

```
slackless watch [--channel CHANNEL] [--count N]
```

Stream live Slack `message`, `reaction_added`, and `thread_marked` events over the realtime websocket. Under `--json`, emits one JSON object per line (NDJSON). Pass `--count N` to exit cleanly after `N` matching events.

### auth status

```
slackless auth status
```

Show every stored session and its local archive. Runs offline; only the global flags apply.

JSON output:

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

`signed_in` is true when at least one session has credentials. Human output prints `signed in` (or why not), then one line per workspace plus its archive path and message count.

### auth login / auth logout

```
slackless auth login [DOMAIN]
slackless auth logout [--workspace TEAM_ID]
```

`login` opens Slack's sign-in page (the workspace picker, or `DOMAIN.slack.com`) in a window and stores every workspace signed in there; it needs a human at the screen and exits non-zero if the window is closed first. `logout` forgets the `--workspace` session, or every stored session without the flag. Both then print the `auth status` document.

## The message row

`search` and `read` return the same shape. Fields are always present; optional ones are `null` rather than omitted.

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

Success exits 0, and so do local commands that found nothing to read. Any failure exits 1 and prints one of these shapes:

```json
{ "ok": false, "error": "<code>", "message": "<what to do about it>" }
```

Human mode prints `slackless: <message>` on stderr instead. The codes:

| Code               | When it happens                                                                           |
| ------------------ | ----------------------------------------------------------------------------------------- |
| `not_signed_in`    | a write command ran with no stored session holding credentials                            |
| `client_error`     | building the Slack client failed                                                          |
| `session_rejected` | Slack rejected the stored session; ask the user to run `slackless auth login`, then retry |
| `request_failed`   | the API call failed for another reason                                                    |
| `command_failed`   | anything else, such as a startup problem                                                  |

## Worked examples

### Search the archive

```sh
slackless search "launch in:#eng from:@ana has:file" --json
```

Sample output, trimmed:

```json
{
  "query": "launch in:#eng from:@ana has:file",
  "count": 1,
  "messages": [
    {
      "channel": "C0123ABC",
      "ts": "1739280000.000100",
      "user": "U0123ABC",
      "user_name": "ana",
      "bot_id": null,
      "text": "launch checklist: docs/launch.md",
      "thread_ts": null,
      "subtype": null,
      "deleted": false,
      "edited": false,
      "reply_count": 3,
      "reactions": [{ "name": "tada", "count": 4 }],
      "files": ["launch.md"],
      "permalink": "https://acme.slack.com/archives/C0123ABC/p1739280000000100"
    }
  ]
}
```

Narrow by date:

```sh
slackless search "postmortem after:2026-01-01 before:2026-02-01" --limit 50 --json
```

### Read a conversation

Find the channel, then read it:

```sh
slackless channels --json
slackless read "#eng" --limit 20 --json
```

DMs work the same way through `dms` and an `@handle`:

```sh
slackless dms --json
slackless read "@ana" --json
```

The example message above has `reply_count` 3. To follow that thread, read the channel and pick the messages whose `thread_ts` equals the root's `ts`.

### Send, reply, react

```sh
slackless send "#eng" "deploy 2026.11.19 is green"
# {"ok":true,"channel":"C0123ABC","ts":"1739280456.000300"}

slackless reply "#eng" "1739280000.000100" "checklist looks good, shipping it"
# {"ok":true,"channel":"C0123ABC","thread_ts":"1739280000.000100","ts":"1739280457.000400"}

slackless react "#eng" "1739280000.000100" tada
# {"ok":true,"channel":"C0123ABC","ts":"1739280000.000100","name":"tada"}
```

With no stored session, each of these fails the same way:

```json
{
  "ok": false,
  "error": "not_signed_in",
  "message": "not signed in: no stored session with credentials; run `slackless auth login`"
}
```

## Notes for agents

- Always pass `--json` and parse the single output line. The tab-separated mode exists for human eyes.
- Quote arguments in the shell. Text and queries contain spaces, and a leading `#` starts a shell comment.
- Treat `ts` values as opaque identifiers and copy them verbatim into `reply` and `react`.
- Empty local results with exit 0 usually mean the archive has not synced yet; check `auth status` for `archive_exists` and `messages`.
- A `session_rejected` error is fixed by the user signing in again (`slackless auth login`), not by retrying.

### huddle

```sh
slackless huddle devices --json
slackless huddle cameras --json
slackless huddle screens --json
slackless huddle join "#eng" --muted --duration 15 --json
slackless huddle link "#eng" --json
slackless huddle invite "#eng" U0123ABC --json
```

Native Amazon Chime huddles from the terminal: `devices`, `cameras`, and `screens` run offline to enumerate local microphones/speakers, cameras, and screen-share monitors/windows; `join` connects to Slack's `rooms.join` + Chime DTLS-SRTP media stack until Ctrl+C or `--duration N`; `link` fetches the shareable huddle URL; `invite` rings a member (`rooms.notifyMember`).

### status

```sh
slackless status show --json
slackless status set "In a meeting" --emoji spiral_calendar_pad --expires-in 3600 --json
slackless status clear --json
```

Inspect, set, or clear your Slack custom status (`users.profile.get` / `users.profile.set`).

### unread

```sh
slackless unread --json
```

Live unread summary from `client.counts` across channels, DMs, group DMs, and followed threads (`unread_conversations`, `total_mentions`, `threads_unread`, `items`).

### download

```sh
slackless download F0123ABC --out ./report.pdf --json
```

Downloads any Slack file by ID via `files.info` and the authenticated session cookie.

### watch / backfill / stats / vacuum / revisions / export

```sh
slackless watch --channel "#eng" --count 5 --json
slackless backfill "#eng" --pages 10 --json
slackless stats --breakdown --json
slackless vacuum --json
slackless revisions "#eng" "1739280000.000100" --json
slackless export "#eng" --format md --limit 200 --out eng.md
```

- `watch` streams live RTM events over `wss-primary`.
- `backfill` walks `conversations.history` cursors politely (Tier-3 paced, `--pages 0` walks all history) into the local SQLite archive.
- `stats` and `vacuum` inspect and compact the local SQLite archive (compressing legacy message JSON rows and reclaiming unused pages).
- `revisions` prints the append-only edit/deletion history of one message.
- `export` writes archived messages to Markdown or JSON offline.
