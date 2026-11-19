# Conversations history and messaging - Slack web-client API

## Summary

Covers reading conversation content (`conversations.view/history/replies/info`, `conversations.genericInfo`, `conversations.historyChanges`, `messages.list`), writing it (`chat.postMessage/update/delete/meMessage`, scheduled messages, `drafts.*`), and link unfurls (`chat.unfurl`, `chat.attachmentAction`), plus the message object and `rich_text` schema both directions. Roughly half of this area is public API (`chat.*`, `conversations.history/replies/info`); the web client adds private methods (`conversations.view`, `conversations.genericInfo`, `conversations.historyChanges`, `drafts.*`, `messages.list`) called on the same `POST https://<ws>.slack.com/api/<method>` surface with `token` + `Cookie: d=` auth. The biggest risks: `conversations.view`'s response is versioned, undocumented and carries inline history; `drafts.*` bodies double-encode JSON inside JSON; the web client sends every user message as `rich_text` blocks with a client-generated `client_msg_id` (dedup relies on it), and ts values are padded/truncated inconsistently (drafts require exactly 7 decimal places). Websocket `message` events with subtypes (`message_changed`, `message_deleted`, `message_replied`) are the only push channel for edits/deletes/unreads.

## Endpoints

Cross-area methods seen in this area but owned by sibling docs: `conversations.list` (channel discovery - OBSERVED: github.com/dylex/slack-libpurple/blob/master/slack-conversation.c:19), `client.counts` (unread/read-state snapshots - OBSERVED: github.com/rusq/slackdump/blob/master/internal/edge/client.go), `conversations.listPrefs`, `bookmarks.list` (OBSERVED: .omo/research/findings.md). Not covered here.

### conversations.view

Transport: `POST https://<workspace>.slack.com/api/conversations.view`, form-encoded (slackdump `PostForm`); also works JSON-bodied via the session-auth path (karbassi sends `{channel}` as JSON).
OBSERVED: github.com/rusq/slackdump/blob/master/internal/edge/conversations.go
OBSERVED: github.com/karbassi/slack-mcp/blob/master/src/slack_mcp/tools/undocumented.py (`conversations_view` -> `session_call("conversations.view", channel=...)`, JSON body)
OBSERVED: github.com/karbassi/slack-mcp/blob/master/src/slack_mcp/client.py:130 (session_call posts JSON to `https://slack.com/api/<method>` with xoxc + `Cookie: d=`)

Visibility: private (web client only; NOT in public docs - docs.slack.dev/reference/methods/conversations.view returns 404).

Request params (form, all optional except `channel`): OBSERVED: github.com/rusq/slackdump/blob/master/internal/edge/conversations.go (`conversationsViewForm`)

- `channel` - string - required - conversation ID to hydrate
- `count` - int - optional - number of history messages to return inline (slackdump sends 50)
- `canonical_avatars` - bool - optional - avatar normalization (slackdump sends true)
- `no_user_profile` - bool - optional - omit user profile objects (slackdump sends true)
- `ignore_replies` - bool - optional - exclude thread replies from inline history
- `no_self` - bool - optional - omit own messages (slackdump sends true)
- `include_full_users` - bool - optional - full user objects in `users`
- `include_use_cases`, `include_stories`, `no_members`, `include_mutation_timestamps`, `include_free_team_extra_messages` - bool - optional
- `token` - string - xoxc token (injected into form by slackdump `PostFormRaw`, OBSERVED: github.com/rusq/slackdump/blob/master/internal/edge/edge.go:247)

Response: channel object plus inline history and user/emoji side loads:

```json
{
  "ok": true,
  "channel": { "id": "C...", "name": "...", "topic": {...}, "purpose": {...}, "is_member": true, "latest": {...} },
  "messages": [ { "type": "message", "ts": "...", ... } ],
  "users": [ {...} ],
  "im": {...},
  "emojis": { "name": "url" }
}
```

OBSERVED: github.com/rusq/slackdump/blob/master/internal/edge/conversations.go (`ConversationsViewResponse` keys `users`/`im`/`emojis`; comment "we don't care about the rest")
OBSERVED: .omo/research/findings.md ("conversations.view - Returned the selected channel object plus initial visible history for the URL's channel id")

Pagination: none observed; `count` caps the inline history slice.

Used for: route hydration when opening `https://app.slack.com/client/<team>/<channel>` - the web client's first call for a channel, returns channel metadata + first page of history in one round trip (OBSERVED: .omo/research/findings.md). slackless can use it for fast channel open; still needs `conversations.history` for deep scrolling.

OBSERVED: live capture shows `conversations.view` fires during boot phase 1 with `client.userBoot` (OBSERVED: .omo/research/gist.md).

### conversations.history

Transport: `POST https://<workspace>.slack.com/api/conversations.history` (form-encoded; public docs show GET also allowed, "GET https://slack.com/api/conversations.history").
OBSERVED: https://docs.slack.dev/reference/methods/conversations.history
OBSERVED: github.com/wee-slack/wee-slack/blob/main/slack/slack_api.py:205 (`_fetch` posts urlencoded form to `https://api.slack.com/api/<method>` with `Authorization: Bearer <token>` + cookie header)
OBSERVED: github.com/dylex/slack-libpurple/blob/master/slack-conversation.c:328 (`slack_api_post(..., "conversations.history", "channel", id, "oldest", since ?: "0", "limit", count_buf, NULL)`)

Visibility: public (scopes `channels:history`/`groups:history`/`im:history`/`mpim:history`; rate limit Tier 3). OBSERVED: https://docs.slack.dev/reference/methods/conversations.history

Request params:

- `token` - string - required - xoxc token in body for session auth (OBSERVED: github.com/rusq/slackdump/blob/master/internal/edge/edge.go:247)
- `channel` - string - required - conversation ID (OBSERVED: docs.slack.dev/reference/methods/conversations.history)
- `oldest` - string - optional - only messages after this ts (default `0`) (OBSERVED: docs + slack-conversation.c:328)
- `latest` - string - optional - only messages before this ts (OBSERVED: docs + github.com/dylex/slack-libpurple/blob/master/slack-thread.c:254)
- `inclusive` - bool - optional - include boundary ts in results (OBSERVED: docs + slack-thread.c:254 `"inclusive", "1"`)
- `limit` - number - optional - max items, max 999, default 100 (OBSERVED: docs.slack.dev/reference/methods/conversations.history)
- `cursor` - string - optional - `next_cursor` from previous page (OBSERVED: docs)
- `include_all_metadata` - bool - optional - full per-message metadata (OBSERVED: docs)

Response:

```json
{
  "ok": true,
  "messages": [
    {
      "type": "message",
      "user": "U...",
      "text": "...",
      "ts": "1512085950.000216"
    }
  ],
  "has_more": true,
  "pin_count": 0,
  "channel_actions_ts": null,
  "channel_actions_count": 0,
  "is_limited": false,
  "response_metadata": { "next_cursor": "bmV4dF90czoxNTEyMDg1ODYxMDAwNTQz" }
}
```

