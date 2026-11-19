# Reactions, pins, bookmarks, saved items and reminders - Slack web-client API

## Summary

- Covers message reactions (emoji), channel pins, channel bookmarks bar (incl. folders), the legacy stars.* family, the private saved.* ("Later"/remind-me) family, and reminders.*.
- Public methods (reactions, pins, bookmarks, stars, reminders) work with xoxc- session tokens sent as a body `token` field; the saved.* family is web-client-only and effectively requires session auth (xoxc token + d cookie).
- Biggest risks: (1) saved.* is entirely undocumented and has no public schema; (2) stars.* and reminders.* are officially retired (March 2023) - the web client replaced "remind me about this message" with `saved.add`+`date_due`; (3) the modern web client no longer calls reactions.get/reactions.list/stars.list/bookmarks.edit/reminders.* - it consumes `message.reactions` arrays and `saved_*` socket events instead.
- Reaction pickers/rankings come from cross-area methods (`search.autocomplete.topEmojis`, `users.prefs` `emoji_use`) - see Gotchas, one line each.
- Transport for everything here: `POST https://<workspace>.slack.com/api/<method>`, form-encoded, `token=<xoxc->` in the body, `Cookie: d=<xoxd->` header (the web client and all mined clients POST; note docs.slack.dev officially documents GET for the read methods reactions.get, reactions.list, pins.list, stars.list, reminders.list, reminders.info - see those sections). OBSERVED: .omo/research/gist.md ("All API tokens are sent in request bodies", lines 309/335; auth table) and dylex/slack-libpurple slack-api.c:138-177 (multipart form with `token` field + `Cookie: d=`), wee-slack slack/slack_api.py:141-150 (`_fetch` builds `https://api.slack.com/api/<method>` with urlencoded postfields) and :67-70 (`Authorization: Bearer` + cookie header).

## Endpoints

### reactions.add

- Transport: `POST https://<workspace>.slack.com/api/reactions.add`, form-encoded. OBSERVED: docs.slack.dev/reference/methods/reactions.add; .omo/research/gist.md ("Adding a Reaction (reactions.add)" shows live captured form fields).
- Visibility: public.
- Request params:
  - `channel` - string (channel ID, e.g. `C09KB2J6QNN`) - required for messages - channel the message is in.
  - `timestamp` - string (message ts, e.g. `1772325870.309319`) - required for messages - the message ID.
  - `name` - string - required - emoji name WITHOUT colons (e.g. `eyes`). Skin tone modifier may be appended (see Gotchas).
  - `file` - string - deprecated - file to react to.
  - `file_comment` - string - deprecated - file comment to react to.
  - OBSERVED: docs.slack.dev/reference/methods/reactions.add; beeper/slackgo reactions.go:145-170 (sets name/channel/timestamp/file/file_comment); .omo/research/gist.md (`channel: C09KB2J6QNN`, `timestamp: 1772325870.309319`, `name: eyes`, `_x_reason: changeReactionFromUserAction`).
  - Web-client request tagging: `_x_reason=changeReactionFromUserAction`. OBSERVED: .omo/research/gist.md.
- Response: `{"ok": true}`. OBSERVED: docs.slack.dev/reference/methods/reactions.add ("Typical success response { ok: true }"); ImShyMike/slack-undoc-client src/types.ts:18786-18789 (`ReactionsAddResponse = { ok: true }`).
- Pagination: none.
- Used for: the message hover-bar reaction button and emoji picker.
- Errors worth handling: `already_reacted` (also `bad_timestamp`, `channel_not_found`, `access_denied`; `not_found` is NOT in the official errors table - corrected in verification). OBSERVED: docs.slack.dev/reference/methods/reactions.add (Errors table + typical error response `already_reacted`).
- Rate limit: Tier 3 (50+/min). OBSERVED: docs.slack.dev/reference/methods/reactions.add.

### reactions.remove

- Transport: `POST https://<workspace>.slack.com/api/reactions.remove`, form-encoded. OBSERVED: docs.slack.dev/reference/methods/reactions.remove.
- Visibility: public.
- Request params: same shape as reactions.add (`channel` + `timestamp` for messages; `name` required; `file`/`file_comment` optional/legacy). OBSERVED: docs.slack.dev/reference/methods/reactions.remove (required: `token`, `name`; optional: `file`, `file_comment`, `channel`, `timestamp` - "One of file, file_comment, or the combination of channel and timestamp must be specified"); beeper/slackgo reactions.go:185-214; wee-slack slack/slack_api.py:537-559 (`reactions_change` maps `+`/`-` to reactions.add/reactions.remove with `channel`, `timestamp`, `name`).
- Response: `{"ok": true}`. OBSERVED: ImShyMike/slack-undoc-client src/types.ts:18790-18792.
- Pagination: none.
- Used for: clicking your own reaction chip to un-react.
- Errors: `no_reaction` (typical error response; also `bad_timestamp`, `channel_not_found`; `not_found` is NOT in the official errors table - corrected in verification). OBSERVED: docs.slack.dev/reference/methods/reactions.remove (Errors table + typical error response `no_reaction`).
- Rate limit: Tier 2 (20+/min). OBSERVED: docs.slack.dev/reference/methods/reactions.remove.

### reactions.get

- Transport: `GET https://<workspace>.slack.com/api/reactions.get`, params as query string (official Method Access shows GET; the mined clients POST the same path, e.g. beeper/slackgo reactions.go:224+ postMethod - both accepted). OBSERVED: docs.slack.dev/reference/methods/reactions.get ("GET https://slack.com/api/reactions.get"). CORRECTED in verification: was listed as POST.
- Visibility: public (not called by the current Gantry web client - its method union has no reactions.get; see Gotchas).
- Request params:
  - `channel` + `timestamp` - strings - for a message (this is the web-client case).
  - `file` - string - for a file.
  - `file_comment` - string - for a file comment.
  - `full` - boolean - if true always return the complete reaction list (otherwise users list may be truncated).
  - OBSERVED: docs.slack.dev/reference/methods/reactions.get; beeper/slackgo reactions.go:209-230 (GetReactionsContext sets channel/timestamp/file/file_comment/full).
- Response excerpt:
  ```json
  {
    "ok": true,
    "type": "message",
    "channel": "C123ABC456",
    "message": {
      "type": "message",
      "text": "Hi there!",
      "ts": "...",
      "reactions": [{ "name": "thumbsup", "count": 2, "users": ["U1", "U2"] }]
    }
  }
  ```
  OBSERVED: docs.slack.dev/reference/methods/reactions.get (response description + example); beeper/slackgo reactions.go:13-50 (`getReactionsResponseFull` with `type`, `channel`, `message`, `file`, `comment` variants).