OBSERVED: https://docs.slack.dev/reference/methods/conversations.history (messages/has_more/pin_count/response_metadata)
OBSERVED: github.com/wee-slack/wee-slack/blob/main/typings/slack_api/slack_conversations_history.pyi (`SlackConversationsHistorySuccessResponse`: `ok/messages/has_more/pin_count/channel_actions_ts/channel_actions_count`, `is_limited` NotRequired)

Pagination: cursor-based via `response_metadata.next_cursor`; pass as `cursor`. In the web client, older pages are loaded with `oldest=<lowest seen ts>` (OBSERVED: .omo/research/gist.md - "conversations.history API with `oldest` timestamp for loading message history").

Used for: main message list; incremental catch-up after reconnect (oldest=last seen); slackless scrolling.

OBSERVED: .omo/research/findings.md (fresh-load fires `conversations.history` for the URL channel; sidebar click fires only history + listPrefs + bookmarks.list; threads NOT fetched on plain channel load).

### conversations.replies

Transport: `POST https://<workspace>.slack.com/api/conversations.replies`, form-encoded.
OBSERVED: github.com/wee-slack/wee-slack/blob/main/slack/slack_api.py:231 (`params = {"channel": ..., "ts": parent_message_ts}` then `_fetch_list(method, "messages", params)`)
OBSERVED: github.com/dylex/slack-libpurple/blob/master/slack-conversation.c:326 (`"conversations.replies", "channel", id, "oldest", since ?: "0", "limit", count_buf, "ts", thread_ts`)
OBSERVED: https://docs.slack.dev/reference/methods/conversations.replies

Visibility: public (same `*:history` scopes; Tier 3). OBSERVED: https://docs.slack.dev/reference/methods/conversations.replies

Request params:

- `channel` - string - required - conversation ID
- `ts` - string - required - parent message ts (thread root)
- `oldest` - string - optional - fetch replies after ts (OBSERVED: slack-conversation.c:326)
- `latest` - string - optional (OBSERVED: docs.slack.dev/reference/methods/conversations.replies)
- `inclusive`, `limit` (max 999), `cursor` - as history (OBSERVED: docs)

Response: `messages` mixes the thread parent (full thread fields) and reply messages:

```json
{
  "ok": true,
  "messages": [
    {
      "type": "message",
      "user": "U...",
      "text": "root",
      "ts": "1700000000.000100",
      "thread_ts": "1700000000.000100",
      "reply_count": 3,
      "reply_users_count": 2,
      "latest_reply": "1700000100.000200",
      "reply_users": ["U1", "U2"],
      "subscribed": false,
      "is_locked": false
    },
    {
      "type": "message",
      "user": "U1",
      "text": "reply",
      "ts": "1700000050.000100",
      "thread_ts": "1700000000.000100",
      "parent_user_id": "U..."
    }
  ],
  "has_more": true,
  "response_metadata": { "next_cursor": "..." }
}
```

OBSERVED: github.com/wee-slack/wee-slack/blob/main/typings/slack_api/slack_conversations_replies.pyi (`SlackMessageThread`: `thread_ts`, `parent_user_id`; `SlackConversationsRepliesSuccessResponse`: `messages/has_more/response_metadata`; parent variants with `reply_count/reply_users_count/latest_reply/reply_users/is_locked/subscribed`, subscribed variant adds `last_read`)
OBSERVED: https://docs.slack.dev/reference/methods/conversations.replies

Pagination: cursor; `has_more` + `response_metadata.next_cursor`.

Used for: thread pane (opened lazily only when the thread view opens in the web client - OBSERVED: .omo/research/findings.md "Thread replies are fetched lazily when the thread pane is opened").

### conversations.info

Transport: `POST https://<workspace>.slack.com/api/conversations.info`, form-encoded.
OBSERVED: github.com/wee-slack/wee-slack/blob/main/slack/slack_api.py:244 (`params = {"channel": conversation_id}`)
OBSERVED: github.com/dylex/slack-libpurple/blob/master/slack-conversation.c:161 (`slack_api_post(sa, conversation_retrieve_cb, lookup, "conversations.info", "channel", sid, NULL)`)

Visibility: public (`channels:read`/`groups:read`/`im:read`/`mpim:read`; Tier 3). OBSERVED: https://docs.slack.dev/reference/methods/conversations.info

Request params:

- `channel` - string - required - conversation ID
- `include_locale` - bool - optional - include `locale` (OBSERVED: docs)

Response:

```json
{ "ok": true, "channel": {
  "id": "C...", "created": 1447749809, "is_archived": false, "is_org_shared": false,
  "context_team_id": "T...", "updated": 1688123456789000, "name": "general", "is_channel": true,
  "is_private": false, "is_general": true, "is_member": true, "is_mpim": false, "is_im": false,
  "creator": "U...", "unlinked": 0, "name_normalized": "general",
  "topic": { "value": "...", "creator": "U...", "last_set": 1447749849 },
  "purpose": { "value": "...", "creator": "U...", "last_set": 1447749849 },
  "previous_names": [], "shared_team_ids": ["T..."], "pending_shared": [],
  "properties": { "canvas": {...}, "posting_restricted_to": {...}, "tabs": [...] }
} }
```

OBSERVED: https://docs.slack.dev/reference/methods/conversations.info (channel object incl. topic/purpose; `properties.tabs` empty-set omission note)
OBSERVED: github.com/beeper/slackgo/blob/beeper/conversation.go:14 (`Conversation` struct: `id, created, is_open, last_read, latest, unread_count, unread_count_display, is_shared, is_im, is_ext_shared, is_org_shared, is_private, is_mpim, name_normalized, num_members, priority, user, connected_team_ids, context_team_id, conversation_host_id, previous_names, pending_shared`)
OBSERVED: github.com/beeper/slackgo/blob/beeper/conversation.go (`Properties`: `canvas{file_id,is_empty,quip_thread_id}`, `posting_restricted_to`, `tabs[{id,label,type}]`, `record_channel`, `has_custom_mpdm_name`)
OBSERVED: github.com/wee-slack/wee-slack/blob/main/typings/slack_api/slack_conversations_info.pyi (per-conversation-type field sets: IM adds `user`, `latest`, `unread_count`, `unread_count_display`, `is_open`, `priority`; public adds `previous_names`, `pending_connected_team_ids`)

Pagination: none.

Used for: channel metadata when hydrating a conversation not in boot cache; DM `latest`/unread counts (fields IM-only per docs and wee-slack typing).

OBSERVED: emacs-slack uses the same endpoint URL `https://slack.com/api/conversations.info` (OBSERVED: github.com/emacs-slack/emacs-slack/blob/master/slack-conversations.el:65).

### conversations.genericInfo

Transport: `POST https://<workspace>.slack.com/api/conversations.genericInfo`, form-encoded; batch with conditional fetching.
OBSERVED: github.com/rusq/slackdump/blob/master/internal/edge/conversations.go (`ConversationsGenericInfo`)

Visibility: private (web client).

Request params:

- `updated_channels` - JSON string - map of channel id -> last-known updated ts, e.g. `{"C065H568ZAT":0}` (OBSERVED: slackdump `conversationsGenericInfoForm`, builds `json.Marshal(map[string]int)`)
- `token` - string - xoxc (injected)

Response:

```json
{ "ok": true, "channels": [ {...} ], "unchanged_channel_ids": ["C065H568ZAT"] }
```

OBSERVED: github.com/rusq/slackdump/blob/master/internal/edge/conversations.go (`conversationsGenericInfoResponse`)

Pagination: none.

Used for: conditional refresh of several channels' metadata in one call (the `updated_*` pattern matches Flannel's conditional fetching; OBSERVED: .omo/research/gist.md - "Edge API uses conditional fetching... `updated_ids` with timestamps").

### conversations.historyChanges

Transport: `POST https://<workspace>.slack.com/api/conversations.historyChanges` (fired by the live web client at boot).
OBSERVED: .omo/research/findings.md (fresh-load capture lists `conversations.historyChanges` among bootstrap calls; same doc published as github.com/ethanyc216/personal-slack-agent/blob/main/docs/slack-client-findings.md)

Visibility: private (web client only).

Request params: not captured in any mined source - exact param/response shape is unknown (see LEADS).

Used for: incremental history invalidation after boot; the web client calls it during bootstrap (OBSERVED: .omo/research/findings.md). `client.counts` exposes a companion `history_invalid` flag per channel (OBSERVED: .omo/research/findings.md - items include `history_invalid`; and github.com/rusq/slackdump/blob/master/internal/edge/client.go `ChannelSnapshot.HistoryInvalid`).

### messages.list

Transport: `POST https://<workspace>.slack.com/api/messages.list`, JSON body (`session_call`).
OBSERVED: github.com/karbassi/slack-mcp/blob/master/src/slack_mcp/tools/undocumented.py (`messages_list` -> `session_call("messages.list", message_ids=...)`)
OBSERVED: github.com/karbassi/slack-mcp/blob/master/src/slack_mcp/client.py:130 (JSON POST to `https://slack.com/api/<method>`)

Visibility: private.

Request params:

- `message_ids` - array of `{ "channel": "C0123", "timestamps": ["1700000000.000100", ...] }` - required - batch of messages by channel+ts

Response: full message objects for top-level messages and thread replies (compacted in the MCP wrapper; full shape is the standard message object below).
OBSERVED: github.com/karbassi/slack-mcp/blob/master/src/slack_mcp/tools/undocumented.py (`compact_messages_by_channel`; docstring: "Resolves both top-level messages and thread replies in one call")

Used for: hydrating permalinks/bookmarks/saved items without paging history.

### chat.postMessage

Transport: `POST https://<workspace>.slack.com/api/chat.postMessage`, form-encoded (`application/x-www-form-urlencoded`) with JSON-string fields; the web client sends multipart/form-data.
OBSERVED: https://docs.slack.dev/reference/methods/chat.postMessage (`POST https://slack.com/api/chat.postMessage`, content type `application/x-www-form-urlencoded`)
OBSERVED: .omo/research/gist.md ("Messages are sent via the Workspace API as multipart form-data")
OBSERVED: github.com/beeper/slackgo/blob/beeper/chat.go:340 (`chatPostMessage sendMode = "chat.postMessage"`; formSender sets `token` + `channel` values and JSON-encodes `blocks`/`attachments` into the form)

Visibility: public (`chat:write`, docs frontmatter `rate_limit: "t5"`).

Web-client wire format (what slackless must reproduce) - form fields:

- `channel` - string - required - `C...`/`D...` id
- `type` - string - `"message"` - sent by web client
- `ts` - string - client-generated optimistic timestamp `1772325870.xxxxx2`
- `client_msg_id` - UUID - client-generated, server-side dedup on retry
- `blocks` - JSON string - `[{"type":"rich_text","elements":[{"type":"rich_text_section","elements":[{"type":"text","text":"..."}]}]}]`
- `unfurl` - `[]` - empty array; unfurling is server-side
- `_x_reason` - `"webapp_message_send"`; `_x_mode` - `"online"`
  OBSERVED: .omo/research/gist.md ("Sending a Message (chat.postMessage)" section, exact field list quoted there; "Client generates an optimistic timestamp"; "client_msg_id UUID enables deduplication"; "All unfurling is entirely server-side... chat.postMessage includes an `unfurl: []` field")

Request params (public method surface):

- `token` - string - required - xoxc token in form body
- `channel` - string - required - conversation ID or channel name
- `text` - string - fallback text (required when no blocks; OBSERVED: docs "How this field works... depends on other fields")
- `blocks` - JSON string - Block Kit blocks; web client always sends one `rich_text` block (OBSERVED: .omo/research/gist.md "Even a plain text message like \"hello\" is encoded as rich_text > rich_text_section > text")
- `attachments` - JSON string - legacy structured attachments (OBSERVED: docs)
- `thread_ts` - string - reply target parent ts (OBSERVED: github.com/beeper/slackgo/blob/beeper/chat.go `PostMessageParameters.ThreadTimestamp`; docs)
- `reply_broadcast` - bool - also notify channel for a thread reply (OBSERVED: docs; wee-slack sets it with thread_ts, github.com/wee-slack/wee-slack/blob/main/slack/slack_api.py:484)
- `as_user` - bool - legacy (classic apps); wee-slack sends `as_user: true` with user tokens (OBSERVED: docs; slack_api.py:491)
- `link_names` - bool - link user groups (wee-slack sends `link_names: true`, OBSERVED: slack_api.py:491; docs note "No longer supports linking individual users")
- `parse` / `mrkdwn` / `unfurl_links` / `unfurl_media` / `icon_url` / `icon_emoji` / `username` / `markdown_text` / `metadata` - optional (OBSERVED: docs.slack.dev/reference/methods/chat.postMessage Arguments); `file_ids` - optional (OBSERVED: github.com/beeper/slackgo/blob/beeper/chat.go:56 `PostMessageParameters.FileIDs`, json `file_ids`; NOT in public docs Arguments)
- `current_draft_last_updated_ts` - string - draft sync token when sending a message that was previously a draft (OBSERVED: docs.slack.dev/reference/methods/chat.postMessage - documented parameter!)
- `client_msg_id` - client-side dedup id, sent by the web client (OBSERVED: .omo/research/gist.md; NOT in public docs)
- `_x_reason`, `_x_mode` - telemetry/replay-mode fields (OBSERVED: .omo/research/gist.md "`_x_mode` field reveals offline capability")

Response:

```json
{ "ok": true, "channel": "C...", "ts": "1772325870.309319",
  "message": { "type": "message", "text": "...", "user": "U...", "ts": "1772325870.309319", "blocks": [...] } }
```

OBSERVED: github.com/beeper/slackgo/blob/beeper/chat.go:31 (`chatResponseFull`: `channel`, `ts`, `message_ts` for ephemeral, `scheduled_message_id`, `message *Message`)
OBSERVED: .omo/research/gist.md ("The response returns the server-assigned timestamp (ts: \"1772325870.309319\") which becomes the message's permanent ID")

Pagination: none.

Used for: sending every user message in slackless; reply in thread via `thread_ts`; broadcast via `reply_broadcast`.