- Pagination: none.
- Used for: fetching full reaction detail for a single item; the desktop client can instead read `message.reactions` from history payloads.
- Errors: `bad_timestamp`, `channel_not_found` (`not_found` is NOT in the official errors table - corrected in verification). OBSERVED: docs.slack.dev/reference/methods/reactions.get (Errors table).
- Rate limit: Tier 3 (50+/min). OBSERVED: docs.slack.dev/reference/methods/reactions.get.

### reactions.list

- Transport: `GET https://<workspace>.slack.com/api/reactions.list`, params as query string (official Method Access shows GET; the mined clients POST the same path - beeper/slackgo reactions.go:291 postMethod). OBSERVED: docs.slack.dev/reference/methods/reactions.list ("GET https://slack.com/api/reactions.list"). CORRECTED in verification: was listed as POST.
- Visibility: public (legacy; not in the current web client's method union).
- Request params:
  - `user` - string - optional - reactions made by this user (defaults to caller).
  - `team_id` - string - optional - required with org-level tokens.
  - `full` - boolean - optional.
  - `limit` - int - optional - page size (official docs also list legacy `count` (default 100) and `page` (default 1)).
  - `cursor` - string - optional - cursor from previous `response_metadata.next_cursor`.
  - OBSERVED: docs.slack.dev/reference/methods/reactions.list (Optional arguments: user, full, count, page, cursor, limit, team_id); beeper/slackgo reactions.go:76-84 and 267-291 (ListReactionsParameters sends user/team_id/cursor/limit/full).
- Response excerpt:
  ```json
  {
    "ok": true,
    "items": [
      {
        "type": "message",
        "channel": "C123ABC456",
        "message": {
          "ts": "...",
          "reactions": [
            { "count": 1, "name": "robot_face", "users": ["U123ABC456"] }
          ]
        }
      }
    ],
    "response_metadata": { "next_cursor": "..." }
  }
  ```
  OBSERVED: docs.slack.dev/reference/methods/reactions.list (Typical success response); beeper/slackgo reactions.go:59-74 (`listReactionsResponseFull`, `ResponseMetadata`).
- Pagination: cursor-based (`cursor` + `response_metadata.next_cursor`). OBSERVED: beeper/slackgo reactions.go:275-291.
- Used for: "show all my reactions" views; legacy.
- Rate limit: Tier 2 (20+/min). OBSERVED: docs.slack.dev/reference/methods/reactions.list.

### pins.add

- Transport: `POST https://<workspace>.slack.com/api/pins.add`, form-encoded. OBSERVED: docs.slack.dev/reference/methods/pins.add.
- Visibility: public.
- Request params:
  - `channel` - string - required - channel ID to pin into.
  - `timestamp` - string - required in practice - ts of the message to pin.
  - `file` / `file_comment` - strings - legacy file pinning ("In the past, files and file comments could be pinned"). OBSERVED: docs.slack.dev/reference/methods/pins.add (Usage info).
  - OBSERVED: docs.slack.dev/reference/methods/pins.add (Arguments + Usage info: "Both the channel and timestamp arguments are functionally required"); beeper/slackgo pins.go:17-40; ImShyMike/slack-undoc-client src/types.ts:1564-1568 (`PinsAddParams { timestamp, channel }`).
- Response: `{"ok": true}`. OBSERVED: ImShyMike/slack-undoc-client src/types.ts:18586-18588.
- Pagination: none.
- Used for: "Pin to channel" context-menu action; the pinned items banner.
- Errors: `already_pinned`, `bad_timestamp`, `channel_not_found`, `not_found`, `permission_denied`-class. OBSERVED: docs.slack.dev/reference/methods/pins.add (Errors table lists `already_pinned`, `bad_timestamp`, `channel_not_found`).
- Rate limit: Tier 2 (20+/min). OBSERVED: docs.slack.dev/reference/methods/pins.add.

### pins.remove

- Transport: `POST https://<workspace>.slack.com/api/pins.remove`, form-encoded. OBSERVED: docs.slack.dev/reference/methods/pins.remove.
- Visibility: public.
- Request params: `channel` (required) + `timestamp` (for messages); `file`/`file_comment` legacy. OBSERVED: docs.slack.dev/reference/methods/pins.remove; beeper/slackgo pins.go:50-74; ImShyMike/slack-undoc-client src/types.ts:1573-1576 (`PinsRemoveParams { channel, timestamp }`).
- Response: `{"ok": true}`. OBSERVED: ImShyMike/slack-undoc-client src/types.ts:18653-18655.
- Pagination: none.
- Used for: unpinning from the pinned-items banner.
- Errors: `no_pin` (typical error response; also `bad_timestamp`, `channel_not_found`; CORRECTED in verification - was "not_pinned-class"). OBSERVED: docs.slack.dev/reference/methods/pins.remove (typical error response `{"error":"no_pin"}` + Errors table).
- Rate limit: Tier 2. OBSERVED: docs.slack.dev/reference/methods/pins.remove.

### pins.list

- Transport: `GET https://<workspace>.slack.com/api/pins.list`, params as query string (official Method Access shows GET; the mined clients POST the same path - beeper/slackgo pins.go:101 postMethod). OBSERVED: docs.slack.dev/reference/methods/pins.list ("GET https://slack.com/api/pins.list"). CORRECTED in verification: was listed as POST.
- Visibility: public.
- Request params: `channel` - string - required. OBSERVED: docs.slack.dev/reference/methods/pins.list; beeper/slackgo pins.go:90-108; ImShyMike/slack-undoc-client src/types.ts:1569-1571 (`PinsListParams { channel }`).
- Response excerpt:
  ```json
  {
    "ok": true,
    "items": [
      {
        "type": "message",
        "channel": "C123ABC456",
        "created": 1508881078,
        "created_by": "U123ABC456",
        "message": {
          "permalink": "https://hitchhikers.slack.com/archives/C2U86NC6H/p1508197641000151",
          "pinned_to": ["C2U86NC6H"],
          "text": "...",
          "ts": "1508197641.000151",
          "type": "message",
          "user": "U123ABC456"
        }
      }
    ]
  }
  ```
  OBSERVED: docs.slack.dev/reference/methods/pins.list (Typical success response); ImShyMike/slack-undoc-client src/types.ts:18590-18651 (`PinsListResponse` items with `type`, `created`, `created_by`, `channel`, `message{...}` incl. thread fields).
- Pagination: none; legacy `paging` block. OBSERVED: beeper/slackgo pins.go:8-12 (`listPinsResponseFull` embeds `Paging`).
- Used for: rendering the channel's pinned items list (also fetched on channel open by clients that show pins).
- Rate limit: Tier 2. OBSERVED: docs.slack.dev/reference/methods/pins.list.

### bookmarks.add

- Transport: `POST https://<workspace>.slack.com/api/bookmarks.add`, form-encoded. OBSERVED: docs.slack.dev/reference/methods/bookmarks.add.
- Visibility: public.
- Request params:
  - `channel_id` - string - channel to add the bookmark in ("Required for public channels"). OBSERVED: docs.slack.dev/reference/methods/bookmarks.add.
  - `title` - string - required.
  - `type` - string - required - e.g. `link`. OBSERVED: docs.slack.dev/reference/methods/bookmarks.add ("Type of the bookmark i.e link").
  - `link` - string - URL for type `link`. OBSERVED: docs.slack.dev/reference/methods/bookmarks.add.
  - `emoji` - string - optional - emoji tag shown as the bookmark icon. OBSERVED: docs.slack.dev/reference/methods/bookmarks.add.
  - `entity_id` - string - optional - "ID of the entity being bookmarked. Only applies to message and file types". OBSERVED: docs.slack.dev/reference/methods/bookmarks.add.
  - `parent_id` - string - optional - parent folder bookmark (creates an entry inside a bookmarks folder); official docs also list `access_level` (read/write) for files. OBSERVED: docs.slack.dev/reference/methods/bookmarks.add (Optional arguments incl. access_level, parent_id); beeper/slackgo bookmarks.go:28-35 (`AddBookmarkParameters.ParentID`); ImShyMike/slack-undoc-client src/types.ts:362-369 (`BookmarksAddParams { channel, link, title, type, parent_id, entity_id }`).
- Response excerpt:
  ```json
  {
    "ok": true,
    "bookmark": {
      "id": "Bk123ABC4DEF",
      "channel_id": "C1RQ000",
      "title": "...",
      "link": "...",
      "icon_url": null,
      "type": "link",
      "date_created": 1644956055,
      "date_updated": 0,
      "rank": "g",
      "last_updated_by_user_id": "U...",
      "last_updated_by_team_id": "T...",
      "shortcut_id": null,
      "app_id": null,
      "app_action_id": "",
      "image_url": null,
      "date_create": 1644956055,
      "date_update": 0,
      "parent_id": ""
    }
  }
  ```
  OBSERVED: ImShyMike/slack-undoc-client src/types.ts:4249-4271 (`BookmarksAddResponse.bookmark` full field list); docs.slack.dev/reference/methods/bookmarks.list (same bookmark shape in list example).
- Pagination: none.
- Used for: the channel bookmarks bar (header) - "Add bookmark" flow, incl. bookmarking a message/canvas/file into the bar.
- Errors: `channel_not_found`, `permission_denied`, `file_already_added` ("The file has already been added to the folder"), `too_many_bookmarks` ("Bookmark limit reached for channel"; channels are limited to 100 bookmarks; `not_found` is NOT in the official errors table - corrected in verification). OBSERVED: docs.slack.dev/reference/methods/bookmarks.add (Errors table incl. `file_already_added`, `too_many_bookmarks`, `permission_denied`); docs.slack.dev/reference/methods/bookmarks.list (Usage info: "Conversations are limited to 100 bookmarks").
- Rate limit: Tier 2. OBSERVED: docs.slack.dev/reference/methods/bookmarks.add.

### bookmarks.edit

- Transport: `POST https://<workspace>.slack.com/api/bookmarks.edit`, form-encoded. OBSERVED: docs.slack.dev/reference/methods/bookmarks.edit.
- Visibility: public (present in the public API but NOT in the current web client method union - see Gotchas).
- Request params:
  - `bookmark_id` - string - bookmark to update ("Required for public channels"). OBSERVED: docs.slack.dev/reference/methods/bookmarks.edit.
  - `channel_id` - string - channel. OBSERVED: docs.slack.dev/reference/methods/bookmarks.edit.
  - `title` - string - optional. OBSERVED: docs.slack.dev/reference/methods/bookmarks.edit.
  - `link` - string - optional. OBSERVED: docs.slack.dev/reference/methods/bookmarks.edit.
  - `emoji` - string - optional. OBSERVED: docs.slack.dev/reference/methods/bookmarks.edit.
  - beeper/slackgo sends title/emoji/link with pointer semantics (empty string clears emoji/title). OBSERVED: beeper/slackgo bookmarks.go:37-41 (EditBookmarkParameters with *string Title/Emoji, `Set to "" to clear` comments) and :156-178 (EditBookmarkContext sends channel_id/bookmark_id + optional link/emoji/title).
- Response: `{"ok": true, "bookmark": {...}}` (same bookmark object as add). OBSERVED: beeper/slackgo bookmarks.go:48-51 (`editBookmarkResponse` with `Bookmark`).
- Pagination: none.
- Used for: rename/re-icon of a bookmark or folder.
- Rate limit: Tier 2. OBSERVED: docs.slack.dev/reference/methods/bookmarks.edit.

### bookmarks.remove

- Transport: `POST https://<workspace>.slack.com/api/bookmarks.remove`, form-encoded. OBSERVED: docs.slack.dev/reference/methods/bookmarks.remove.
- Visibility: public.
- Request params:
  - `channel_id` - string - channel. OBSERVED: beeper/slackgo bookmarks.go:90-103.
  - `bookmark_id` - string - the bookmark (slackgo form). OBSERVED: beeper/slackgo bookmarks.go:90-103. Note: the web client's generated param map uses `bookmark` (plus `type`) instead: `BookmarksRemoveParams { channel, bookmark, type }`. OBSERVED: ImShyMike/slack-undoc-client src/types.ts:377-380.
- Response: `{"ok": true}`. OBSERVED: ImShyMike/slack-undoc-client src/types.ts:4297-4299 (`BookmarksRemoveResponse { ok: true }`).
- Pagination: none.
- Used for: removing a bookmark/folder from the bar.
- Rate limit: Tier 2. OBSERVED: docs.slack.dev/reference/methods/bookmarks.remove.

### bookmarks.list

- Transport: `POST https://<workspace>.slack.com/api/bookmarks.list`, form-encoded. OBSERVED: rusq/slackdump internal/edge/bookmarks.go:58-75 (BookmarksList posts form `bookmarks.list`); .omo/research/findings.md (bookmarks.list fires on fresh client load and after every sidebar channel click).
- Visibility: public.
- Request params (web-client flavor):
  - `channel` - string - channel ID. OBSERVED: rusq/slackdump internal/edge/bookmarks.go:25-30 (`Channel` field), ImShyMike/slack-undoc-client src/types.ts:371-375 (`BookmarksListParams { channel, include_folders, include_legacy_workflows }`).
  - `include_folders` - boolean - include bookmark folders in the result. OBSERVED: rusq/slackdump internal/edge/bookmarks.go:28 (`IncludeFolders bool json:"include_folders"`, sent `true`).
  - `include_legacy_workflows` - boolean - include legacy workflow shortcuts. OBSERVED: rusq/slackdump internal/edge/bookmarks.go:29, sent `true` with `_x_reason: bookmarks-store/conditional-fetching`.
- Response excerpt:
  ```json
  {
    "ok": true,
    "bookmarks": [
      {
        "id": "Bk123ABC4DEF",
        "channel_id": "C1RQ000",
        "title": "bookmark-1",
        "link": "https://google.com",
        "emoji": ":clap:",
        "icon_url": "https://www.google.com/favicon.ico",
        "type": "link",
        "entity_id": null,
        "date_created": 1644956055,
        "date_updated": 0,
        "rank": "g",
        "last_updated_by_user_id": "U0123A1B1C1",
        "last_updated_by_team_id": "T012AB34CDE",
        "shortcut_id": null,
        "app_id": null
      }
    ]
  }
  ```
  OBSERVED: docs.slack.dev/reference/methods/bookmarks.list (Typical success response); rusq/slackdump internal/edge/bookmarks.go:31-53 (Bookmark struct with `app_action_id`, `image_url`, `parent_id` fields beyond the public doc).
- Pagination: none (single response, <=100 entries).
- Used for: channel header bookmarks bar; fetched by the web client on every channel open. OBSERVED: .omo/research/findings.md ("The main calls observed were conversations.history, conversations.listPrefs, bookmarks.list") and .omo/research/gist.md (boot phase "bookmarks.list, saved.list" in Phase 3 channel-specific data).
- Rate limit: Tier 3 (50+/min). OBSERVED: docs.slack.dev/reference/methods/bookmarks.list.

### stars.add

- Transport: `POST https://<workspace>.slack.com/api/stars.add`, form-encoded. OBSERVED: docs.slack.dev/reference/methods/stars.add.
- Visibility: public, DEPRECATED ("Stars can still be added via stars.add but they can no longer be viewed or interacted with by end-users. Additionally, the star_added event is no longer emitted."). OBSERVED: docs.slack.dev/reference/methods/stars.add (Usage info).
- Request params: `channel` (+ `timestamp`, `file`, `file_comment` in the classic API). The web client's param map lists only `channel`. OBSERVED: beeper/slackgo stars.go:43-61; ImShyMike/slack-undoc-client src/types.ts:1957-1959 (`StarsAddParams { channel }`).
- Response: `{"ok": true}`. OBSERVED: beeper/slackgo stars.go:55-60.
- Used for: nothing in the new client - superseded by saved.add ("Later"). OBSERVED: docs.slack.dev/reference/methods/stars.add; yuya373/emacs-slack slack-star.el:33-36 ("right this moved to saved for later https://api.slack.com/changelog/2023-07-its-later-already-for-stars-and-reminders").
- Rate limit: Tier 2. OBSERVED: docs.slack.dev/reference/methods/stars.add.

### stars.remove

- Transport: `POST https://<workspace>.slack.com/api/stars.remove`, form-encoded. OBSERVED: docs.slack.dev/reference/methods/stars.remove.
- Visibility: public, DEPRECATED (same Usage info as stars.add). OBSERVED: docs.slack.dev/reference/methods/stars.remove.
- Request params: same as stars.add. OBSERVED: beeper/slackgo stars.go:63-91; ImShyMike/slack-undoc-client src/types.ts:1961-1963.
- Response: `{"ok": true}`. OBSERVED: beeper/slackgo stars.go:75-90.
- Used for: un-starring legacy items.
- Rate limit: Tier 2. OBSERVED: docs.slack.dev/reference/methods/stars.remove.

### stars.list

- Transport: `GET https://<workspace>.slack.com/api/stars.list`, params as query string (official Method Access shows GET; the mined clients POST the same path - beeper/slackgo stars.go:133 postMethod). OBSERVED: docs.slack.dev/reference/methods/stars.list ("GET https://slack.com/api/stars.list"). CORRECTED in verification: was listed as POST.
- Visibility: public, DEPRECATED ("stars.list will no longer reflect anything new that users are saving"). OBSERVED: docs.slack.dev/reference/methods/stars.list (Usage info).
- Request params: `cursor`, `limit`, `team_id` per official docs (which list `count` (default 100) and `page` too but NO `user` argument); slackgo additionally sends `user` (StarsParameters.User) - treat `user` as slackgo-only/unofficial. OBSERVED: docs.slack.dev/reference/methods/stars.list (Optional arguments: count, cursor, limit, page, team_id); beeper/slackgo stars.go:14-19 and 112-133 (`StarsParameters{User,Cursor,Limit,TeamID}`, sends user/cursor/limit/team_id). CORRECTED in verification: `user` was listed as an official parameter.
- Response excerpt: `{ "ok": true, "items": [ { "type": "message", "channel": "C...", "message": {...}, "date_create": 16560... } ], "response_metadata": { "next_cursor": "..." } }`. OBSERVED: docs.slack.dev/reference/methods/stars.list (Typical success response); beeper/slackgo stars.go:22-26 and 111-133.
- Pagination: cursor (`response_metadata.next_cursor`); slackgo uses a 200 page size. OBSERVED: beeper/slackgo stars.go:231 (`limit: 200, // per slack api documentation.`).
- Used for: legacy "Stars" view. The current web client replaced it with saved.list ("Later") and the sidebar section type `stars` is now a user channel-section type. OBSERVED: .omo/research/findings.md (section types list includes `stars`).
- Rate limit: Tier 3. OBSERVED: docs.slack.dev/reference/methods/stars.list.

### saved.list

- Transport: `POST https://<workspace>.slack.com/api/saved.list`, form-encoded, token in body plus web-client fields. OBSERVED: rusq/slackdump-adjacent implementation korotovsky/slack-mcp-server pkg/provider/edge/saved.go:15-73 (`cl.PostForm(ctx, "saved.list", ...)`); 0xdeafcafe/loafer docs/slack-webapp-methods.md:126-127 (live client calls `saved.list` with `filter=saved`, `limit=50`).
- Visibility: private (web-client only; "Only accessible with browser session tokens (xoxc/xoxd)"). OBSERVED: korotovsky/slack-mcp-server pkg/provider/edge/saved.go:8-9 comment.
- Request params:
  - `filter` - string - which Later tab: observed value `saved` (default in slack-mcp); loafer documents the client filter set `saved`, `todo`, `completed`, `archived`, `todo_overdue`, `todo_overdue_all`, `todo_deleted_messages`, `none` (some marked inferred there - treat archived/todo_overdue* as LEADS-grade). OBSERVED: korotovsky/slack-mcp-server pkg/handler/saved.go:39 (`filter := request.GetString("filter", "saved")`); 0xdeafcafe/loafer docs/slack-webapp-methods.md:120.
  - `limit` - int - page size (slack-mcp uses 50; loafer observed limit=50 single page, "max page likely 30"). OBSERVED: korotovsky/slack-mcp-server pkg/handler/saved.go:40; loafer docs 121,126.
  - `cursor` - string - pagination cursor from previous `response_metadata.next_cursor`. OBSERVED: korotovsky/slack-mcp-server pkg/provider/edge/saved.go:16-20 and pkg/handler/saved.go:60-64; yuya373/emacs-slack slack-star.el:117-130 (slack-stars-list-request posts only `cursor`).
  - `include_tombstones` - boolean - include deleted-message placeholders; slack-mcp sends `true`. OBSERVED: korotovsky/slack-mcp-server pkg/provider/edge/saved.go:21, 70; polaralias/slack-mcp slack_native.py:324-334 (`include_tombstones: "true"`).
  - `_x_reason` - string - `saved-api/savedList`. OBSERVED: polaralias/slack-mcp slack_native.py:328; korotovsky/slack-mcp-server pkg/provider/edge/saved.go:69 (`webclientReason("saved-api/savedList")`).
- Response excerpt:
  ```json
  {
    "ok": true,
    "saved_items": [
      {
        "item_id": "C...",
        "item_type": "message",
        "date_created": 1700000000,
        "date_due": 0,
        "date_completed": 0,
        "date_updated": 1700000001,
        "is_archived": false,
        "date_snoozed_until": 0,
        "ts": "1700000000.000100",
        "state": "...",
        "todo_state": "to_do"
      }
    ],
    "counts": {
      "uncompleted_count": 3,
      "uncompleted_overdue_count": 0,
      "archived_count": 1,
      "completed_count": 5,
      "total_count": 9
    },
    "response_metadata": { "next_cursor": "" }
  }
  ```
  OBSERVED: ImShyMike/slack-undoc-client src/types.ts:19106-19131 (`SavedListResponse` with saved_items/counts/response_metadata.next_cursor; `todo_state` optional at :19119); korotovsky/slack-mcp-server pkg/provider/edge/saved.go:23-50 (matching Go SavedItem/SavedCounts structs - NOTE: its SavedItem has NO `todo_state` field; todo_state is sourced from types.ts:19119 and the loafer response-validator schema, loafer docs:117-118).
- Pagination: cursor-based via `response_metadata.next_cursor`. OBSERVED: korotovsky/slack-mcp-server pkg/handler/saved.go:60-64, 120-123.
- Used for: the "Saved" / Later panel (sidebar "Later" view); fetched at boot alongside bookmarks.list. OBSERVED: .omo/research/gist.md (boot Phase 3 lists `saved.list`).
- Rate limit: unknown (undocumented); slack-mcp applies a Tier-3-style limiter around it. OBSERVED: korotovsky/slack-mcp-server pkg/handler/saved.go:44-48.

### saved.add

- Transport: `POST https://<workspace>.slack.com/api/saved.add`, form-encoded. OBSERVED: yuya373/emacs-slack slack-message.el:47 (`slack-message-stars-add-url "https://slack.com/api/saved.add"`); 0xdeafcafe/loafer docs/slack-webapp-methods.md:133 ("Save: `saved.add` with `item_type=message, item_id=<channel>, ts`").
- Visibility: private (web-client only).
- Request params:
  - `item_type` - string - `message` for message saves. OBSERVED: ImShyMike/slack-undoc-client src/types.ts:1656-1661 (`SavedAddParams { item_type, description, client_id, date_due }`); loafer docs:113 (message-save shape `item_type, item_id(channel), ts, date_due, todo_state`).
  - `item_id` - string - the channel ID for message saves (NOT the message id). OBSERVED: 0xdeafcafe/loafer docs/slack-webapp-methods.md:113; consistent with saved.delete usage in yuya373/emacs-slack slack-star.el (remove passes `ts`, `item_id` from the saved item, `item_type`).
  - `ts` - string - message ts. OBSERVED: loafer docs:113.
  - `date_due` - int - optional due date for "remind me about this" (Later reminder). OBSERVED: ImShyMike/slack-undoc-client src/types.ts:1656-1661; loafer docs:135-137.
  - `description` - string - optional (block-formatted description; `SavedAddResponse.item.description` is an array of block objects). OBSERVED: ImShyMike/slack-undoc-client src/types.ts:19078-19096.
  - `client_id` - string - optional client-generated id (echoed by the `saved_added` socket event's `client_id`). OBSERVED: ImShyMike/slack-undoc-client src/types.ts:1656-1661; 0xdeafcafe/loafer docs/slack-webapp-methods.md:122 (`saved_added` (`e.saved`, `e.client_id`)).
  - `todo_state` - string - optional. OBSERVED: loafer docs:113.
- Response excerpt: `{ "ok": true, "item": { "item_id": "...", "item_type": "message", "date_created": ..., "date_due": ..., "date_completed": ..., "date_updated": ..., "is_archived": false, "date_snoozed_until": ..., "description": [{"type":"...","block_id":"..."}], "state": "...", "ts": "..." } }`. OBSERVED: ImShyMike/slack-undoc-client src/types.ts:19078-19096.
- Pagination: none.
- Used for: "Save this message" (Later) and "Remind me about this" (with date_due) in the modern client - loafer: "the web client's 'remind me about this' is a saved item with a due time now". OBSERVED: 0xdeafcafe/loafer docs/slack-webapp-methods.md:135-137.

### saved.update

- Transport: `POST https://<workspace>.slack.com/api/saved.update`, form-encoded. OBSERVED: korotovsky/slack-mcp-server pkg/provider/edge/saved.go:75-101 (`cl.PostForm(ctx, "saved.update", ...)`).
- Visibility: private.
- Request params:
  - `item_type` - string - `message`. OBSERVED: korotovsky/slack-mcp-server pkg/provider/edge/saved.go:75-78; polaralias/slack-mcp slack_native.py:344-349 (`item_type: "message"`).
  - `item_id` - string - channel ID. OBSERVED: same sources.
  - `ts` - string - message ts. OBSERVED: same sources.
  - `mark` - string - optional - state transition. Observed value: `completed` (slack-mcp server uses it in production to complete a Later item); loafer observed the client sending `uncompleted`/`unarchived` (and pairs `completed`/`archived` presumed there). OBSERVED: korotovsky/slack-mcp-server pkg/handler/saved.go:184-207 (`mark == "completed"` branch); 0xdeafcafe/loafer docs/slack-webapp-methods.md:114, 128.
  - `date_due` - int (unix seconds) - optional - set/clear due date; slack-mcp passes 0 when unset. OBSERVED: korotovsky/slack-mcp-server pkg/provider/edge/saved.go:75-81; polaralias/slack-mcp slack_native.py:350-353.
  - `_x_reason` - string - `saved-api/updateSavedMessage`. OBSERVED: polaralias/slack-mcp slack_native.py:345.
- Response excerpt: `{ "ok": true, "item": { ...same saved-item shape as saved.add... } }`. OBSERVED: ImShyMike/slack-undoc-client src/types.ts:19133-19140 (`SavedUpdateResponse`).
- Pagination: none.
- Used for: mark completed/unread, archive/unarchive, set due date on a Later item. OBSERVED: korotovsky/slack-mcp-server pkg/handler/saved.go:190-191 ("at least one of mark or date_due must be provided").

### saved.delete

- Transport: `POST https://<workspace>.slack.com/api/saved.delete`, form-encoded. OBSERVED: yuya373/emacs-slack slack-message.el:48 (`saved.delete` URL) and slack-star.el slack-star-remove-star (posts `ts`, `item_id`, `item_type`).
- Visibility: private.
- Request params: `item_type` - string (`message`), `item_id` - string (channel ID), `ts` - string (message ts). OBSERVED: ImShyMike/slack-undoc-client src/types.ts:1663-1666 (`SavedDeleteParams { item_type, item_id, ts }`); yuya373/emacs-slack slack-star.el (remove-star request params); loafer docs:129 ("Remove: saved.delete with item_type, item_id, ts").
- Response: `{"ok": true}`. OBSERVED: ImShyMike/slack-undoc-client src/types.ts:19102-19104.
- Pagination: none.
- Used for: "Remove from saved" context action.

### saved.clearCompleted

- Transport: `POST https://<workspace>.slack.com/api/saved.clearCompleted`, form-encoded. OBSERVED: korotovsky/slack-mcp-server pkg/provider/edge/saved.go:95-113 (`cl.PostForm(ctx, "saved.clearCompleted", ...)`).
- Visibility: private.
- Request params: none beyond token + `_x_reason: manually_marked_all_completed`. OBSERVED: korotovsky/slack-mcp-server pkg/provider/edge/saved.go:95-99; polaralias/slack-mcp slack_native.py:356-360.
- Response: `{"ok": true}`. OBSERVED: ImShyMike/slack-undoc-client src/types.ts:19098-19100 (`SavedClearCompletedResponse { ok: true }`).
- Pagination: none.
- Used for: the "Clear completed" button at the bottom of the Later panel.

### reminders.add

- Transport: `POST https://<workspace>.slack.com/api/reminders.add`, form-encoded. OBSERVED: docs.slack.dev/reference/methods/reminders.add.
- Visibility: public; officially in retirement ("Retirement of API methods for interfacing with reminders began March 2023, and as such have become degraded or useless."). OBSERVED: docs.slack.dev/reference/methods/reminders.add (Usage info).
- Request params:
  - `text` - string - required - reminder text. OBSERVED: docs.slack.dev/reference/methods/reminders.add.
  - `time` - string/int - required - "the Unix timestamp (up to five years from now), the number of seconds until the reminder (if within 24 hours), or a natural language description (Ex. 'in 15 minutes,' or 'every Thursday')". OBSERVED: docs.slack.dev/reference/methods/reminders.add (Arguments).
  - `user` - string - optional - "No longer supported - reminders cannot be set for other users". OBSERVED: docs.slack.dev/reference/methods/reminders.add. (beeper/slackgo still sends it for user reminders.) OBSERVED: beeper/slackgo reminders.go:95-114 (AddUserReminder sends `user`).
  - `channel` - string - undocumented; targets the reminder at a channel (channel reminders post in-channel). beeper/slackgo notes "the ability to set reminders on a channel is currently undocumented but has been tested to work". OBSERVED: beeper/slackgo reminders.go:74-91 (AddChannelReminder sends `channel`).
  - `recurrence` - object - optional - `{ "frequency": "weekly", "weekdays": ["monday","wednesday","friday"] }` (daily/weekly/monthly/yearly). OBSERVED: docs.slack.dev/reference/methods/reminders.add (Optional arguments).
  - `team_id` - string - optional - org-level tokens. OBSERVED: docs.slack.dev/reference/methods/reminders.add.
- Response excerpt: `{ "ok": true, "reminder": { "id": "Rm12345678", "creator": "U18888888", "user": "U18888888", "text": "eat a banana", "recurring": false, "time": 1458678068, "complete_ts": 1458678200 } }`. OBSERVED: docs.slack.dev/reference/methods/reminders.info (same reminder object); beeper/slackgo reminders.go:12-20 (Reminder struct).
- Pagination: none.
- Used for: user Slackbot reminders ("/remind", remind me in 20 minutes) and channel reminders. Note the modern web client's per-message "Remind me about this" moved to saved.add with date_due (see saved.add; loafer docs:135-137). libpurple exposes reminders via the `remind` slash commands ("remind [@someone or #channel] to [What] [When]"), i.e. through chat.command, not reminders.add. OBSERVED: dylex/slack-libpurple slack-cmd.c:33-35.
- Rate limit: Tier 2. OBSERVED: docs.slack.dev/reference/methods/reminders.add.

### reminders.list

- Transport: `GET https://<workspace>.slack.com/api/reminders.list`, params as query string (official Method Access shows GET; the mined clients POST the same path - beeper/slackgo reminders.go doReminders postMethod). OBSERVED: docs.slack.dev/reference/methods/reminders.list ("GET https://slack.com/api/reminders.list"). CORRECTED in verification: was listed as POST.
- Visibility: public (retirement notice applies). OBSERVED: docs.slack.dev/reference/methods/reminders.list (Usage info).
- Request params: `token`; official docs also list optional `team_id` (org tokens). slackgo sends only token. OBSERVED: docs.slack.dev/reference/methods/reminders.list (Optional arguments: team_id); beeper/slackgo reminders.go:56-64 (ListRemindersContext sends only token). CORRECTED in verification: was "token only".
- Response: `{ "ok": true, "reminders": [ { ...reminder object... } ] }`. OBSERVED: beeper/slackgo reminders.go:22-25 (`remindersResp`).
- Pagination: none.
- Used for: listing the user's reminders (Slackbot UI).
- Rate limit: Tier 2. OBSERVED: docs.slack.dev/reference/methods/reminders.list.

### reminders.info

- Transport: `GET https://<workspace>.slack.com/api/reminders.info`, params as query string. OBSERVED: docs.slack.dev/reference/methods/reminders.info ("GET https://slack.com/api/reminders.info"). CORRECTED in verification: was listed as POST.
- Visibility: public (retirement notice applies). OBSERVED: docs.slack.dev/reference/methods/reminders.info.
- Request params: `reminder` - string - required - reminder ID (`Rm...`); `team_id` optional. OBSERVED: docs.slack.dev/reference/methods/reminders.info (Arguments).
- Response: `{ "ok": true, "reminder": { "id": "Rm12345678", "creator": "U...", "user": "U...", "text": "...", "recurring": false, "time": 1458678068, "complete_ts": 1458678200 } }` ("only non-recurring reminders will have time and complete_ts field"). OBSERVED: docs.slack.dev/reference/methods/reminders.info (Typical success response).
- Pagination: none.
- Used for: detail view of a reminder.
- Rate limit: Tier 2. OBSERVED: docs.slack.dev/reference/methods/reminders.info.

### reminders.complete

- Transport: `POST https://<workspace>.slack.com/api/reminders.complete`, form-encoded. OBSERVED: docs.slack.dev/reference/methods/reminders.complete.
- Visibility: public (retirement notice applies). OBSERVED: docs.slack.dev/reference/methods/reminders.complete.
- Request params: `reminder` - string - required - "The ID of the reminder to be marked as complete" (`Rm12345678`); `team_id` optional. OBSERVED: docs.slack.dev/reference/methods/reminders.complete (Arguments).
- Response: `{"ok": true}`. OBSERVED: docs.slack.dev/reference/methods/reminders.complete (Typical success response).
- Pagination: none.
- Used for: ticking off a Slackbot reminder.
- Rate limit: Tier 2. OBSERVED: docs.slack.dev/reference/methods/reminders.complete.

### reminders.delete

- Transport: `POST https://<workspace>.slack.com/api/reminders.delete`, form-encoded. OBSERVED: docs.slack.dev/reference/methods/reminders.delete.
- Visibility: public (retirement notice applies). OBSERVED: docs.slack.dev/reference/methods/reminders.delete.
- Request params: `reminder` - string - required - reminder ID. OBSERVED: docs.slack.dev/reference/methods/reminders.delete; beeper/slackgo reminders.go:116-127 (sends `reminder`).
- Response: `{"ok": true}`. OBSERVED: docs.slack.dev/reference/methods/reminders.delete.
- Pagination: none.
- Used for: deleting a Slackbot reminder.
- Rate limit: Tier 2. OBSERVED: docs.slack.dev/reference/methods/reminders.delete.

## Websocket events

The web client socket (wss://wss-primary.slack.com/...) delivers these area events; payload shapes below. The client also receives message updates carrying a full `message.reactions` array (see Gotchas).

- `reaction_added` - payload: `{ "type": "reaction_added", "user": "U...", "item_user": "U...", "item": { "type": "message", "channel": "C...", "ts": "..." }, "reaction": "thumbsup", "event_ts": "..." }`. For file items: `item.file`; for file comments: `item.file_comment`. Fires when any user adds a reaction to an item your client can see ("sent to all connected clients for users who can see the content that was reacted to"). OBSERVED (payload struct): beeper/slackgo websocket_reactions.go:3-22 (ReactionItem `{type, channel, file, file_comment, ts}` + ReactionEvent fields); wee-slack slack/slack_workspace.py:755, 829-836 (handles reaction_added/reaction_removed for messages); mautrix/slack pkg/connector/handleslack.go:316-321 (consumes *slack.ReactionAddedEvent / ReactionRemovedEvent); docs.slack.dev/reference/events/reaction_added ("When a reaction is added to an item...").
- `reaction_removed` - same shape as reaction_added. OBSERVED: beeper/slackgo websocket_reactions.go:21-22; docs.slack.dev/reference/events/reaction_removed.
- `pin_added` - payload: `{ "type": "pin_added", "user": "U...", "item": { ...pinned item... }, "channel_id": "C...", "event_ts": "...", "has_pins": true }` - note the channel field is literally `channel_id` in this event (unlike the API's `channel`), and `has_pins` signals other pins remain. OBSERVED: beeper/slackgo websocket_pins.go:3-13 (pinEvent with `json:"channel_id"` and `json:"has_pins,omitempty"`); docs.slack.dev/reference/events/pin_added ("sent to all members of that channel").
- `pin_removed` - same shape; "The has_pins property indicates that there are other pinned items in that channel". OBSERVED: beeper/slackgo websocket_pins.go:12-13; docs.slack.dev/reference/events/pin_removed.
- `star_added` / `star_removed` - payload: `{ "type": "star_added", "user": "U...", "item": { ...starred item... }, "event_ts": "..." }`. LEGACY: "the star_added event is no longer emitted" since the stars retirement. OBSERVED: beeper/slackgo websocket_stars.go:3-11 (StarAddedEvent struct); docs.slack.dev/reference/events/star_added, star_removed; docs.slack.dev/reference/methods/stars.add (Usage info).
- `saved_added` - carries `{ saved: <saved item>, client_id: <echo of saved.add client_id> }`. OBSERVED: 0xdeafcafe/loafer docs/slack-webapp-methods.md:122 ("RTM: saved_added (e.saved, e.client_id), saved_updated, saved_deleted, saved_clear, saved_due").
- `saved_updated` - fires on any saved.update. OBSERVED: loafer docs:122.
- `saved_deleted` - fires on saved.delete. OBSERVED: loafer docs:122.
- `saved_clear` - fires on saved.clearCompleted. OBSERVED: loafer docs:122 (and the client simply refetches the list rather than diffing: loafer docs:130).
- `saved_due` - fires when a Later item's date_due elapses (the "remind me" ding). OBSERVED: loafer docs:122, 136 ("saved_reminder / saved_due are its events").

## Gotchas

- Transport/auth for this area: every method takes `token` in the request body (xoxc-); the `d` cookie (xoxd-) rides along as `Cookie: d=...`; multipart/form-data is also accepted (libpurple posts multipart with a `token` part). OBSERVED: .omo/research/gist.md (Identity & Auth section); dylex/slack-libpurple slack-api.c (slack_api_encode_post_request).
- Skin tone encoding: `name` may append `::skin-tone-N` with N in 2..6, e.g. `thumbsup::skin-tone-6`, `wave::skin-tone-3`; the reaction is stored under the base emoji with the modifier. Skin-tone-1 (default yellow) has no suffix. OBSERVED: docs.slack.dev/reference/methods/reactions.add (Usage info: "name may indicate a modifier by appending ::skin-tone- and a number from 2 to 6"); mautrix/slack pkg/emoji/emoji-generate.go:87-104 (skinToneIDs map: U+1F3FB->2, U+1F3FC->3, U+1F3FD->4, U+1F3FE->5, U+1F3FF->6, joined as `skin-tone-N`, multi-modifier e.g. `skin-tone-3-5` impossible here - single tone only); wee-slack slack/slack_emoji.py:14-20 (EmojiSkinVariation keyed "1".."5" from weemoji.json for rendering).
- Idempotency/retries: reactions.add fails with `already_reacted` and pins.add with `already_pinned` if the mutation is replayed - treat those as success-equivalent. OBSERVED: docs.slack.dev/reference/methods/reactions.add, pins.add (Errors tables).
- The current Gantry web client's method registry (extracted type union) contains `reactions.add`, `reactions.remove`, `pins.add/list/remove`, `bookmarks.add/list/remove`, `saved.*`, `stars.add/remove` - but NOT `reactions.get`, `reactions.list`, `stars.list`, `bookmarks.edit`, `reminders.*`. It reads reactions from `message.reactions` embedded in message payloads/updates instead of re-fetching per item. OBSERVED: ImShyMike/slack-undoc-client src/types.ts:24784-25072 (method union) and 25256-25544 (map).
- Stars retirement: stars.* still function but are invisible to end users; `star_added` is no longer emitted; the user-facing feature is "Later" (saved.*). Migration changelog: api.slack.com/changelog/2023-07-its-later-already-for-stars-and-reminders. OBSERVED: docs.slack.dev/reference/methods/stars.add/stars.list (Usage info); yuya373/emacs-slack slack-star.el:33-36.
- Reminders retirement: docs mark the reminders.* family "degraded or useless" since March 2023, yet the endpoints still work for self-reminders; the modern per-message "Remind me about this" is saved.add with date_due, not reminders.add. OBSERVED: docs.slack.dev/reference/methods/reminders.complete (Usage info); 0xdeafcafe/loafer docs/slack-webapp-methods.md:135-137.
- `saved.update` requires at least one of `mark` or `date_due`; `mark` values observed: `completed`, `uncompleted`, `unarchived` (archived presumed). OBSERVED: korotovsky/slack-mcp-server pkg/handler/saved.go:190-191; loafer docs:114.
- `saved.list` item identity is (item_id=channel, ts), not a separate id; delete/update need the same triple. OBSERVED: korotovsky/slack-mcp-server pkg/handler/saved.go:48-55, 184-194; loafer docs:129.
- Bookmarks bar limits: 100 bookmarks per conversation; folders are returned by `bookmarks.list` only with `include_folders=true` and entries inside a folder carry `parent_id`. OBSERVED: docs.slack.dev/reference/methods/bookmarks.list (Usage info); rusq/slackdump internal/edge/bookmarks.go:28, 53 (ParentID field).
- Web-client query params on all /api calls: `_x_id`, `_x_csid`, `slack_route` (Enterprise Grid routing `E...:E...`), `_x_version_ts`, `_x_gantry=true`, `_x_reason`, `_x_mode=online`, etc. OBSERVED: .omo/research/gist.md (query param listing + "Every API call includes a reason tag").
- Rate limits (public methods): reactions.add T3, reactions.remove/get/list T2/T3/T2, pins.* T2, bookmarks.add T2 / list T3 / edit/remove T2, stars.add/remove T2 / list T3, reminders.* T2. On 429 `ratelimited`, honor Retry-After. OBSERVED: docs.slack.dev pages per method (each "Rate Limits" line above); dylex/slack-libpurple slack-api.c (api_cb requeues calls on `ratelimited`).
- Enterprise Grid: pass `team_id` where the method accepts it (reactions.list, stars.list, reminders.*) when using org-level tokens; the workspace host is `<workspace>.enterprise.slack.com` with `slack_route` routing. OBSERVED: docs.slack.dev pages (team_id arguments); .omo/research/gist.md (Enterprise Grid Routing).
- Quick-reaction chips in the hover bar come from cross-area `search.autocomplete.topEmojis` at boot ("React with white_check_mark / eyes / raised_hands ... fetched at boot via search.autocomplete.topEmojis"). OBSERVED: .omo/research/gist.md ("Quick reactions are server-personalized").
- Reacji/workflow triggers on channel open are cross-area `conversations.bulkReacjiTriggers` (fires whenever you open a channel). OBSERVED: .omo/research/gist.md ("Reacji triggers are checked on every channel load"); .omo/research/findings.md (bootstrap call list).
- The Later panel badge count is delivered by cross-area `client.counts` (response carries a `saved` object). OBSERVED: 0xdeafcafe/loafer docs/slack-webapp-methods.md:228, 122-area ("client.counts response has a saved object").
- Reaction-picker personalization may come from a `users.prefs` `emoji_use` map (loafer marks this UNCERTAIN - LEADS-grade). OBSERVED: 0xdeafcafe/loafer docs/slack-webapp-methods.md:309 (explicitly flagged uncertain).
- The web client optimistically updates reaction chips and pins locally; reaction/pin events are the reconciliation path (mautrix applies them directly). OBSERVED: mautrix/slack pkg/connector/handleslack.go:316-321, 389-391.

## LEADS

- Extra saved.* methods present in the web client registry but not implemented by any mined client: `saved.get`, `saved.markCompleted`, `saved.bulkUpdate`, `saved.bulkDelete` (`saved.bulkDelete` seen with `args:{delete_all:!0, filter:<filter>}`). Params unobserved. Source: 0xdeafcafe/loafer docs/slack-webapp-methods.md:107-111.
- `saved.list` filter values `todo`, `archived`, `todo_overdue`, `todo_overdue_all`, `todo_deleted_messages`, `none` are from loafer's teardown (some marked inferred there); only `saved` is corroborated by slack-mcp. Source: loafer docs:120; korotovsky/slack-mcp-server pkg/handler/saved.go:39.
- `todo_state` enum observed by loafer as `["completed","saved","to_do"]` on saved items - verify against live traffic. Source: loafer docs:118.
- `saved.add` `date_due` unit: loafer guesses unix seconds (same scale as `saved.list`'s date_due reads); confirm whether the API wants seconds or ms. Source: loafer docs:135.
- Duplicate-save behavior: loafer expects an `already_saved`-class error code for re-saving; error codes for saved.* are otherwise unobserved. Source: loafer docs:133-134.
- `saved.list` responses occasionally use `items` instead of `saved_items` as the array key per loafer; slack-mcp structs only show `saved_items`. Source: loafer docs:127.
- `bookmarks.remove` param naming conflict: web client map says `bookmark` (+`type`), slackgo sends `bookmark_id`; possibly both accepted. Source: ImShyMike/slack-undoc-client src/types.ts:377-380 vs beeper/slackgo bookmarks.go:90-103.
- Bookmark `type` vocabulary beyond `link` (message/file/canvas/folder task-bookmark types like `channel_task_bookmark_completed`, `folder_bookmark_id` in shortcut payloads) seen in prefs/feature flags but not tied to bookmarks.add calls. Source: ImShyMike/slack-undoc-client src/types.ts:17888, 2741-2752, 13080.
- Slack-native "remind me about this message" historically (pre-2023) used `reminders.add` with a message reference in `text`; no mined client captured that exact call shape - the modern path is saved.add+date_due. Source: loafer docs:135-137.
- `reminders.add` natural-language `time` parsing surface ("in 15 minutes", "every Thursday") - the desktop client can just pass the string; server-side Slackbot parses it. Source: docs.slack.dev/reference/methods/reminders.add.
- Legacy `pins.list` `Paging` block and `stars.list`/`reactions.list` limits (slackgo uses limit=200 for stars) - confirm current max page sizes. Source: beeper/slackgo pins.go:8-12, stars.go:231.
- `users.prefs` `emoji_use` map as reaction-picker ranking source (loafer: "UNCERTAIN, not used yet"). Source: loafer docs:309.
- emacs-slack's slack-reminder.el "remind from message" flow is dead/stub code that calls star-add with a ms duration - do not use as a reference. Source: yuya373/emacs-slack slack-reminder.el (slack-reminder-add-from-message calls slack-buffer-add-star with due-in-ms).