OBSERVED: emacs-slack posts the whole payload as JSON body `Content-Type: application/json;charset=utf-8` to `https://slack.com/api/chat.postMessage` (OBSERVED: github.com/emacs-slack/emacs-slack/blob/master/slack-message-sender.el:133-141) - both encodings are accepted.

### chat.update

Transport: `POST https://<workspace>.slack.com/api/chat.update`, form-encoded.
OBSERVED: https://docs.slack.dev/reference/methods/chat.update
OBSERVED: github.com/beeper/slackgo/blob/beeper/chat.go:339 (`chatUpdate sendMode = "chat.update"`; MsgOptionUpdate adds `ts`, chat.go:501)

Visibility: public (`chat:write`, Tier 3). OBSERVED: docs frontmatter.

Request params:

- `channel` - string - required - conversation containing the message
- `ts` - string - required - message ts to update
- `text` - string - optional/new text
- `blocks` - JSON string - replacement blocks (OBSERVED: docs)
- `attachments` - JSON string - replacement attachments (OBSERVED: docs)
- `unfurled_attachments` - JSON string (OBSERVED: docs.slack.dev/reference/methods/chat.update)
- `link_names` - bool - "If you do not specify a value ... overwritten with the default, none" (OBSERVED: docs)
- `parse` - defaults to `client` here, unlike postMessage (OBSERVED: docs.slack.dev/reference/methods/chat.update)
- `file_ids` - array - new file ids sent with the message (OBSERVED: docs)
- `metadata`, `reply_broadcast`, `as_user`, `markdown_text` - optional (OBSERVED: docs)
- `as_user: true` + `link_names: true` is what wee-slack sends (OBSERVED: github.com/wee-slack/wee-slack/blob/main/slack/slack_api.py:512)

Response: `{ "ok": true, "channel": "C...", "ts": "...", "message": {...} }` (OBSERVED: github.com/beeper/slackgo/blob/beeper/chat.go:31 chatResponseFull)

Used for: message editing; server pushes the edit to all clients as `message_changed` (see Websocket events).

### chat.delete

Transport: `POST https://<workspace>.slack.com/api/chat.delete`, form-encoded.
OBSERVED: https://docs.slack.dev/reference/methods/chat.delete
OBSERVED: github.com/beeper/slackgo/blob/beeper/chat.go:342 (`chatDelete`; MsgOptionDelete adds `ts`, chat.go:510)

Visibility: public (`chat:write`, Tier 3). OBSERVED: docs frontmatter.

Request params: `token`, `channel` (required), `ts` (required), `as_user` (optional legacy). OBSERVED: docs.

Response: `{ "ok": true, "channel": "C...", "ts": "..." }` plus a hidden tombstone broadcast via websocket (see `message_deleted`). OBSERVED: github.com/beeper/slackgo/blob/beeper/chat.go:31 (chatResponseFull carries channel/ts).

Used for: deleting own messages; client renders tombstone from the pushed event.

### chat.meMessage

Transport: `POST https://<workspace>.slack.com/api/chat.meMessage`, form-encoded.
OBSERVED: https://docs.slack.dev/reference/methods/chat.meMessage
OBSERVED: github.com/beeper/slackgo/blob/beeper/chat.go:345 (`chatMeMessage`; MsgOptionMeMessage chat.go:493)

Visibility: public (`chat:write`, Tier 3). OBSERVED: docs frontmatter.

Request params: `token`, `channel` (required), `text` (required). OBSERVED: docs.

Response: `{ "ok": true, "channel": "...", "ts": "..." }` (OBSERVED: github.com/beeper/slackgo/blob/beeper/chat.go:31).

Used for: `/me` action messages; server stores them as `subtype: "me_message"` (OBSERVED: github.com/wee-slack/wee-slack/blob/main/typings/slack_api/slack_conversations_history.pyi `SlackMessageMe subtype: "me_message"`; github.com/beeper/slackgo/blob/beeper/messages.go `MsgSubTypeMeMessage`).

### chat.scheduleMessage

Transport: `POST https://<workspace>.slack.com/api/chat.scheduleMessage`, form-encoded.
OBSERVED: https://docs.slack.dev/reference/methods/chat.scheduleMessage
OBSERVED: github.com/beeper/slackgo/blob/beeper/chat.go:341 (`chatScheduleMessage`; MsgOptionSchedule adds `post_at`, chat.go:464)

Visibility: public (`chat:write`, Tier 3). OBSERVED: docs frontmatter.

Request params: `token`, `channel` (required), `post_at` (int unix ts, required), plus the same message fields as postMessage (`text`, `blocks`, `attachments`, `thread_ts`, `reply_broadcast`, `link_names`, `parse`, `unfurl_links`, `unfurl_media`, `metadata`, `markdown_text`, `as_user`). OBSERVED: docs.slack.dev/reference/methods/chat.scheduleMessage Arguments.

Response:

```json
{ "ok": true, "channel": "C...", "scheduled_message_id": "Q...", "post_at": "299876400", "message": {...} }
```

OBSERVED: github.com/beeper/slackgo/blob/beeper/chat.go:35 (`ScheduledMessageID string \`json:"scheduled_message_id,omitempty"\``)

Used for: slackless send-later feature; scheduled messages appear to other clients only in `chat.scheduledMessages.list`.

### chat.scheduledMessages.list

Transport: `POST https://<workspace>.slack.com/api/chat.scheduledMessages.list`, form-encoded.
OBSERVED: https://docs.slack.dev/reference/methods/chat.scheduledMessages.list

Visibility: public (no scopes listed; Tier 3). OBSERVED: docs frontmatter.

Request params: `token`, optional `channel`, `cursor`, `latest`, `limit`, `oldest`, `team_id` (org tokens). OBSERVED: docs Arguments.

Response:

```json
{
  "ok": true,
  "scheduled_messages": [
    {
      "id": "Q...",
      "channel_id": "C...",
      "post_at": 299876400,
      "date_created": 1690000000,
      "text": "..."
    }
  ],
  "response_metadata": { "next_cursor": "..." }
}
```

OBSERVED: github.com/beeper/slackgo/blob/beeper/messages.go (`ScheduledMessage` struct: `id`, `channel_id`, `post_at`, `date_created`, `text`)

Pagination: cursor via `response_metadata.next_cursor`. OBSERVED: docs.

Used for: rendering the user's scheduled messages UI in slackless.

### chat.deleteScheduledMessage

Transport: `POST https://<workspace>.slack.com/api/chat.deleteScheduledMessage`, form-encoded.
OBSERVED: https://docs.slack.dev/reference/methods/chat.deleteScheduledMessage

Visibility: public (`chat:write`, Tier 3). OBSERVED: docs frontmatter.

Request params: `token`, `channel` (required), `scheduled_message_id` (required, from scheduleMessage), `as_user` (optional). OBSERVED: docs Arguments.

Response: `{ "ok": true }`.

Used for: canceling a scheduled send from slackless.

### chat.unfurl

Transport: `POST https://<workspace>.slack.com/api/chat.unfurl`, form-encoded with `unfurls` as JSON string.
OBSERVED: https://docs.slack.dev/reference/methods/chat.unfurl
OBSERVED: github.com/beeper/slackgo/blob/beeper/chat.go:519 (MsgOptionUnfurl adds `ts` + `unfurls` = json.Marshal(map[url]Attachment))

Visibility: public (`links:write`, Tier 3) - but only usable by the app that owns the unfurl; for the web client this matters mostly as behavior context.

Request params:

- Addressing, one pair required: `channel` + `ts` (posted message) OR `unfurl_id` + `source` (composer preview; `source` is `composer` or `conversations_history`) (OBSERVED: docs.slack.dev/reference/methods/chat.unfurl Arguments)
- `unfurls` - string - JSON map URL -> attachment/block payload (OBSERVED: docs; slackgo chat.go:519)
- `user_auth_url`, `user_auth_required`, `user_auth_message`, `user_auth_blocks` - authenticated-unfurl prompt (OBSERVED: docs; slackgo chat.go:577-619)

Response: `{ "ok": true }`.

Web-client behavior to model: user posts a link -> `chat.postMessage` includes empty `unfurl: []`; server fetches metadata and pushes the unfurl card asynchronously over the websocket; external images rewritten to `slack-imgs.com` proxy (`?c=1&o1=wi32.he32.si&url=<encoded>`); GitHub links handled by the app's unfurl via chat.unfurl rather than generic scraping.
OBSERVED: .omo/research/gist.md ("Link unfurling is entirely server-side"; "All unfurled images are proxied through slack-imgs.com"; "For GitHub links, the GitHub Slack app handles the unfurl via Slack's app unfurl API")

Used for: slackless mostly needs to RENDER server-unfurled `attachments` (with `from_url`); chat.unfurl is app-side.

### chat.attachmentAction

Transport: `POST https://<workspace>.slack.com/api/chat.attachmentAction` - endpoint name confirmed in the wild via Slack error strings ("chat.attachmentAction not ok: internal_error" when a legacy message button is pressed).
OBSERVED: https://stackoverflow.com/questions/75293431/slack-interactive-button-give-an-error-chat-attachmentaction-not-ok-internal (question title/body fetched via api.stackexchange.com)

Visibility: private/internal.

Request params: not captured (see LEADS).

Used for: legacy interactive-message attachment button presses (the route Slack itself uses when the user clicks a button on a legacy attachment; app-facing delivery is the documented `interaction payload`, OBSERVED: https://docs.slack.dev/reference/interaction-payloads).

### drafts.list

Transport: `POST https://<workspace>.slack.com/api/drafts.list` - JSON body (karbassi `session_call`) and form scalars both accepted.
OBSERVED: github.com/karbassi/slack-mcp/blob/master/src/slack_mcp/tools/undocumented.py (`drafts_list` -> `session_call("drafts.list", is_active=..., limit=...)`)
OBSERVED: github.com/karbassi/slack-mcp/blob/master/src/slack_mcp/client.py:130

Visibility: private.

Request params:

- `is_active` - bool - only drafts that are currently active/unsent (`"true"`/`"false"` strings sent)
- `limit` - int
  OBSERVED: karbassi/slack-mcp tools/undocumented.py `drafts_list`.

Response: `{ "ok": true, "drafts": [ { "id": "Dr...", "last_updated_ts": "1700000000.0000001", "blocks": [...rich_text...], "destinations": [{"channel_id": "C..."}], ... } ] }` - field names `drafts[]`, `id`, `last_updated_ts` observed via the delete flow reading them back.
OBSERVED: github.com/karbassi/slack-mcp/blob/master/src/slack_mcp/tools/undocumented.py (`drafts_delete` fallback fetches `drafts.list` and reads `drafts[].id` / `drafts[].last_updated_ts`)

Pagination: unobserved.

Used for: restoring unsent drafts at boot; the live web client calls `drafts.list` during boot phase 2. OBSERVED: .omo/research/findings.md (fresh-load fires `drafts.list`).

### drafts.create

Transport: `POST https://<workspace>.slack.com/api/drafts.create`, JSON body with double-encoded JSON strings inside.
OBSERVED: github.com/karbassi/slack-mcp/blob/master/src/slack_mcp/tools/undocumented.py (`drafts_create` + `_draft_body`)

Visibility: private.

Request params (from `_draft_body`):

- `blocks` - JSON string - `[{type:"rich_text", elements:[{type:"rich_text_section", elements:[{type:"text", text}]}]}]` (JSON-encoded inside the JSON body)
- `destinations` - JSON string - `[{"channel_id": "C...", "thread_ts": "...", "broadcast": false}]`
- `client_msg_id` - fresh UUID
- `file_ids` - JSON string - `[]` or `["F..."]`
- `is_from_composer` - `"true"` (create only)
- `date_scheduled` - optional unix ts (draft scheduled for sending)
  OBSERVED: karbassi/slack-mcp `_draft_body` (verbatim payload in source: "The drafts endpoints expect `blocks`, `destinations` and `file_ids` as JSON _strings_ nested inside the JSON request body (double-encoded)")

Response: draft object (incl. generated `id` and `last_updated_ts`). Not fully captured (see LEADS).

Used for: composer draft persistence across devices; appears in the Slack client's native drafts list. OBSERVED: stablyai/agent-slack issue #83 ("Slack has an undocumented client API for managing drafts... list/create/update/delete" - https://github.com/stablyai/agent-slack/issues/83)

### drafts.update

Transport: `POST https://<workspace>.slack.com/api/drafts.update`, JSON body (same double-encoding).
OBSERVED: github.com/karbassi/slack-mcp/blob/master/src/slack_mcp/tools/undocumented.py (`drafts_update`)

Visibility: private.

Request params: all of `_draft_body` above plus:

- `draft_id` - string - Dr... id
- `client_last_updated_ts` - string - MUST be padded to 7 decimal places (karbassi `_pad_draft_ts`: `1700000000.1` -> `1700000000.1000000`) or Slack rejects with a conflict
  OBSERVED: karbassi/slack-mcp `_pad_draft_ts` ("Pad a draft timestamp to 7 decimal places (required by Slack)")

Used for: live-typing persistence (debounced while the user types).

### drafts.delete

Transport: `POST https://<workspace>.slack.com/api/drafts.delete` - FORM-encoded, not JSON (karbassi deliberately uses `session_call_form`; its docstring notes some legacy endpoints require form-encoded data rather than JSON).
OBSERVED: github.com/karbassi/slack-mcp/blob/master/src/slack_mcp/tools/undocumented.py (`drafts_delete` -> `session_call_form`)
OBSERVED: github.com/karbassi/slack-mcp/blob/master/src/slack_mcp/client.py:137 (`session_call_form` posts `application/x-www-form-urlencoded; charset=utf-8`)

Visibility: private.

Request params: `draft_id` (required), `client_last_updated_ts` (7-decimal padded, required; fetched from drafts.list if omitted). OBSERVED: karbassi/slack-mcp `drafts_delete`.

Used for: discarding a draft from the composer or drafts list.

### drafts.listActive

Transport: `POST https://<workspace>.slack.com/api/drafts.listActive`.
OBSERVED: .omo/research/findings.md (fresh-load boot capture lists `drafts.listActive` among the calls fired)

Visibility: private.

Request params: unobserved (see LEADS). Likely a filtered variant of drafts.list returning only composer-active drafts.

Used for: the web client restoring the draft the user was last typing in, per conversation, right after boot.

## Message object shape (conversations.history / replies / view / websocket `message`)

Canonical observed fields (union over sources):

```json
{
  "type": "message",
  "subtype": "thread_broadcast",
  "text": "hello",
  "ts": "1700000000.000100",
  "thread_ts": "1700000000.000000",
  "client_msg_id": "a789f1c4-...",
  "user": "U...",
  "team": "T...",
  "user_profile": { "avatar_hash": "...", "image_72": "...", "display_name": "...", "real_name": "..." },
  "blocks": [ { "type": "rich_text", "elements": [...] } ],
  "attachments": [ { "from_url": "...", "service_name": "...", "title": "...", "title_link": "...", "text": "...", "image_url": "...", "fallback": "...", "footer": "..." } ],
  "files": [ { ...file object... } ],
  "edited": { "user": "U...", "ts": "1700000001.000000" },
  "reactions": [ { "name": "eyes", "users": ["U1"], "count": 1 } ],
  "bot_id": "B...",
  "bot_profile": { "id": "B...", "app_id": "A...", "name": "...", "icons": {...}, "team_id": "T...", "updated": 123 },
  "reply_count": 3, "reply_users_count": 2, "reply_users": ["U1","U2"], "replies": [{"user":"U1","ts":"..."}],
  "latest_reply": "1700000100.000200", "parent_user_id": "U...",
  "subscribed": false, "is_locked": false, "last_read": "...", "unread_count": 0,
  "is_starred": false, "pinned_to": [], "hidden": false, "deleted_ts": "...", "event_ts": "...",
  "upload": false, "display_as_bot": false, "permalink": "https://<ws>.slack.com/archives/C.../p1700000000000100",
  "username": "...", "icons": { "emoji": ":robot_face:", "image_64": "..." }
}
```

OBSERVED: github.com/wee-slack/wee-slack/blob/main/typings/slack_api/slack_conversations_history.pyi (`SlackMessageStandardCommon`: client_msg_id/user/user_profile/bot_id/bot_profile/blocks/attachments/team/edited; thread parent: thread_ts/reply_count/reply_users_count/latest_reply/reply_users/is_locked/subscribed/last_read; broadcast: subtype "thread_broadcast" + root; files variant: files/upload/display_as_bot; reaction: name/users/count; attachment standard: from_url/service_name/title/title_link/service_icon/fallback/original_url)
OBSERVED: github.com/beeper/slackgo/blob/beeper/messages.go (`Msg` struct: type/channel/user/text/ts/thread_ts/is_starred/pinned_to/attachments/edited/last_read/subscribed/unread_count/subtype/hidden/deleted_ts/event_ts/bot_id/username/icons/bot_profile/trigger_id/workflow_id/inviter/topic/purpose/name/old_name/members/reply_count/reply_users/replies/parent_user_id/latest_reply/files/upload/permalink/reactions/team/blocks/room; wrapper `Message` adds `message` (SubMessage) + `previous_message` + `root`)
OBSERVED: github.com/beeper/slackgo/blob/beeper/messages.go (subtype constants list: bot_message, me_message, message_changed, message_deleted, message_replied, reply_broadcast, thread_broadcast, channel_join/leave, channel_topic/purpose/name, channel_archive/unarchive, group_*, file_share, file_comment, file_mention, pinned_item, unpinned_item, ekm_access_denied, channel_posting_permissions, assistant_app_thread)
OBSERVED: https://docs.slack.dev/reference/methods/conversations.history (message samples: type/user/text/ts; attachment sample with service_name/text/fallback)

`subtype` values seen in history-type responses: `me_message`, `bot_message`, `thread_broadcast`, `huddle_thread`, `channel_join`, `channel_leave`, `channel_topic`, `bot_add`, `bot_remove`, `file_share` (OBSERVED: github.com/wee-slack/wee-slack/blob/main/typings/slack_api/slack_conversations_history.pyi Literals cover all of these except `file_share`; `file_share` per github.com/beeper/slackgo/blob/beeper/messages.go `MsgSubTypeFileShare`).

## Full rich_text block element schema

A user-typed message is one `rich_text` block whose `elements` are containers; containers hold inline elements. OBSERVED: github.com/beeper/slackgo/blob/beeper/block_rich_text.go (`RichTextBlock`: `type`, `block_id`, `elements[]`), OBSERVED: .omo/research/gist.md ("Block Kit is the universal message format").

Containers:

- `rich_text_section` - `{ type, elements: [...inline...], border? }` - OBSERVED: github.com/beeper/slackgo/blob/beeper/block_rich_text.go (`RichTextSection`; note: section `type` may be absent in responses, defaults to `rich_text_section` per UnmarshalJSON)
- `rich_text_list` - `{ type, elements: [...section-like...], style: "ordered"|"bullet", indent, offset?, border }` - OBSERVED: slackgo block_rich_text.go (`RichTextList`; styles `RTEListOrdered`/`RTEListBullet`) and github.com/wee-slack/wee-slack/blob/main/typings/slack_api/slack_conversations_history.pyi (`style: Literal["ordered","bullet"]`, `indent`, `offset` NotRequired, `border`)
- `rich_text_quote` - `{ type, elements: [...], border? }` - OBSERVED: slackgo block_rich_text.go (`RichTextQuote`)
- `rich_text_preformatted` - `{ type, elements: [...], border, language? }` (code blocks; `language` field observed in slackgo fork) - OBSERVED: slackgo block_rich_text.go (`RichTextPreformatted` with `Language`)

Inline elements (all with optional `style` unless noted) - OBSERVED: github.com/beeper/slackgo/blob/beeper/block_rich_text.go (RTSE* constants + structs):

- `text` - `{ type: "text", text, style? }`
- `link` - `{ type: "link", url, text?, style? }`
- `user` - `{ type: "user", user_id }`
- `channel` - `{ type: "channel", channel_id }`
- `usergroup` - `{ type: "usergroup", usergroup_id }`
- `team` - `{ type: "team", team_id }` (Enterprise user group/IDP group variant)
- `broadcast` - `{ type: "broadcast", range: "channel"|"here"|"everyone" }` - OBSERVED: slackgo `RichTextBroadcastRange` constants; wee-slack typing has `range: Literal["channel","here"]` (OBSERVED: typings/slack_api/slack_conversations_history.pyi)
- `emoji` - `{ type: "emoji", name, skin_tone?, unicode? }`
- `date` - `{ type: "date", timestamp, format, url?, fallback? }`
- `color` - `{ type: "color", value }`
- `message_mention` - `{ type: "message_mention", channel_id, message_ts, author_id?, text?, url? }` (permalink paste) - OBSERVED: slackgo block_rich_text.go (`RichTextSectionMessageMentionElement`)
- `attachment_mention` - `{ type: "attachment_mention", url, text?, app_id?, entity_id?, icon_url?, ... }` (Work Object mention) - OBSERVED: slackgo block_rich_text.go
- `canvas` - `{ type: "canvas", file_id }` - OBSERVED: slackgo block_rich_text.go (`RichTextSectionCanvasElement`)

Text `style` flags: `bold`, `italic`, `strike`, `code`, plus observed server-side: `underline`, `highlight`, `client_highlight`, `unlink` - OBSERVED: github.com/beeper/slackgo/blob/beeper/block_rich_text.go (`RichTextSectionTextStyle`). `client_highlight` is set by the server on search-result snippets (OBSERVED: .omo/research/gist.md - "the server returns the original Block Kit structure but with an extra `client_highlight": true` style on matched text spans").

wee-slack's observed typing (from live traffic) matches: text(bold/italic/strike/code), link(url/text), emoji(name/unicode/skin_tone), color(value), date(timestamp/format), channel(channel_id), user(user_id), usergroup(usergroup_id), broadcast(range) - OBSERVED: github.com/wee-slack/wee-slack/blob/main/typings/slack_api/slack_conversations_history.pyi lines 15-101.

## Legacy mrkdwn escaping and entity syntax

Escaping rules for `text` fields (mrkdwn contexts): `&` -> `&amp;`, `<` -> `&lt;`, `>` -> `&gt;`; only these three are decoded. OBSERVED: https://docs.slack.dev/messaging/formatting-message-text ("You shouldn't encode the entire piece of text, as only the specific characters shown above will be decoded")
OBSERVED: github.com/dylex/slack-libpurple/blob/master/slack-message.c:52-64 (escaping `&`, `<`, `>` to entities when sending)

Entity syntax in plain-text/mrkdwn fields (what the client must parse when rendering `text` fallbacks and attachments):

- `<@U012AB3CD>` - user mention (OBSERVED: docs.slack.dev/messaging/formatting-message-text "provide their user ID with the following syntax")
- `<@U123|label>` - user mention with fallback label (OBSERVED: slack-libpurple slack-message.c builds `<@id|name>` on send, parses `@...|label` on receive)
- `<#C123|channel-name>` - channel mention (OBSERVED: slack-libpurple slack-message.c:121 renders `#` + label)
- `<!here>`, `<!channel>`, `<!everyone>`, `<!group>` - broadcast (OBSERVED: slack-libpurple slack-message.c COMMAND(here/channel/everyone); also github.com/beeper/slackgo/blob/beeper/block_rich_text.go broadcast ranges)
- `<url|label>` - labeled link (OBSERVED: slack-libpurple slack-message.c default branch builds `<A HREF...>`; parses `|` label; https://docs.slack.dev/messaging/formatting-message-text links section)
- `\n` - newline (OBSERVED: slack-libpurple slack-message.c `<br>` -> `\n` on send, `\n` -> `<BR>` on receive)

## Websocket events

Transport frame: `{"type": "<event>", ..., "channel": "C..."}` delivered on the RTM websocket the web client opens at `wss://wss-primary.slack.com/...` (OBSERVED: .omo/research/findings.md - websocket URL with `lazy_channels=1&no_query_on_subscribe=1`; auth uses the session token).

### message (new message)

Payload: full message object (shape above) plus `channel`. Fires for every message posted to a conversation the user is a member of, including own sends (which also arrive after the chat.postMessage response). OBSERVED: github.com/beeper/slackgo/blob/beeper/messages.go (`MessageEvent` wraps `Msg`); mautrix-slack routes `*slack.MessageEvent` as the new-message path (OBSERVED: github.com/mautrix/slack/blob/main/pkg/connector/handleslack.go:252).

### message subtype `message_changed` (edit, incl. unfurl application)

Payload: `{ "type": "message", "subtype": "message_changed", "hidden": true, "channel": "C...", "ts": "<now>", "message": { ...new message... }, "previous_message": { ...old message... } }`. OBSERVED: github.com/beeper/slackgo/blob/beeper/messages.go (`Message.SubMessage json:"message,omitempty"`, `PreviousMessage json:"previous_message,omitempty"`, `Hidden`); mautrix converts edits from these events (OBSERVED: github.com/mautrix/slack/blob/main/pkg/connector/handleslack.go:563 `convertEdit`).

### message subtype `message_deleted`

Payload: `{ "type": "message", "subtype": "message_deleted", "hidden": true, "channel": "C...", "ts": "<now>", "deleted_ts": "<deleted message ts>" }`. OBSERVED: github.com/beeper/slackgo/blob/beeper/messages.go (`DeletedTimestamp json:"deleted_ts,omitempty" // message_deleted`); mautrix handles it (OBSERVED: handleslack.go:621 `MsgSubTypeMessageDeleted`).

### message subtype `message_replied` (thread activity)

Fires on the parent thread when a reply arrives (also broadcast to subscribers); carries thread bookkeeping. OBSERVED: github.com/beeper/slackgo/blob/beeper/messages.go (`MsgSubTypeMessageReplied`); mautrix treats `message_replied` as thread-metadata change (OBSERVED: github.com/mautrix/slack/blob/main/pkg/connector/handleslack.go:623).

### message subtype `thread_broadcast`

A reply broadcast to the channel; carries `root` = parent summary. OBSERVED: github.com/wee-slack/wee-slack/blob/main/typings/slack_api/slack_conversations_history.pyi (`SlackMessageThreadBroadcast` with `root`); github.com/beeper/slackgo/blob/beeper/messages.go (`Root *Msg` "message that was broadcast to the channel").

### reaction_added / reaction_removed

Payload: `{ "type": "reaction_added", "user": "U...", "reaction": "eyes", "item_user": "U...", "item": { "type": "message", "channel": "C...", "ts": "..." }, "event_ts": "..." }`. OBSERVED: github.com/beeper/slackgo/blob/beeper/websocket_reactions.go (`ReactionEvent`: user/item_user/item{type,channel,ts}/reaction/event_ts; ReactionItem.Timestamp json key is `ts`). Handled by mautrix (OBSERVED: github.com/mautrix/slack/blob/main/pkg/connector/handleslack.go:316-321).

### user_typing

Payload: `{ "type": "typing", "channel": "C...", "user": "U..." }`. OBSERVED: github.com/beeper/slackgo/blob/beeper/websocket_misc.go:59 (`UserTypingEvent{Type, User, Channel}`); mautrix consumes it (OBSERVED: handleslack.go:323). libpurple SENDS typing over RTM as `{"type":"typing","channel":...,"thread_ts"?}` (OBSERVED: github.com/dylex/slack-libpurple/blob/master/slack-message.c:654-659).

### channel_marked / im_marked / group_marked / thread_marked (read markers)

Fires when the user's read marker advances for a channel/DM/group/thread; payload `{ "type": "channel_marked", "channel": "C...", "ts": "..." }`. OBSERVED: github.com/beeper/slackgo/blob/beeper/websocket_channels.go (`ChannelMarkedEvent`); mautrix handles channel/im/group/thread marked (OBSERVED: github.com/mautrix/slack/blob/main/pkg/connector/handleslack.go:330-345). (Read-state writes themselves - `conversations.mark`, `subscriptions.thread.mark` - are owned by the read-state area doc; conversations.mark observed at github.com/dylex/slack-libpurple/blob/master/slack-conversation.c:177.)

### hello / connected

`{"type":"hello"}` on websocket connect; mautrix gates catch-up on it. OBSERVED: github.com/mautrix/slack/blob/main/pkg/connector/handleslack.go:101 (`HelloEvent` -> `rtmHelloReceived`).

## Gotchas

- Rate limits are per-method tiers: `conversations.*` history reads are Tier 3 (50+/min); chat.postMessage docs frontmatter declares `t5` (OBSERVED: https://docs.slack.dev/reference/methods/conversations.history; https://docs.slack.dev/reference/methods/chat.postMessage). On HTTP 429, retry honoring `Retry-After` - mautrix uses `RateLimitedError.RetryAfter` as the floor of its backoff (OBSERVED: github.com/mautrix/slack/blob/main/pkg/connector/client.go:249-251).
- Session errors to handle: `invalid_auth`, `not_authed`, `token_revoked`, `token_expired`, `account_inactive`, `team_disabled`, `user_removed_from_team`; `ratelimited`/`service_unavailable`/`request_timeout` are retryable. OBSERVED: github.com/mautrix/slack/blob/main/pkg/connector/client.go:204-209 (`handleConnectError` switch over `slackAPIErrorCode`).
- Auth: every POST carries `token=<xoxc>` in the form body plus `Cookie: d=<xoxd>`; the cookie option is how the beeper fork logs in (OBSERVED: github.com/mautrix/slack/blob/main/pkg/connector/client.go:68-79 `slack.OptionCookie("d", cookieToken)`; github.com/rusq/slackdump/blob/master/internal/edge/edge.go:247 PostFormRaw sets `form["token"]`). Enterprise Grid adds `slack_route=<orgID>:<orgID>` query param and `<workspace>.enterprise.slack.com` host (OBSERVED: .omo/research/gist.md "Enterprise Grid Routing").
- ts values are message IDs: orderable, addressable; the client sends an optimistic ts and the authoritative one comes back in the chat.postMessage response keyed by `client_msg_id` dedup (OBSERVED: .omo/research/gist.md "Timestamps are message IDs" + "Client generates an optimistic timestamp"). Drafts however REQUIRE exactly 7 decimal places in their ts (`_pad_draft_ts`), a different convention from message ts (6 places observed). OBSERVED: github.com/karbassi/slack-mcp/blob/master/src/slack_mcp/tools/undocumented.py `_pad_draft_ts`.
- Editing semantics differ by method: chat.update defaults `parse` to `client` and overwrites `link_names` if omitted - always send explicit values (OBSERVED: https://docs.slack.dev/reference/methods/chat.update).
- Sending overwrites unfurls: chat.update drops existing attachment unfurls unless included; there is a separate `unfurled_attachments` param (OBSERVED: https://docs.slack.dev/reference/methods/chat.update Arguments).
- Unfurl lifecycle is async: postMessage returns before the unfurl card exists; it arrives later over the websocket as an edit of the message. Images are proxied via `slack-imgs.com` and must not be loaded from origin (OBSERVED: .omo/research/gist.md).
- `conversations.view` is the web client's hydration contract and undocumented; its response embeds history and side-loads users/emojis - treat field drift as likely and parse defensively but keep `channel` + `messages` primary (OBSERVED: slackdump only models `users`/`im`/`emojis` and ignores the rest, github.com/rusq/slackdump/blob/master/internal/edge/conversations.go).
- Threads are lazy: plain channel loads never call conversations.replies; only opening a thread does (OBSERVED: .omo/research/findings.md). Don't pre-fetch threads on channel open.
- History pagination: `limit` caps at 999 (docs); `cursor` pagination works, but the web client's own scrolling uses `oldest`; `is_limited: true` in a history response means the free-plan message cap hid older messages (OBSERVED: wee-slack typing `is_limited` NotRequired).
- Drafts JSON-in-JSON: `blocks`, `destinations`, `file_ids` are JSON-encoded strings INSIDE a JSON body for drafts.create/update, while drafts.delete is form-encoded - mixing them up yields "field missing" errors (OBSERVED: karbassi/slack-mcp `_draft_body` docstring + client.py:137 and issue ref in code comments).
- chat.postMessage accepts a documented `current_draft_last_updated_ts` param - when sending a former draft, pass it to resolve server-side draft conflicts (OBSERVED: https://docs.slack.dev/reference/methods/chat.postMessage).
- Web client telemetry params (`_x_reason`, `_x_mode`, `_x_id`, `_x_csid`, `fp`, `_x_gantry`) are sent on every call; they are attribution, not auth - safe to include, and `_x_mode: online` hints at an offline replay path (OBSERVED: .omo/research/gist.md).
- Enterprise Grid: conversation ids and methods work the same, but requests route through `<workspace>.enterprise.slack.com` and edge cache `edgeapi.slack.com/cache/<orgID>/...` for users/channels (not messages) (OBSERVED: .omo/research/gist.md).
- `chat.postMessage` `link_names: true` no longer links individual users, only user groups (OBSERVED: https://docs.slack.dev/reference/methods/chat.postMessage).

## LEADS

- `conversations.historyChanges` request/response shape: never observed in any mined source; only its invocation at boot is captured (.omo/research/findings.md). Capture from the live client DevTools; expect `channel` + ts-based delta params.
- `drafts.listActive` params/response: only its existence (boot call) is observed; capture wire format live. Presumably a drafts.list variant filtered to composer-active drafts.
- `chat.attachmentAction` wire params: endpoint name confirmed via a live error string only (StackOverflow 75293431); params likely mirror interaction payloads. Confirm by pressing a legacy attachment button with DevTools open.
- Unfurl application is believed to arrive as `message_changed` (attachments added) on the original message ts - consistent with the edit event schema but not directly observed in mined sources; verify live.
- `drafts.create`/`drafts.update` full response (draft object field list beyond `id`/`last_updated_ts`): unobserved; karbassi only reads back those two fields.
- `conversations.genericInfo` in the current web client boot was observed as `conversations.genericInfo` in an earlier capture (.omo/research/findings.md "Other calls observed during bootstrap") but slackdump's live code labels it "fallback:UnknownFetchManager" - the web client may have moved to a replacement; verify before relying on it.
- Whether `chat.postMessage` accepts `rich_text` blocks with all container types (`rich_text_list`, `rich_text_preformatted`) symmetric with what history returns: the web client sends them (composer supports lists/code blocks), but mined wire captures only show a plain `rich_text_section`. Confirm round-trip live.
- `messages.list` (private batch fetch) response envelope key names beyond message objects: only the request was observed; capture response.
- Slack's public `chat.postMessage` rate limit: frontmatter says `t5`, older documentation described a special ~1 msg/sec per channel limit; reconcile against live 429 behavior.
- `conversations.view` variants observed (`count=50` inline history) vs the live client's exact flag set at boot: slackdump's flags are a subset; a full DevTools capture would pin the canonical flag set.
- `drafts.delete` behavior "a JSON body is ignored here and Slack reports every field missing": unverified: quoted wording not present in any mined source; only the form-encoded transport is reproducible (karbassi `session_call_form`, client.py:137).
