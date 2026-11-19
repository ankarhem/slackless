# Channel management and sidebar - Slack web-client API

_Evidence pins: line numbers in `OBSERVED:` tags refer to rusq/slackdump@fec2e68, beeper/slackgo@d0ae74c (the slack-go fork that mautrix/slack/go.mod:48 substitutes for slack-go), mautrix/slack@38d69d5, wee-slack/wee-slack@aa904c2, emacs-slack/emacs-slack@eafa2d4, dylex/slack-libpurple@1115ec1, 3kh0/slack-datamine@27df832 (method/arg/event names datamined from the web client bundle), gammons/slk@78a0177, tammersaleh/slack-cli@613f9e2, 0xdeafcafe/loafer@9af0863, some-du6e/clack@e93a5e7, andersfischernielsen/Lax@e0e23e2, 3kh0/super-platinum@58e7455, karbassi/slack-mcp@7abc8af and matthewdonsemail-lab/log@43c0f35. `ankarhem/slackless/.omo/research/*` are this repo's live web-client teardowns. docs.slack.dev pages were fetched 2026-11-19; where a subsection says "docs 404" the method's docs.slack.dev page returned HTTP 404 that day (control `conversations.list` returned 200)._

## Summary

- Scope: listing/discovering conversations, channel lifecycle (create, join, leave, archive, rename, topic, purpose, privacy), membership, DM/group-DM open and close, the read cursor, server-side sidebar sections, per-channel notification and mute, stars, Slack Connect, channel canvases/tabs, and Enterprise Grid org-wide fields.
- Public (documented, work with `xoxc` + `d`): `conversations.{list,info,create,join,leave,archive,unarchive,rename,setTopic,setPurpose,invite,kick,members,open,close,mark,inviteShared,acceptSharedInvite,listConnectInvites,canvases.create}`, `users.conversations`, `stars.*`.
- Private (web client only): sidebar model `users.channelSections.*`, notification writes `users.prefs.setNotifications`, `users.prefs.get/set`, `conversations.listPrefs/genericInfo/teamConnections`, tab/property methods, browser `search.modules.channels`, `client.dms`, edge `channels/{info,membership,search}`.
- Risk 1: section writes answer `{"ok":true}` even when nothing changed, `users.channelSections.set` clears every key you omit, and REST vs websocket name section fields differently.
- Risk 2: Enterprise Grid splits methods between the org-level and member-workspace tokens (some fail with `team_is_restricted` / `internal_error`).
- Risk 3: stars are dead for end users (Starred is now a section), and `conversations.listPrefs` returns channel posting permissions, not notification prefs.

## Endpoints

Conventions that apply to every subsection below:

- Web API: `POST https://<ws>.slack.com/api/<method>` (`<ws>.enterprise.slack.com` on Grid); the official client sends multipart form-data with `token` in the body. OBSERVED: ankarhem/slackless/.omo/research/gist.md:77-79; rusq/slackdump/internal/edge/edge.go:90. An urlencoded form with `token` (plus `Cookie: d=…`) works too. OBSERVED: gammons/slk/internal/slack/client.go:1906-1925; beeper/slackgo/misc.go:460-466
- Optional attribution fields the official client adds: `_x_reason`, `_x_mode=online`, `_x_sonic=true`, `_x_app_name=client`. OBSERVED: rusq/slackdump/internal/edge/edge.go:433-446
- Edge API: `POST https://edgeapi.slack.com/cache/<team-or-enterprise-id>/<resource>/<action>`, JSON body including `token`. OBSERVED: rusq/slackdump/internal/edge/edge.go:91; ankarhem/slackless/.omo/research/gist.md:109
- "Used for (slackless)" lines are this project's feature mapping, not claims about Slack.

| Client feature                   | Methods                                                                                                                 |
| -------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| Sidebar build                    | `client.userBoot` + `client.counts` (cross-area), `users.channelSections.list`, `users.prefs.get`, edge `channels/info` |
| Channel browser / quick switcher | `search.modules.channels` (fallback `conversations.list`), edge `channels/search`                                       |
| DMs                              | `client.dms`, `conversations.open`, `conversations.close`                                                               |
| Lifecycle                        | `conversations.create/join/leave/archive/unarchive/rename/setTopic/setPurpose/convertToPrivate`                         |
| Membership                       | `conversations.invite/kick/members`, edge `channels/membership`, `conversations.joinRequests.*`                         |
| Sections                         | `users.channelSections.*`                                                                                               |
| Mute / notifications             | `users.prefs.setNotifications`, `users.prefs.get/set`                                                                   |
| Slack Connect                    | `conversations.inviteShared/acceptSharedInvite/teamConnections`, invite lists                                           |
| Canvases / tabs                  | `conversations.canvases.create`, tab and property methods                                                               |

### conversations.list

- **Transport:** `POST /api/conversations.list`, form. OBSERVED: dylex/slack-libpurple/slack-conversation.c:19
- **Visibility:** public, rate Tier 2. OBSERVED: https://docs.slack.dev/reference/methods/conversations.list
- **Request:** OBSERVED: https://docs.slack.dev/reference/methods/conversations.list
  - `types` - csv string - optional - any of `public_channel,private_channel,mpim,im`; default `public_channel`.
  - `exclude_archived` - bool - optional - default `false`.
  - `limit` - int - optional - default 100, must be under 1000.
  - `cursor` - string - optional - previous `response_metadata.next_cursor`.
  - `team_id` - string - optional - only for org-level tokens; ignored on workspace tokens.
  - The web client's registry lists only `exclude_archived, limit, types`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:334
- **Response:** OBSERVED: https://docs.slack.dev/reference/methods/conversations.list
  ```json
  {"ok":true,"channels":[{"id":"C012AB3CD","name":"general","is_channel":true,"is_private":false,
    "is_member":true,"is_archived":false,"is_ext_shared":false,"is_org_shared":false,"num_members":4,
    "topic":{"value":"…","creator":"","last_set":0},"purpose":{…},"previous_names":[]}],
   "response_metadata":{"next_cursor":"…"}}
  ```
- **Pagination:** cursor. Filters like `exclude_archived` are applied after each virtual page is cut, so a page can hold fewer than `limit` rows (even zero) while a cursor remains. No sort argument exists. OBSERVED: https://docs.slack.dev/reference/methods/conversations.list
- **Used for (slackless):** full public-channel directory (browser fallback). loafer browses with `types=public_channel&exclude_archived=true&limit=200` OBSERVED: 0xdeafcafe/loafer/internal/slack/manage.go:20-33; emacs-slack sends `limit=999` OBSERVED: emacs-slack/emacs-slack/slack-conversations.el:357-362

### users.conversations

- **Transport:** `POST /api/users.conversations`, form. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:282-300
- **Visibility:** public, Tier 3. OBSERVED: https://docs.slack.dev/reference/methods/users.conversations
- **Request:** `types` (csv, default `public_channel`), `exclude_archived` (bool, default false), `exclude_muted` (bool, default false), `limit` (default 100), `cursor`, `team_id` (org-level tokens), `user` (another user's public memberships). OBSERVED: https://docs.slack.dev/reference/methods/users.conversations
- **Response:** `{"ok":true,"channels":[<conversation objects>],"response_metadata":{"next_cursor":"…"}}` OBSERVED: https://docs.slack.dev/reference/methods/users.conversations. DM rows carry no `is_open`. OBSERVED: some-du6e/clack/docs/SIDEBAR-RULES.md:11
- **Pagination:** cursor. slk asks for all four types with `limit=200`, `exclude_archived=true` OBSERVED: gammons/slk/internal/slack/client.go:681-690; mautrix pages with `types/limit/cursor` OBSERVED: mautrix/slack/pkg/connector/client.go:615-619
- **Used for (slackless):** "my conversations" when boot data is unavailable. The official client's fresh load did not call it. OBSERVED: ankarhem/slackless/.omo/research/findings.md:112. Grid: only the member-workspace token may call it. OBSERVED: gammons/slk/internal/slack/client.go:356-362

### conversations.info

- **Transport:** `POST /api/conversations.info`, form. OBSERVED: dylex/slack-libpurple/slack-channel.c:228
- **Visibility:** public, Tier 3. OBSERVED: https://docs.slack.dev/reference/methods/conversations.info
- **Request:** `channel` (required), `include_locale` (bool), `include_num_members` (bool, default false). OBSERVED: https://docs.slack.dev/reference/methods/conversations.info. Web-client extras: `display_counts`, `no_members`, `return_app_home`, `timezone_count`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:325
- **Response:** top level per docs OBSERVED: https://docs.slack.dev/reference/methods/conversations.info; `properties` shape OBSERVED: beeper/slackgo/conversation.go:73-104
  ```json
  {
    "ok": true,
    "channel": {
      "id": "C012AB3CD",
      "name": "general",
      "is_private": false,
      "is_archived": false,
      "is_org_shared": false,
      "context_team_id": "T123ABC456",
      "updated": 1723130875818,
      "creator": "U123ABC456",
      "properties": {
        "canvas": { "file_id": "F…", "is_empty": false, "quip_thread_id": "…" },
        "tabs": [{ "id": "…", "label": "…", "type": "…" }],
        "posting_restricted_to": { "type": ["…"], "user": ["U…"] },
        "threads_restricted_to": { "type": [], "user": [] }
      }
    }
  }
  ```
- **Pagination:** none.
- **Used for (slackless):** channel details; re-read after `channel_updated` (mautrix invalidates and resyncs) OBSERVED: mautrix/slack/pkg/connector/handleslack.go:377-380. `posting_restricted_to.type` containing `admin` means admin-only posting. OBSERVED: mautrix/slack/pkg/connector/chatinfo.go:320

### conversations.genericInfo

- **Transport:** `POST /api/conversations.genericInfo`, form. OBSERVED: rusq/slackdump/internal/edge/conversations.go:62
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/conversations.genericInfo; 3kh0/slack-datamine/build/api-methods.txt:483
- **Request:**
  - `updated_channels` - JSON-object string - `{"<channel_id>":<last-known updated>}`; slackdump sends `0` per id to get full records. OBSERVED: rusq/slackdump/internal/edge/conversations.go:30-33,49,59
  - `channels` - registry-listed; semantics unobserved. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:322
  - slackdump uses `_x_reason=fallback:UnknownFetchManager`. OBSERVED: rusq/slackdump/internal/edge/conversations.go:60
- **Response:** `{"ok":true,"channels":[<full conversation objects>],"unchanged_channel_ids":["C…"]}` OBSERVED: rusq/slackdump/internal/edge/conversations.go:36-40
- **Pagination:** none; batch by id list.
- **Used for (slackless):** hydrate conversations missing from boot. slackdump uses it for group DMs that only `client.counts` mentions. OBSERVED: rusq/slackdump/internal/edge/slacker.go:237-260. The official client calls it during boot. OBSERVED: ankarhem/slackless/.omo/research/findings.md:62

### edge: channels/info

- **Transport:** `POST https://edgeapi.slack.com/cache/<id>/channels/info`, JSON (the official client sent `Content-Type: text/plain`). OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2258; ankarhem/slackless/.omo/research/gist.md:119
- **Visibility:** private (edge cache API). OBSERVED: ankarhem/slackless/.omo/research/gist.md:109
- **Request:** OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2258
  - `token` - string - required.
  - `updated_ids` - object id→last-known `updated` - required - conditional fetch; batches of 1 to 63 ids were observed. Also OBSERVED: ankarhem/slackless/.omo/research/gist.md:143
  - `check_membership` - bool - optional.
  - `enterprise_token` - required on Enterprise Grid. OBSERVED: karbassi/slack-mcp/docs/undocumented-endpoints.md:81
- **Response:** 43 fields per result. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2258
  ```json
  {"ok":true,"results":[{"id":"C…","name":"…","enterprise_id":"","context_team_id":"T…","internal_team_ids":[],
    "shared_team_ids":["T…"],"connected_team_ids":[],"connected_limited_team_ids":[],"pending_connected_team_ids":[],
    "conversation_host_id":"","is_org_default":false,"is_org_mandatory":false,"is_ext_shared":false,"is_ext_ws_shared":false,
    "is_record_channel":false,"is_file":false,"is_moved":0,"use_case":"","updated":0,
    "properties":{"canvas":…,"tabs":…,"tabz":…,"default_tab_id":…,"auto_open_tab_id":…,"who_can_manage_tabs":…}}],
   "member_channels":["C…"],"failed_ids":["C…"]}
  ```
- **Pagination:** none. An id absent from `results` is unchanged unless it is listed in `failed_ids`. `check_membership:true` adds top-level `member_channels` and does not add `is_member` to results. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2258
- **Used for (slackless):** lazy hydration of sidebar rows and channel headers.

### edge: channels/search

- **Transport:** `POST https://edgeapi.slack.com/cache/<id>/channels/search`, JSON. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2725
- **Visibility:** private. OBSERVED: ankarhem/slackless/.omo/research/gist.md:495
- **Request:** `query`, `count` (30 observed), `fuzz` (1), `include_record_channels` (bool), `top_channels` (array of 22 ids observed), `default_workspace` (`T…`), `check_membership` (bool). OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2725
- **Response:** `{"ok":true,"results":[<channel records as channels/info>],"member_channels":[…]}` OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2725
- **Pagination:** none observed.
- **Used for (slackless):** the channel half of the Cmd+K switcher, which runs on the edge API. OBSERVED: ankarhem/slackless/.omo/research/gist.md:624. The search-area doc owns ranking.

### search.modules.channels

- **Transport:** `POST /api/search.modules.channels`, form. OBSERVED: rusq/slackdump/internal/edge/search.go:164-167
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/search.modules.channels
- **Request** (slackdump's "browse channels" call). OBSERVED: rusq/slackdump/internal/edge/search.go:45-72,130-161
  - `module=channels` - required; `query` - string - `""` lists all.
  - `cursor` - `*` for the first page, then `pagination.next_cursor`. OBSERVED: rusq/slackdump/internal/edge/search.go:140,185
  - `count` - 100; `page` - omitted when 0. OBSERVED: rusq/slackdump/internal/edge/search.go:31,50,135,143
  - `sort` - `name` | `recommended`; `sort_dir` - `asc` | `desc`. OBSERVED: rusq/slackdump/internal/edge/search.go:88-92,96-99,150-151
  - `channel_type` - `""` (all) | `private` | `archived` | `external_shared` | `exclude_archived` | `private_exclude`. OBSERVED: rusq/slackdump/internal/edge/search.go:78-83
  - `exclude_my_channels` (0/1), `search_only_my_channels` (bool). OBSERVED: rusq/slackdump/internal/edge/search.go:66-68
  - Fixed values: `browse=standard`, `search_context=desktop_channel_browser`, `recommend_source=channel-browser`, `client_req_id` and `browse_session_id` (UUIDs), `extracts=0`, `highlight=0`, `no_user_profile=1`, `include_files_shares=1`, `max_filter_suggestions=10`, `_x_reason=browser-query`. OBSERVED: rusq/slackdump/internal/edge/search.go:130-161
  - Web-client registry: `browse, channel_type, count, cursor, exclude_my_channels, module, query, sort, sort_dir`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:689
- **Response:** OBSERVED: rusq/slackdump/internal/edge/assets/search.module.channels.json:1-25
  ```json
  {
    "ok": true,
    "module": "channels",
    "query": "",
    "filters": {},
    "pagination": {
      "total_count": 6,
      "page": 1,
      "per_page": 50,
      "first": 1,
      "last": 6
    },
    "items": [
      {
        "iid": "87914a28-…",
        "id": "CHY5HUESG",
        "is_member": false,
        "member_count": 2,
        "name": "everyone",
        "purpose": {
          "value": "…",
          "creator": "UHSD97ZA5",
          "last_set": 1555493778
        }
      }
    ]
  }
  ```
- **Pagination:** follow `pagination.next_cursor` until it is empty. OBSERVED: rusq/slackdump/internal/edge/search.go:179-185. Page-number paging past page 100 wraps back to page 1. OBSERVED: rusq/slackdump/internal/edge/edge.go:456-463
- **Used for (slackless):** sorted, filtered channel browser. It is also part of the full-search fan-out. OBSERVED: ankarhem/slackless/.omo/research/gist.md:505. In JS the browse thunk names its filters `search_channel_types` (e.g. `["exclude_archived","org_wide"]`) and `team_ids`. OBSERVED: 0xdeafcafe/loafer/docs/slack-webapp-methods.md:202

### client.dms

- **Transport:** `POST /api/client.dms`, form. OBSERVED: rusq/slackdump/internal/edge/client.go:146
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/client.dms
- **Request:** `count` (slackdump: 250), `include_closed` (bool), `include_channel` (bool), `exclude_bots` (bool), `cursor`. OBSERVED: rusq/slackdump/internal/edge/client.go:78-86,136-141. Also `priority_mode`. OBSERVED: karbassi/slack-mcp/docs/undocumented-endpoints.md:19. slackdump sends `_x_reason=dms-tab-populate`. OBSERVED: rusq/slackdump/internal/edge/client.go:141
- **Response:** OBSERVED: rusq/slackdump/internal/edge/client.go:88-115
  ```json
  {"ok":true,"ims":[{"id":"D…","latest":"1710632873.037269",
     "channel":{"id":"D…","user":"U…","is_im":true,"is_open":true,"is_archived":false,"is_frozen":false,
       "is_org_shared":false,"context_team_id":"T…","created":0,"updated":0,"last_read":"…","latest":"…"}}],
   "mpims":[…],"response_metadata":{"next_cursor":"…"}}
  ```
- **Pagination:** `response_metadata.next_cursor`. OBSERVED: rusq/slackdump/internal/edge/client.go:158-161
- **Used for (slackless):** DMs tab, including closed DMs, with per-DM `is_open`.

### im.list / mpim.list / sidebar.dms

- **Transport:** `POST /api/im.list` and `/api/mpim.list`, form. OBSERVED: rusq/slackdump/internal/edge/dms.go:57; rusq/slackdump/internal/edge/mpims.go:55
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/im.list; https://docs.slack.dev/reference/methods/mpim.list
- **Request:** `im.list` takes `get_latest`, `get_read_state` and `cursor`. OBSERVED: rusq/slackdump/internal/edge/dms.go:25-31. It also takes `limit`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:545. `mpim.list` takes `get_latest` and `cursor`. OBSERVED: rusq/slackdump/internal/edge/mpims.go:25-30
- **Response:** `im.list` returns `{"ok":true,"ims":[{"id":"D…","user":"U…","is_open":true,…}]}`. OBSERVED: rusq/slackdump/internal/edge/dms.go:33-36. `mpim.list` puts group DMs under `groups`, not `mpims`. OBSERVED: rusq/slackdump/internal/edge/mpims.go:32-35
- **Pagination:** `response_metadata.next_cursor`. OBSERVED: rusq/slackdump/internal/edge/dms.go:69-72; rusq/slackdump/internal/edge/mpims.go:67-70
- **`sidebar.dms`:** a registered method (OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:1148). Its `is_open` set matches `client.userBoot`'s. OBSERVED: some-du6e/clack/docs/SIDEBAR-RULES.md:10. Its args are unobserved.
- **Used for (slackless):** DM directory fallbacks. The `_x_reason` values `guided-search-people-empty-state` and `external-connections-browser-conversation-counts` show where the official client calls them. OBSERVED: rusq/slackdump/internal/edge/dms.go:47; rusq/slackdump/internal/edge/mpims.go:45

### conversations.create

- **Transport:** `POST /api/conversations.create`, form. OBSERVED: beeper/slackgo/conversation.go:584-592; beeper/slackgo/channels.go:29-31
- **Visibility:** public, Tier 2. OBSERVED: https://docs.slack.dev/reference/methods/conversations.create
- **Request:** `name` - string - required - lowercase letters, numbers, hyphens and underscores, at most 80 chars. `is_private` - bool - optional. OBSERVED: https://docs.slack.dev/reference/methods/conversations.create. `team_id` is sent for org-level tokens. OBSERVED: beeper/slackgo/conversation.go:584-591
- **Response:** `{"ok":true,"channel":{"id":"C0EAQDV4Z","name":"endeavor","is_channel":true,"is_member":true,"is_private":false,"creator":"U0123456","last_read":"0000000000.000000",…}}` OBSERVED: https://docs.slack.dev/reference/methods/conversations.create
- **Errors:** `name_taken`, `invalid_name`, `invalid_name_maxlength`, `invalid_name_punctuation`, `invalid_name_specials`, `restricted_action`, `cannot_create_channel`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.create
- **Pagination:** n/a.
- **Used for (slackless):** "Create channel". mautrix creates the channel, invites the initial members, then sets the topic. OBSERVED: mautrix/slack/pkg/connector/startchat.go:145-172

### conversations.validateName

- **Transport:** `POST /api/conversations.validateName`, form.
- **Visibility:** private (web-client registry). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:536
- **Request:** `name`, `team_id`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:360
- **Response:** unobserved (see LEADS).
- **Used for (slackless):** live name check in the create and rename dialogs.

### conversations.join

- **Transport:** `POST /api/conversations.join`, form. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:417-423
- **Visibility:** public, Tier 3. OBSERVED: https://docs.slack.dev/reference/methods/conversations.join
- **Request:** `channel` (required). OBSERVED: https://docs.slack.dev/reference/methods/conversations.join. Web-client extra: `request_in_background`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:327
- **Response:** the `channel` object comes from the docs. OBSERVED: https://docs.slack.dev/reference/methods/conversations.join. The `warning` fields come from slack-go. OBSERVED: beeper/slackgo/conversation.go:979-999
  ```json
  {"ok":true,"channel":{"id":"C061EG9SL","name":"general","is_member":true,…},
   "warning":"…","response_metadata":{"warnings":["…"]}}
  ```
- **Errors:** `channel_is_limited_access`, `is_archived`, `method_not_supported_for_channel_type`, `app_cannot_join_channel`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.join
- **Used for (slackless):** "Join" from the browser or a channel preview. Joining a channel you are already in succeeds as a no-op. OBSERVED: gammons/slk/internal/slack/client.go:780-783. A temporary "Join…" membership carries `expiration_ts` and lands in the Temporary channels section. OBSERVED: some-du6e/clack/docs/SIDEBAR-RULES.md:57

### conversations.leave

- **Transport:** `POST /api/conversations.leave`, form. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:433-439
- **Visibility:** public, Tier 3. OBSERVED: https://docs.slack.dev/reference/methods/conversations.leave
- **Request:** `channel` (required). OBSERVED: https://docs.slack.dev/reference/methods/conversations.leave; emacs-slack/emacs-slack/slack-conversations.el:213-221
- **Response:** `{"ok":true}`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.leave. slack-go surfaces a not-in-channel flag from the same response. OBSERVED: beeper/slackgo/conversation.go:649-663
- **Errors:** `cant_leave_general`, `last_member`, `is_archived`, `method_not_supported_for_channel_type`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.leave
- **Used for (slackless):** "Leave channel". For bulk leave, see `conversations.bulkLeave` below.

### conversations.archive / conversations.unarchive

- **Transport:** `POST /api/conversations.archive` and `/api/conversations.unarchive`, form. OBSERVED: beeper/slackgo/conversation.go:224-230,250-255
- **Visibility:** public, Tier 2 each. OBSERVED: https://docs.slack.dev/reference/methods/conversations.archive; https://docs.slack.dev/reference/methods/conversations.unarchive
- **Request:** `channel` (required). OBSERVED: https://docs.slack.dev/reference/methods/conversations.archive; https://docs.slack.dev/reference/methods/conversations.unarchive
- **Response:** `{"ok":true}`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.archive
- **Errors:**
  - archive: `already_archived`, `cant_archive_general`, `cant_archive_required`, `not_in_channel`, `restricted_action`.
  - unarchive: `not_archived`. Unarchiving adds the caller to the channel.
  - OBSERVED: https://docs.slack.dev/reference/methods/conversations.archive; https://docs.slack.dev/reference/methods/conversations.unarchive
- **Used for (slackless):** channel settings, Archive/Unarchive.

### conversations.rename

- **Transport:** `POST /api/conversations.rename`, form. OBSERVED: beeper/slackgo/conversation.go:334-344
- **Visibility:** public, Tier 2. OBSERVED: https://docs.slack.dev/reference/methods/conversations.rename
- **Request:** `channel` and `name` (both required). OBSERVED: https://docs.slack.dev/reference/methods/conversations.rename. Web-client extras: `skip_rename_message`, `validate`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:345
- **Response:** `{"ok":true,"channel":{…,"name":"<server-normalized>"}}`. The server may rewrite the name, so store the `name` it returns. OBSERVED: https://docs.slack.dev/reference/methods/conversations.rename
- **Errors:** `not_authorized` (only the creator, a Workspace Admin or a Channel Manager may rename), `name_taken`, `invalid_name_*`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.rename
- **Used for (slackless):** rename dialog. mautrix strips a leading `#` before sending. OBSERVED: mautrix/slack/pkg/connector/handlematrix.go:448

### conversations.setTopic / conversations.setPurpose

- **Transport:** `POST /api/conversations.setTopic` and `/api/conversations.setPurpose`, form. OBSERVED: dylex/slack-libpurple/slack-channel.c:407; beeper/slackgo/conversation.go:275-284,304-314
- **Visibility:** public, Tier 2. OBSERVED: https://docs.slack.dev/reference/methods/conversations.setTopic; https://docs.slack.dev/reference/methods/conversations.setPurpose
- **Request:** `channel` plus `topic` or `purpose` (all required). OBSERVED: https://docs.slack.dev/reference/methods/conversations.setTopic. setPurpose's web-client extra: `name_tagging`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:350
- **Response:**
  - setTopic: `{"ok":true,"channel":{…,"topic":{"value":"…","creator":"U…","last_set":0}}}`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.setTopic
  - setPurpose: the docs example is `{"ok":true}`, but slack-go decodes a `channel` from it, so accept both. OBSERVED: https://docs.slack.dev/reference/methods/conversations.setPurpose; beeper/slackgo/conversation.go:300-314
- **Errors:** `not_in_channel`, `is_archived`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.setTopic
- **Used for (slackless):** editing the channel header and details. Each change also arrives as a topic/purpose/name message subtype. OBSERVED: mautrix/slack/pkg/connector/handleslack.go:228-230

### conversations.convertToPrivate / conversations.convertToPublic

- **Transport:** `POST /api/conversations.convertToPrivate` and `/api/conversations.convertToPublic`, form.
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/conversations.convertToPrivate; 3kh0/slack-datamine/build/api-methods.txt:468-469
- **Request:** convertToPrivate takes `channel`, `name`, `user_ids` and `workspace`; convertToPublic takes `channel`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:312-313
- **Response:** unobserved.
- **Admin variant:** `admin.conversations.convertToPrivate` is public but needs an admin token with an org-wide install. It takes `channel_id` (required) and `name` (optional), is Tier 2, and returns errors such as `default_org_wide_channel` and `name_taken`. OBSERVED: https://docs.slack.dev/reference/methods/admin.conversations.convertToPrivate
- **Used for (slackless):** "Change to a private channel". The `channel_updated` event carries `updates.is_private`. OBSERVED: beeper/slackgo/websocket_channels.go:74-87

### conversations.bulkJoin / bulkLeave / bulkArchive / bulkRejoin

- **Transport:** `POST /api/conversations.bulk*`, form.
- **Visibility:** private (web-client registry). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:457-462
- **Request:** bulkArchive takes `channel_ids`; bulkLeave takes `channels`. bulkJoin and bulkRejoin args are unobserved. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:303,305
- **Response:** unobserved.
- **Used for (slackless):** multi-select actions in the channel browser.

### conversations.invite

- **Transport:** `POST /api/conversations.invite`, form. OBSERVED: beeper/slackgo/conversation.go:364-374; dylex/slack-libpurple/slack-channel.c:397
- **Visibility:** public, Tier 3. OBSERVED: https://docs.slack.dev/reference/methods/conversations.invite
- **Request:**
  - `channel` (required); `users` - csv of 1-1000 user ids (required); `force` - bool, default false. OBSERVED: https://docs.slack.dev/reference/methods/conversations.invite
  - Usage of `force=true`. OBSERVED: beeper/slackgo/conversation.go:402-417
  - libpurple sends a single `user` instead. OBSERVED: dylex/slack-libpurple/slack-channel.c:397
- **Response:** `{"ok":true,"channel":{…}}`. If any invitee fails, none are invited. OBSERVED: https://docs.slack.dev/reference/methods/conversations.invite
- **Errors:** `already_in_channel`, `cant_invite_self`, `cant_invite`, `invitee_cant_see_channel`, `no_external_invite_permission`, `external_channel_migrating`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.invite
- **Used for (slackless):** the "Add people" dialog. mautrix also uses it after creating a channel and to bridge Matrix invites. OBSERVED: mautrix/slack/pkg/connector/startchat.go:153-157; mautrix/slack/pkg/connector/handlematrix.go:489-490

### conversations.kick

- **Transport:** `POST /api/conversations.kick`, form. OBSERVED: beeper/slackgo/conversation.go:521-528
- **Visibility:** public, Tier 3. OBSERVED: https://docs.slack.dev/reference/methods/conversations.kick
- **Request:** `channel` (required) and `user` (one user id). OBSERVED: https://docs.slack.dev/reference/methods/conversations.kick; emacs-slack/emacs-slack/slack-conversations.el:290-291; beeper/slackgo/conversation.go:517-528
- **Response:** `{"ok":true,"errors":{}}`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.kick
- **Errors:** `cant_kick_from_general`, `cant_kick_self`, `not_in_channel`, `not_supported`, `no_user`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.kick
- **Used for (slackless):** member list, "Remove from channel". OBSERVED: mautrix/slack/pkg/connector/handlematrix.go:502

### conversations.members

- **Transport:** `POST /api/conversations.members`, form. OBSERVED: dylex/slack-libpurple/slack-channel.c:182
- **Visibility:** public, Tier 4. OBSERVED: https://docs.slack.dev/reference/methods/conversations.members
- **Request:** `channel` (required), `cursor`, and `limit` (default 100). OBSERVED: https://docs.slack.dev/reference/methods/conversations.members. The web-client registry lists `channel` and `limit`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:339
- **Response:** `{"ok":true,"members":["U023BECGF","U061F7AUR","W012A3CDE"],"response_metadata":{"next_cursor":"e3VzZXJfaWQ6IFcxMjM0NTY3fQ=="}}` OBSERVED: https://docs.slack.dev/reference/methods/conversations.members
- **Pagination:** cursor. emacs-slack pages with `limit=100`. OBSERVED: emacs-slack/emacs-slack/slack-conversations.el:563-596
- **Errors:** `fetch_members_failed`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.members
- **Used for (slackless):** the member list in channel details, and naming group DMs. Lax uses it to name group DMs. OBSERVED: andersfischernielsen/Lax/docs/SLACK_PROTOCOL.md:45

### edge: channels/membership

- **Transport:** `POST https://edgeapi.slack.com/cache/<id>/channels/membership`, JSON. OBSERVED: ankarhem/slackless/.omo/research/gist.md:120; rusq/slackdump/internal/edge/userlist.go:192-198
- **Visibility:** private (edge). OBSERVED: ankarhem/slackless/.omo/research/gist.md:109
- **Request:** `channel` (id), `users` (array of user ids; 1-66 observed), `as_admin` (always `false` observed). OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:3355; rusq/slackdump/internal/edge/userlist.go:178-183
- **Response:** `{"ok":true,"channel":"C…","members":["U…"],"non_members":["U…"]}`. In 10 of 10 observed calls, `members` + `non_members` equalled the ids sent. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:3355; rusq/slackdump/internal/edge/userlist.go:185-189
- **Pagination:** none.
- **Used for (slackless):** "is this user in the channel" checks, such as mention warnings and the invite dialog, without paging `conversations.members`.

### conversations.joinRequests.list / approve / deny

- **Transport:** `POST /api/conversations.joinRequests.*`, form.
- **Visibility:** private (web-client registry). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:496-498
- **Request:** list takes `channel_id` and `cursor`. approve and deny args are unobserved. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:330
- **Response:** unobserved.
- **Used for (slackless):** requests to join a private channel (admin view).

### conversations.open

- **Transport:** `POST /api/conversations.open`, form. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:409-415; dylex/slack-libpurple/slack-im.c:187
- **Visibility:** public, Tier 3. OBSERVED: https://docs.slack.dev/reference/methods/conversations.open
- **Request:**
  - `users` - csv of 1-8 user ids (one opens a DM, more open a group DM).
  - `channel` - resume an existing DM or MPIM instead of `users`.
  - `return_im` - bool - return the full channel object.
  - `prevent_creation` - bool.
  - OBSERVED: https://docs.slack.dev/reference/methods/conversations.open
  - slack-go adds `quiet=1` when its `Quiet` option is set. OBSERVED: beeper/slackgo/conversation.go:945-947
  - emacs-slack sends either `users` or `channel`. OBSERVED: emacs-slack/emacs-slack/slack-conversations.el:598-610
- **Response:** `{"ok":true,"no_op":true,"already_open":true,"channel":{"id":"D069C7QFK"}}`. The channel is only `{id}` unless you pass `return_im`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.open; beeper/slackgo/conversation.go:951-956
- **Errors:** `not_enough_users`, `invalid_user_combination`, `restricted_action`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.open
- **Used for (slackless):** "New message" to people, and reopening a closed DM. mautrix opens 1:1 and group DMs this way. OBSERVED: mautrix/slack/pkg/connector/startchat.go:106,160

### conversations.close

- **Transport:** `POST /api/conversations.close`, form. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:425-431
- **Visibility:** public, Tier 2. OBSERVED: https://docs.slack.dev/reference/methods/conversations.close
- **Request:** `channel` (required). OBSERVED: https://docs.slack.dev/reference/methods/conversations.close; emacs-slack/emacs-slack/slack-conversations.el:485-497
- **Response:** `{"ok":true}`, with optional `no_op` and `already_closed` flags. OBSERVED: https://docs.slack.dev/reference/methods/conversations.close; beeper/slackgo/conversation.go:552-556
- **Errors:** `not_permitted`, `external_channel_migrating`, `method_not_supported_for_channel_type`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.close
- **Used for (slackless):** "Close conversation" on a DM or group DM. A closed DM drops out of every sidebar section. OBSERVED: some-du6e/clack/docs/SIDEBAR-RULES.md:22

### conversations.mark

- **Transport:** `POST /api/conversations.mark`, form. OBSERVED: dylex/slack-libpurple/slack-conversation.c:177; gammons/slk/internal/slack/client.go:1589-1597
- **Visibility:** public, Tier 3. OBSERVED: https://docs.slack.dev/reference/methods/conversations.mark
- **Request:** `channel` and `ts` (both required). OBSERVED: https://docs.slack.dev/reference/methods/conversations.mark. Web-client extra: `reason`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:336
- **Response:** `{"ok":true}`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.mark
- **Errors:** `invalid_timestamp`, `not_in_channel`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.mark
- **Used for (slackless):** the read cursor. To mark as unread, mark with an earlier `ts`. OBSERVED: andersfischernielsen/Lax/docs/SLACK_PROTOCOL.md:44; gammons/slk/internal/slack/client.go:1584-1588. Thread read state uses `subscriptions.thread.mark`, which belongs to another area. OBSERVED: beeper/slackgo/conversation.go:1096-1110

### users.channelSections.list

- **Transport:** `POST /api/users.channelSections.list`, form. OBSERVED: gammons/slk/internal/slack/client.go:1906-1925
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/users.channelSections.list; 3kh0/slack-datamine/build/api-methods.txt:1300
- **Request:** `cursor` - string - optional - the top-level `cursor` from the previous page. OBSERVED: gammons/slk/internal/slack/client.go:1906-1915. No other args were recovered from the bundle. OBSERVED: 0xdeafcafe/loafer/docs/slack-webapp-methods.md:33
- **Response:** OBSERVED: ankarhem/slackless/.omo/research/findings.md:43-44,80-91; gammons/slk/internal/slack/sections.go:45-58
  ```json
  {"ok":true,"count":<int>,"cursor":"","last_updated":<int>,"entities":…,
   "channel_sections":[{"channel_section_id":"L0B12LBBCTD","name":"Priority","type":"standard","emoji":"",
     "next_channel_section_id":"L08BCNXM15Y","last_updated":<unix s>,"is_redacted":false,
     "channel_ids_page":{"channel_ids":["C…","D…"],"count":<int>,"cursor":"C…"}}]}
  ```
  - `type` is one of `standard`, `channels`, `direct_messages`, `recent_apps`, `stars`, `slack_connect`, `salesforce_records`, `agents`. OBSERVED: ankarhem/slackless/.omo/research/findings.md:83-90
  - The client's type enum also has `user_group` (loafer shows the enum key `UserGroup`; clack's rank list spells it `user_group`), and those sections carry `usergroup_id`. OBSERVED: 0xdeafcafe/loafer/docs/slack-webapp-methods.md:36,47; some-du6e/clack/docs/SIDEBAR-RULES.md:26
  - `channel_ids_page.count` counts every channel ever assigned, archived or left ones included. `channel_ids` holds only active ones; 216 came back in one page. `cursor` is the last channel id. OBSERVED: tammersaleh/slack-cli/docs/sections.md:19
  - The boot payload can also carry sections at `sections.channel_sections`. OBSERVED: 0xdeafcafe/loafer/docs/slack-webapp-methods.md:33-34
- **Pagination:** loop on the top-level `cursor` until it is empty; slk also stops if the cursor repeats. OBSERVED: gammons/slk/internal/slack/client.go:1963-1985. No method pages a single section's `channel_ids_page`; a channels.list variant answers `unknown_method`. OBSERVED: tammersaleh/slack-cli/docs/sections.md:19
- **Grid:** the member-workspace token gets `team_is_restricted`. OBSERVED: gammons/slk/internal/slack/client.go:356-362
- **Used for (slackless):** sidebar section names, emoji, order (the `next_channel_section_id` chain) and membership.

### users.channelSections.create

- **Transport:** `POST /api/users.channelSections.create`, form. OBSERVED: tammersaleh/slack-cli/cmd/section.go:353-356
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/users.channelSections.create
- **Request:**
  - `name` - string - required.
  - `emoji` - bare emoji name - the key is REQUIRED (an empty value is fine); without it you get `invalid_arguments`. OBSERVED: tammersaleh/slack-cli/docs/sections.md:11
  - The client-side thunk also carries `channelIds`, `nextChannelSectionId` and `sharedChannelSectionArgs`; their wire names are unconfirmed (see LEADS). OBSERVED: 0xdeafcafe/loafer/docs/slack-webapp-methods.md:35
- **Response:** `{"ok":true,"channel_section_id":"L…"}`. The client also reads `errors`. OBSERVED: tammersaleh/slack-cli/cmd/section.go:360-362; 0xdeafcafe/loafer/docs/slack-webapp-methods.md:35-36
- **Emoji rules:** bare names are stored verbatim (`heart`, `+1`, custom names, `wave::skin-tone-2` through `-6`). `:heart:`, raw unicode, unknown names, `heart ` and `Heart` all return `emoji_invalid`. OBSERVED: tammersaleh/slack-cli/docs/sections.md:15
- **Used for (slackless):** "Create section". Expect a `channel_section_upserted` event.

### users.channelSections.set

- **Transport:** `POST /api/users.channelSections.set`, form. OBSERVED: tammersaleh/slack-cli/cmd/section.go:412-415
- **Visibility:** private (web-client registry). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:1301
- **Request:** `channel_section_id` (required), `name`, `emoji`. **Every key you omit is cleared**: a name-only call wiped the emoji, and an emoji-only call set the name to `""`. Always resend both after reading them from `.list`. `.update`, `.edit` and `.rename` all answer `unknown_method`. OBSERVED: tammersaleh/slack-cli/docs/sections.md:11
- **Response:** a bare `{"ok":true}` whether or not anything changed, so re-fetch to confirm. OBSERVED: tammersaleh/slack-cli/docs/sections.md:11
- **Errors:** `already_exists` (case-insensitive name clash), `section_not_found`, `emoji_invalid`. OBSERVED: tammersaleh/slack-cli/docs/sections.md:11,15
- **Used for (slackless):** rename a section and change its emoji. Collapse state and reordering through this method are unconfirmed (see LEADS).

### users.channelSections.delete

- **Transport:** `POST /api/users.channelSections.delete`, form.
- **Visibility:** private (web-client registry). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:1297
- **Request:** `channel_section_id` (required). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:874. A live run confirmed it works. OBSERVED: tammersaleh/slack-cli/docs/sections.md:11
- **Response:** success observed; the body was not recorded.
- **Used for (slackless):** "Delete section". Expect a `channel_section_deleted` event.

### users.channelSections.channels.bulkUpdate

- **Transport:** `POST /api/users.channelSections.channels.bulkUpdate`, **form-encoded (not a JSON body)**, with two fields whose values are JSON strings. OBSERVED: tammersaleh/slack-cli/docs/sections.md:19; tammersaleh/slack-cli/cmd/section.go:615-636
- **Visibility:** private (web-client registry). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:1293
- **Request:**
  - `insert` - JSON string `[{"channel_section_id":"L…","channel_ids":["C…"]}]` - adds channels to a section. **Ignored for a channel that is already in any section.** OBSERVED: tammersaleh/slack-cli/docs/sections.md:19
  - `remove` - same shape - takes channels out of a section; they become unsectioned. OBSERVED: tammersaleh/slack-cli/docs/sections.md:19
  - `skip_simple_array_serialization_in_get_form_data` - listed in the registry; semantics unobserved. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:871
- **Response:** a bare `{"ok":true}` even when nothing moved. A JSON body of `{channel_sections:[…]}` also answers ok and does nothing. OBSERVED: tammersaleh/slack-cli/docs/sections.md:19
- **Semantics:** to move a channel that is already in a section, send `insert` (target) and `remove` (source) in the same call. slack-cli omits `remove` when it would be empty; whether `[]` is accepted is unverified. OBSERVED: tammersaleh/slack-cli/docs/sections.md:19
- **Used for (slackless):** drag-to-section and "Move to section". Expect `channel_sections_channels_upserted` / `_removed` events.

### users.channelSections.channels.remove

- **Transport:** `POST /api/users.channelSections.channels.remove`, form.
- **Visibility:** private (web-client registry). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:1294
- **Request:** `channel_id`, `channel_section_id`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:872
- **Response:** unobserved.
- **Used for (slackless):** removing one channel from a section. bulkUpdate's `remove` covers the same need with semantics that have been observed.

### users.channelSections.createLink / linkInfo / validateShare / entities.update, channelSections.recommend

- **Transport:** `POST /api/<method>`, form.
- **Visibility:** private (web-client registry). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:1295-1302; 3kh0/slack-datamine/build/api-methods.txt:371-372
- **Request:** OBSERVED: 3kh0/slack-datamine/build/api-args.txt:244-245,873,875
  - `users.channelSections.createLink`: `channels`, `emoji`, `section_name`.
  - `users.channelSections.linkInfo`: `id`.
  - `channelSections.recommend`: `debug_info`, `limit`, `query`, `source`.
  - `channelSections.channels.recommend`: `limit`, `section_name`.
  - `validateShare` and `entities.update`: args unobserved.
- **Response:** unobserved.
- **Used for (slackless):** shareable section links and section suggestions (a later milestone).

### users.prefs.get

- **Transport:** `POST /api/users.prefs.get`, form. OBSERVED: gammons/slk/internal/slack/client.go:1677-1700
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/users.prefs.get
- **Request:** `prefs` - csv of pref names - optional. Calling with no arguments returns them all. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2188; gammons/slk/internal/slack/client.go:1678
- **Response:**
  - `{"ok":true,"prefs":{"muted_channels":"<csv of ids>","all_notifications_prefs":"<JSON text>",…}}`. OBSERVED: andersfischernielsen/Lax/Sources/LaxKit/MutedChannels.swift:65-77; gammons/slk/internal/slack/client.go:1686-1692
  - Decoded `all_notifications_prefs`:
    ```json
    {
      "global": {
        "global_desktop": "everything",
        "global_mpdm_desktop": "everything",
        "global_mobile": "everything",
        "global_keywords": "",
        "push_idle_wait": 0,
        "threads_everything": true
      },
      "channels": {
        "CGTR9ED29": {
          "desktop": "everything",
          "mobile": "mentions_dms",
          "muted": false,
          "suppress_at_channel": false
        }
      }
    }
    ```
    OBSERVED: rusq/slackdump/internal/edge/assets/client.userBoot.json:1350
  - The boot copy of `prefs` also shows the sidebar and notification prefs `sidebar_behavior`, `channel_sort` (`"default"`), `channel_sections`, `team_channel_sections`, `at_channel_suppressed_channels`, `push_mention_channels`, `loud_channels`, `never_channels`, `channel_sidebar_hide_invite` and `channel_sidebar_hide_browse_dms_link`. OBSERVED: rusq/slackdump/internal/edge/assets/client.userBoot.json:1115-1116,1340-1350,1406-1407,1429-1430
  - Per-channel keys that `users.prefs.setNotifications` also writes: `desktop_push_enabled`, `follow_all_threads`, `badge_all_unreads`. OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:69-75
- **Muted set:** the csv `muted_channels`, plus channels with `muted:true` in the JSON, minus channels with `muted:false`. OBSERVED: andersfischernielsen/Lax/Sources/LaxKit/MutedChannels.swift:15-27
- **Grid:** the member-workspace token can call it. OBSERVED: gammons/slk/internal/slack/client.go:356-362
- **Used for (slackless):** mute state, notification levels, and sidebar sort and filter prefs. `pref_change` keeps them fresh.

### users.prefs.setNotifications

- **Transport:** `POST /api/users.prefs.setNotifications`, form. OBSERVED: andersfischernielsen/Lax/Sources/LaxKit/MutedChannels.swift:56-61; 3kh0/super-platinum/docs/channel-notifications-protocol.md:42-50
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/users.prefs.setNotifications
- **Request:** the registry lists `channel_id, channel_ids, global, name, prefs, sync, value`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:890
  - **Single pref:** `name`, `value` (string), `channel_id`, `global=false`, `sync=false`, `_x_reason=prefs-store/setChannelNotificationOverride`. OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:59-67
  - **Mute / unmute:** `name=muted`, `value=true|false`, `channel_id`, `global=false`. OBSERVED: andersfischernielsen/Lax/Sources/LaxKit/MutedChannels.swift:56-61
  - **Multi pref:**
    - `channel_ids` (a raw id for one channel), `global=false`, `sync=true`, `_x_reason=prefs-store/setMultiChannelNotificationOverride`.
    - `prefs=[{"name":"desktop","value":"everything","sync":true},{"name":"follow_all_threads","value":"false","sync":false},{"name":"badge_all_unreads","value":"false","sync":false}]`.
    - OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:42-53
  - **Reset a channel:**
    - Send `channel_id`, `name=reset`, `value=default`, `global=false`, `sync=false` and `_x_reason=prefs-store/resetChannelNotificationOverride`.
    - This drops the whole channel override object; there is no per-key delete.
    - OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:151-170
  - **Names and values:**
    - `desktop` and `mobile` take `everything` | `mentions_dms` | `nothing`.
    - `muted`, `desktop_push_enabled`, `follow_all_threads`, `suppress_at_channel` and `badge_all_unreads` take `"true"` | `"false"`.
    - The "broadcast mentions" toggle is the inverse of `suppress_at_channel`.
    - OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:26,30-38,69-75
- **Presets:**
  - "All new posts" = `desktop=everything`, `follow_all_threads=false`, `badge_all_unreads=false`.
  - "Just mentions" = `desktop=mentions_dms`, `follow_all_threads=false`, `suppress_at_channel=false`.
  - Selecting either while the channel is muted also sends `muted=false`.
  - OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:30-36
- **Response:** an ack. Confirm by reading back `users.prefs.get`. OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:122-127
- **Used for (slackless):** mute and unmute, the per-channel notification menu, and reset. Expect a `pref_change` event.

### users.prefs.set

- **Transport:** `POST /api/users.prefs.set`, form. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:569-575
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/users.prefs.set
- **Request:** `name` and `value`; the registry also lists `prefs`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:889
  - Legacy mute rewrites the whole list: `name=muted_channels`, `value=<csv of every muted id>`. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:569-575
  - slack-go instead sends the pref as its own key, `muted_channels=<csv>`, plus `reason=update-muted-channels`, with no `name` or `value`. OBSERVED: beeper/slackgo/info.go:352-356,377-380
- **Response:** an `ok` envelope; slack-go decodes it as a prefs carrier. OBSERVED: beeper/slackgo/info.go:354-361
- **Used for (slackless):** non-notification user prefs. The official client also calls it on channel navigation. OBSERVED: ankarhem/slackless/.omo/research/gist.md:628

### conversations.listPrefs

- **Transport:** `POST /api/conversations.listPrefs`. The official client sends it as multipart with `_x_reason=client_redux_store`. OBSERVED: matthewdonsemail-lab/log/docs/platforms/slack/conversations/conversationsListPrefs.md:31-52
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/conversations.listPrefs
- **Request:** `channel` (sent in every observed call). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:335; matthewdonsemail-lab/log/docs/platforms/slack/conversations/conversationsListPrefs.md:49
- **Response:** OBSERVED: matthewdonsemail-lab/log/docs/platforms/slack/conversations/conversationsListPrefs.md:56-82
  ```json
  {
    "ok": true,
    "prefs": {
      "can_huddle": true,
      "can_upload_files": true,
      "enable_at_channel": true,
      "who_can_post": { "type": ["ra"] },
      "can_thread": { "type": ["ra"] },
      "canvas_share_status": "can_share",
      "list_share_status": "can_share",
      "shared_channel_invite_requested": false,
      "channel_canvas_summary": "",
      "space_icon": "",
      "space_is_primary": false,
      "space_file_id": "",
      "space_open_by_default": false
    }
  }
  ```
- **Pagination:** none.
- **Used for (slackless):** composer, huddle and thread permissions and canvas share state when a channel opens. The official client calls it on every channel switch. OBSERVED: ankarhem/slackless/.omo/research/findings.md:169-171. It is **not** the notification read, though one client describes it that way. OBSERVED: karbassi/slack-mcp/src/slack_mcp/tools/undocumented.py:649-654
- **Related, unexplored:** `channels.prefs.get` (`channel_id`, `pref_name`) and `channels.prefs.set` (`channel_id`, `prefs`). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:247-248

### stars.add / stars.remove / stars.list

- **Transport:** `POST /api/stars.*`, form. OBSERVED: beeper/slackgo/stars.go:44-68,78-102,112-133
- **Visibility:** public but retired for end users. Stars can still be added and listed, but they are no longer shown, and `star_added` is no longer emitted. OBSERVED: https://docs.slack.dev/reference/methods/stars.add; https://docs.slack.dev/reference/methods/stars.list
- **Request:**
  - add and remove take `channel` plus one optional `timestamp`, `file` or `file_comment`; the item can be a channel or DM. OBSERVED: https://docs.slack.dev/reference/methods/stars.add
  - list takes `count`, `cursor`, `limit`, `page` and `team_id`. OBSERVED: https://docs.slack.dev/reference/methods/stars.list. The web registry adds `exclude` and `org_wide_aware`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:821
- **Response:** add and remove return `{"ok":true}`; list returns `{"ok":true,"items":[{"type":"message","channel":"C…","message":{…}}]}`. OBSERVED: https://docs.slack.dev/reference/methods/stars.list
- **Pagination:** list uses `cursor` or the legacy `page`. OBSERVED: https://docs.slack.dev/reference/methods/stars.list
- **Used for (slackless):** nothing new. Read starred conversations from `client.userBoot.starred` (ids) and the `stars` section. On Grid, `stars.list` can be restricted or empty even when boot has stars. OBSERVED: gammons/slk/internal/slack/client.go:1937-1955; rusq/slackdump/internal/edge/client_boot.go:99

### conversations.inviteShared

- **Transport:** `POST /api/conversations.inviteShared`, form. OBSERVED: beeper/slackgo/conversation.go:479-501
- **Visibility:** public, Tier 2. OBSERVED: https://docs.slack.dev/reference/methods/conversations.inviteShared
- **Request:** `channel` (required), plus `emails` or `user_ids` (one of the two is required, and only one email or one user id may be invited per call; slack-go sends whichever is set, comma-joined), `external_limited` (bool, default true). OBSERVED: https://docs.slack.dev/reference/methods/conversations.inviteShared; beeper/slackgo/conversation.go:483-493. The web registry lists `channel`, `external_limited` and `tracking_id`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:326
- **Response:** `{"ok":true,"invite_id":"I02UKAJ6RJA","is_legacy_shared_channel":false}` OBSERVED: https://docs.slack.dev/reference/methods/conversations.inviteShared
- **Errors:** `cannot_share_mandatory_channel`, `connection_limit_exceeded`, `invalid_email`, `no_external_invite_permission`, `member_limit_exceeded`, `inviter_email_not_verified`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.inviteShared
- **Used for (slackless):** "Share channel with an external org"; the channel becomes a Slack Connect channel. OBSERVED: https://docs.slack.dev/reference/methods/conversations.inviteShared. Link invites use `sharedInvites.getLink` (`emails`, `message`, `tracking_id`). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:784

### conversations.acceptSharedInvite

- **Transport:** `POST /api/conversations.acceptSharedInvite`, form.
- **Visibility:** public, Tier 2. OBSERVED: https://docs.slack.dev/reference/methods/conversations.acceptSharedInvite
- **Request:** `channel_name` (required), `invite_id` or `channel_id` (one of the two is required), `is_private`, `free_trial_accepted`, `team_id` (required with an org-level token). OBSERVED: https://docs.slack.dev/reference/methods/conversations.acceptSharedInvite. The web client sends `channel_name`, `free_trial_accepted`, `invite_id`, `is_private` and `sig`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:298
- **Response:** `{"ok":true,"implicit_approval":true,"channel_id":"C0001111","invite_id":"I00043221"}`. Admins may still need to approve. OBSERVED: https://docs.slack.dev/reference/methods/conversations.acceptSharedInvite
- **Errors:** `accepter_email_not_verified`, `email_does_not_match`, `has_already_connected_to_org`, `invite_from_same_org`, `invalid_link`, `connection_limit_exceeded`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.acceptSharedInvite
- **Used for (slackless):** accepting an invite from the banner or a link. Related private calls: OBSERVED: 3kh0/slack-datamine/build/api-args.txt:328-329,354,782-783
  - `sharedInvites.canAccept` (`sig`)
  - `sharedInvites.accept` (`channel`, `invite`, `sig`)
  - `conversations.sharedInviteInfo` (`context`, `sig`)
  - `conversations.joinPendingShared` / `joinConnectedShared` (`invite_id`, `sig`)

### conversations.teamConnections

- **Transport:** `POST /api/conversations.teamConnections`, form-encoded. OBSERVED: karbassi/slack-mcp/docs/undocumented-endpoints.md:30
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/conversations.teamConnections
- **Request:** `channel` (required). OBSERVED: karbassi/slack-mcp/docs/undocumented-endpoints.md:30; 3kh0/slack-datamine/build/api-args.txt:356
- **Response:** `{"ok":true,"connections":[…],"pending_connections":[…],"previous_connections":[…]}` OBSERVED: karbassi/slack-mcp/docs/undocumented-endpoints.md:30
- **Used for (slackless):** the "organizations in this channel" panel of a shared channel.

### conversations.listConnectInvites / connectInvites.list

- **Transport:** `POST /api/<method>`, form.
- **Visibility:** `conversations.listConnectInvites` is public, Tier 1. OBSERVED: https://docs.slack.dev/reference/methods/conversations.listConnectInvites. `connectInvites.list` is private and was captured from the web client. OBSERVED: karbassi/slack-mcp/docs/undocumented-endpoints.md:29
- **Request:** listConnectInvites takes `team_id`, `count` (default 100) and `cursor`. connectInvites.list takes `invite_types` (an enum list; unknown values are rejected) and `only_pending_invites`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.listConnectInvites; karbassi/slack-mcp/docs/undocumented-endpoints.md:29
- **Response:** connectInvites.list returns `{"ok":true,"connect_invites":[…]}`. OBSERVED: karbassi/slack-mcp/docs/undocumented-endpoints.md:29. listConnectInvites returns no Slack Connect DM invites. OBSERVED: https://docs.slack.dev/reference/methods/conversations.listConnectInvites
- **Pagination:** cursor (listConnectInvites). OBSERVED: https://docs.slack.dev/reference/methods/conversations.listConnectInvites
- **Used for (slackless):** the pending external invitations list.

### Other Slack Connect methods (private)

- **Transport:** `POST /api/<method>`, form.
- **Visibility:** private (web-client registry). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:463,467,475,518-520,1140-1147
- **Request:** OBSERVED: 3kh0/slack-datamine/build/api-args.txt:308,347-348,785
  - `conversations.canCreateSharedInvite` (`channel_id`)
  - `conversations.requestSharedInvite.deny` (`invite_id`, `message`)
  - `conversations.revokeSharedInvite` (`invite_id`)
  - `sharedInvites.revoke` (`id`)
  - Args unobserved: `conversations.disconnectShared`, `conversations.convertExternalLimited`, `conversations.requestSharedInvite.approve`, `sharedInvites.ignoreInvite` / `ignoreUser` / `canGetLink`
- **Response:** unobserved.
- **Used for (slackless):** disconnecting an org and revoking or approving invites.

### conversations.canvases.create

- **Transport:** `POST /api/conversations.canvases.create`, form. OBSERVED: beeper/slackgo/conversation.go:1148-1179
- **Visibility:** public, Tier 2. OBSERVED: https://docs.slack.dev/reference/methods/conversations.canvases.create
- **Request:** `channel_id` (required), `title`, `document_content` (a JSON object). OBSERVED: https://docs.slack.dev/reference/methods/conversations.canvases.create; beeper/slackgo/conversation.go:1160-1172
- **Response:** `{"ok":true,"canvas_id":"F1234ABCD"}` OBSERVED: https://docs.slack.dev/reference/methods/conversations.canvases.create
- **Errors:** `channel_canvas_already_exists`, `free_team_canvas_tab_already_exists`, `canvas_disabled_user_team`, `canvas_tab_creation_failed`. OBSERVED: https://docs.slack.dev/reference/methods/conversations.canvases.create
- **Channel object:** `properties.canvas` = `{"file_id","is_empty","quip_thread_id"}`. OBSERVED: beeper/slackgo/conversation.go:93-97. Canvas content editing belongs to the canvas area.
- **Used for (slackless):** "Add canvas" in the channel header.

### conversations.addTab / updateTab / removeTab / moveTab / setProperties

- **Transport:** `POST /api/<method>`, form.
- **Visibility:** private (addTab docs 404). OBSERVED: https://docs.slack.dev/reference/methods/conversations.addTab; 3kh0/slack-datamine/build/api-methods.txt:453,464,508,515,522,535
- **Request:** OBSERVED: 3kh0/slack-datamine/build/api-args.txt:299,344,349,359
  - `addTab`: `channel_id`, `type`, `label`, `data`, `after`, `target_team_id`
  - `updateTab`: `channel_id`, `id`, `label`, `data`, `is_pinned`
  - `removeTab`: `channel_id`, `id`, `keep_file_share`
  - `setProperties`: `channel_id`, `props`
  - `moveTab` and `canSetProperties`: args unobserved
- **Response:** unobserved.
- **State:**
  - Tabs live on the channel record as `properties.tabs` = `[{"id","label","type"}]`. OBSERVED: beeper/slackgo/conversation.go:73-91
  - Also on the record: `tabz`, `default_tab_id`, `auto_open_tab_id`, `who_can_manage_tabs`. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2258
  - User prefs: `channel_canvas_variant`, `channel_canvas_layouts`. OBSERVED: rusq/slackdump/internal/edge/assets/client.userBoot.json:1257-1258
- **Used for (slackless):** channel header tabs.

### conversations.setTeams (Enterprise Grid org-wide channels)

- **Transport:** `POST /api/conversations.setTeams`, form.
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/conversations.setTeams; 3kh0/slack-datamine/build/api-methods.txt:525
- **Request:**
  - `channel`, `internal_team_ids`, `upgrade_to_org_wide`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:351
  - Admin-token siblings `admin.conversations.setTeams` and `getTeams`. OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:55,63
  - `conversations.findEligibleWorkspaces` also exists; its args are unobserved. OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:482
- **Response:** unobserved.
- **Org-wide fields on channel records:** `is_org_shared`, `is_org_default`, `is_org_mandatory`, `internal_team_ids`, `context_team_id`, `enterprise_id`, `conversation_host_id`, `shared_team_ids`. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2258; rusq/slackdump/internal/edge/client_boot.go:116-149
- **Routing:** Grid web calls go to `<workspace>.enterprise.slack.com` with `slack_route=<E…>:<E…>`, and edge paths use the enterprise id. OBSERVED: ankarhem/slackless/.omo/research/gist.md:77,86,341
- **Used for (slackless):** org-wide and mandatory badges, and "add channel to workspaces".

### Cross-area methods (one line each)

- `client.userBoot` (boot area). Sidebar inputs: `channels[]` (partial), `ims[]`, `is_open[]` (open DM ids), `starred[]`, `channels_priority{id: score}`, `read_only_channels[]`, `prefs{}`, `has_more_mpdms`. The client sends `version_all_channels=false` and `return_all_relevant_mpdms=true`. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:5; rusq/slackdump/internal/edge/client_boot.go:80-113
- `client.counts` (unreads area): `last_read`, `latest`, `mention_count` and `has_unreads` per channel, MPIM and IM, plus `channel_badges`. The client sends `thread_counts_by_channel`, `org_wide_aware` and `include_file_channels`. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:1501; mautrix/slack/pkg/connector/client.go:572-576
- `conversations.view` (route hydration), and `conversations.history` / `bookmarks.list` fired on channel switch (messaging and bookmarks areas). OBSERVED: ankarhem/slackless/.omo/research/findings.md:27,169-171

## Websocket events

Frames are `{"type":"<name>",…}` on the client websocket, which was opened with `lazy_channels=1` and `no_query_on_subscribe=1`. OBSERVED: ankarhem/slackless/.omo/research/findings.md:108-109. Every name below appears in the web client's handler registry. OBSERVED: 3kh0/slack-datamine/build/rtm-events.txt:82-324

### Sidebar section events

- **`channel_section_upserted`** - fires when a section is created or its metadata changes. OBSERVED: gammons/slk/internal/slack/testdata/ws_section_upserted.json:1; 3kh0/slack-datamine/build/rtm-events.txt:103
  ```json
  {
    "type": "channel_section_upserted",
    "channel_section_id": "L0B12LBBCTD",
    "name": "test2",
    "emoji": "",
    "channel_section_type": "standard",
    "next_channel_section_id": "L08BCNXM15Y",
    "last_update": 1777720183,
    "is_redacted": false,
    "event_ts": "1777720183.009300"
  }
  ```
  The websocket uses `channel_section_type`/`last_update` where REST uses `type`/`last_updated`. OBSERVED: gammons/slk/internal/slack/sections.go:80-92
- **`channel_section_deleted`** - `{"type":"channel_section_deleted","channel_section_id":"L…","last_update":<int>}`. OBSERVED: gammons/slk/internal/slack/sections.go:110-114; 3kh0/slack-datamine/build/rtm-events.txt:101
- **`channel_sections_channels_upserted`** - channels were added to a section. OBSERVED: gammons/slk/internal/slack/testdata/ws_channels_upserted.json:1
  ```json
  {
    "type": "channel_sections_channels_upserted",
    "channel_section_id": "L0B1709V0LE",
    "channel_ids": ["D09R4P6G6QL"],
    "last_update": 1777720402,
    "event_ts": "1777720402.010300"
  }
  ```
  The web client's handler also reads `sort_source`. OBSERVED: 0xdeafcafe/loafer/docs/slack-webapp-methods.md:43
- **`channel_sections_channels_removed`** - same shape. OBSERVED: gammons/slk/internal/slack/sections.go:116-122; 0xdeafcafe/loafer/docs/slack-webapp-methods.md:44
- **`channel_section_reset`**, **`channel_sections_entities_upserted`** / **`_removed`** (`entities[]`, `channel_section_id`) - only the field names were recovered. OBSERVED: 0xdeafcafe/loafer/docs/slack-webapp-methods.md:45; 3kh0/slack-datamine/build/rtm-events.txt:102,106-107

### Conversation lifecycle events

- **`channel_created`** - `{"type":"channel_created","channel":{"id","is_channel","name","created","creator"},"event_ts"}`. OBSERVED: beeper/slackgo/websocket_channels.go:3-17
- **`channel_rename`** / **`group_rename`** - `{"type":"channel_rename","channel":{"id","name","created"},"event_ts"}` (`group_rename` uses `ts`). OBSERVED: beeper/slackgo/websocket_channels.go:37-49; beeper/slackgo/websocket_groups.go:31-36
- **`channel_archive`**, **`channel_unarchive`**, **`channel_deleted`** and the `group_*` equivalents - `{"type","channel":"C…","user":"U…","ts"}`; here `channel` is an id string. OBSERVED: beeper/slackgo/websocket_channels.go:25-35,63-72; beeper/slackgo/websocket_groups.go:19-23; 3kh0/slack-datamine/build/rtm-events.txt:183-191
- **`channel_updated`** - `{"type":"channel_updated","channel":"C…","updates":{"is_mpim":<bool>,"is_private":<bool>},"ts","event_ts"}`. mautrix responds by re-reading channel info. OBSERVED: beeper/slackgo/websocket_channels.go:74-87; beeper/slackgo/websocket_managed_conn.go:680; mautrix/slack/pkg/connector/handleslack.go:377-380
- **`channel_changed`**, **`channel_converted_to_shared`**, **`group_converted_to_shared`**, **`teams_joined_shared_channel`**, **`teams_left_shared_channel`** - names only. OBSERVED: 3kh0/slack-datamine/build/rtm-events.txt:89,91,184,323-324

### Own membership and DM visibility events

- **`channel_joined`** / **`group_joined`** - `{"type":"channel_joined","channel":{<full conversation object>}}`; it can carry `temp_membership_expiration_ts`. OBSERVED: beeper/slackgo/websocket_channels.go:19-23; some-du6e/clack/docs/SIDEBAR-RULES.md:57
- **`channel_left`** / **`group_left`** - `{"type":"channel_left","channel":"C…","user","ts"}`. OBSERVED: beeper/slackgo/websocket_channels.go:25-35,63
- **`im_created`** - `{"type":"im_created","user":"U…","channel":{"id",…}}`. **`mpim_joined`** also appears in the registry. OBSERVED: beeper/slackgo/websocket_dm.go:3-8; 3kh0/slack-datamine/build/rtm-events.txt:228
- **`im_open`** / **`im_close`**, **`mpim_open`** / **`mpim_close`** - all four are in the registry. OBSERVED: 3kh0/slack-datamine/build/rtm-events.txt:199-204,226-230. slack-go models `im_open` / `im_close` as `{"type","channel":"D…","user","ts"}` and has no `mpim_open` / `mpim_close` type. OBSERVED: beeper/slackgo/websocket_dm.go:13-17; beeper/slackgo/websocket_channels.go:26-35; beeper/slackgo/websocket_managed_conn.go:599-603. `group_open` / `group_close` are in LEADS. slk instead decodes `channel` as a full object for `mpim_open`, `im_created`, `im_open`, `group_joined` and `channel_joined`, treating them as "conversation became visible". OBSERVED: gammons/slk/internal/slack/events.go:434-440,768

### Membership events for other users

- **`member_joined_channel`** / **`member_left_channel`** - `{"type":"member_joined_channel","channel":"C…","user":"U…","team":"T…"}`. Fires when someone joins or leaves a channel you can see. OBSERVED: gammons/slk/internal/slack/events.go:352-359; beeper/slackgo/websocket_managed_conn.go:668-669; mautrix/slack/pkg/connector/handleslack.go:367-375

### Read-state events

- **`channel_marked`** / **`group_marked`** / **`im_marked`** / **`mpim_marked`** - `{"type":"channel_marked","channel":"C…","ts":"…","unread_count_display":<int>,"mention_count":<int>}`. A missing `mention_count` decodes to 0, which clears the badge. Fires when your read cursor moves. OBSERVED: gammons/slk/internal/slack/events.go:77-78,420-431; mautrix/slack/pkg/connector/handleslack.go:330-349

### Preference events

- **`pref_change`** - `{"type":"pref_change","name":"muted_channels","value":"<csv ids>"}`. `value` is a string for scalar prefs (`muted_channels` csv, `all_notifications_prefs` JSON text) and an array or object for others. OBSERVED: gammons/slk/internal/slack/events.go:442-452; andersfischernielsen/Lax/Sources/LaxKit/MutedChannels.swift:29-36

### Star events (legacy)

- **`star_added`** / **`star_removed`** - `{"type","user","item":{…},"event_ts"}`. The docs say `star_added` is no longer emitted. OBSERVED: beeper/slackgo/websocket_stars.go:3-14; https://docs.slack.dev/reference/methods/stars.add

## Gotchas

- **Rate limits:**
  - Tiers: T1 `listConnectInvites`; T2 `list`, `create`, `archive`, `unarchive`, `rename`, `setTopic`, `setPurpose`, `close`, `stars.add`, `inviteShared`, `acceptSharedInvite`, `canvases.create`; T3 `users.conversations`, `info`, `join`, `leave`, `invite`, `kick`, `open`, `mark`, `stars.list`; T4 `members`. OBSERVED: the docs pages cited above.
  - On HTTP 429, honour `Retry-After`. slackdump retries once and defaults to 5 s when the header is missing. OBSERVED: rusq/slackdump/internal/edge/edge.go:269,303-330
  - slackdump paces private paginated listings at one call per 300 ms (burst 5). OBSERVED: rusq/slackdump/internal/edge/limits.go:35-41
  - A per-channel `conversations.info` sweep over a whole sidebar can be about 2000 calls. OBSERVED: tammersaleh/slack-cli/docs/sections.md:7
- **Section writes:**
  - `users.channelSections.set` and `.channels.bulkUpdate` answer `{"ok":true}` even when nothing changed. Re-fetch `users.channelSections.list`, or wait for the websocket echo, before you show success. OBSERVED: tammersaleh/slack-cli/docs/sections.md:11,19
  - `set` clears every key you omit, so always read-modify-write both `name` and `emoji`. OBSERVED: tammersaleh/slack-cli/docs/sections.md:11
  - `insert` is ignored for a channel that is already in a section, so a move is insert plus remove in one call. OBSERVED: tammersaleh/slack-cli/docs/sections.md:19
- **Section placement and order** (ported from the web client):
  - Starred conversations go to Starred.
  - A conversation listed in one section goes there. One listed in several goes to the highest rank: stars, then standard, then user_group. Anything else goes to the default Channels, Direct messages or Apps bucket.
  - Section order follows the `next_channel_section_id` chains, longest chains first.
  - OBSERVED: some-du6e/clack/docs/SIDEBAR-RULES.md:23-32
  - Externally shared conversations that no section lists go to the `slack_connect` section, which is always titled "External connections". OBSERVED: some-du6e/clack/docs/SIDEBAR-RULES.md:105-108
- **Sort and filter prefs:**
  - A per-section `"sort"` in the `channel_sections` pref wins. Otherwise `channel_sort` applies: `default` (alphabetical with numeric collation), `recent`, `priority` or `recent_dms`.
  - `sidebar_behavior` (`""`, `hide_inactive_channels`, `hide_read_channels`, `show_mentions_only`, `hide_read_channels_unless_starred`) filters rows.
  - Muted rows sort last.
  - OBSERVED: some-du6e/clack/docs/SIDEBAR-RULES.md:33-35,42-49
  - Boot carries a 0-1 `channels_priority` score per channel. OBSERVED: ankarhem/slackless/.omo/research/gist.md:203
- **DM visibility:** only `client.userBoot`, `sidebar.dms` and `client.dms` report `is_open`; `users.conversations` does not. Closed DMs leave every section, custom ones included. OBSERVED: some-du6e/clack/docs/SIDEBAR-RULES.md:10-11,22
- **Enterprise Grid token split:**
  - The org-level token can call `client.userBoot` and `client.counts` but not `users.conversations`.
  - The member-workspace token can call `users.conversations` and `users.prefs.get`. It gets `internal_error` from `client.userBoot`, and `team_is_restricted` from `client.counts`, `conversations.view` and `users.channelSections.list`.
  - Tell the tokens apart by `auth.test` answering `team_id == enterprise_id` for the org-level one; this was observed on a live grid, not documented.
  - OBSERVED: gammons/slk/internal/slack/client.go:348-366
  - Public list methods need `team_id` with org-level tokens. OBSERVED: https://docs.slack.dev/reference/methods/conversations.list
  - Edge calls need `enterprise_token` on grids. OBSERVED: karbassi/slack-mcp/docs/undocumented-endpoints.md:81
- **Mute has two stores:**
  - The legacy csv `muted_channels` and `all_notifications_prefs.channels[id].muted`. Read both and apply the JSON overrides last. OBSERVED: andersfischernielsen/Lax/Sources/LaxKit/MutedChannels.swift:3-27
  - The csv path is a whole-list read-modify-write. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:569-575
  - slack-go's helpers are broken: `MuteChat` prepends `,` to an empty list, and `UnMuteChat` returns early when it finds the channel, so it never unmutes. OBSERVED: beeper/slackgo/info.go:341-386
- **No `conversations.setNotificationPrefs`:** the web client's registry has no such method. Per-channel notification writes go through `users.prefs.setNotifications`. OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:452-537,1330-1332
- **Notification quirks:** a raw mobile `nothing` normalizes to `mentions_dms` in the calculated mode. A channel reset removes the whole override object, so it cannot restore a baseline that had mute or unknown keys. OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:111-117,166-172
- **`conversations.listPrefs` returns channel permissions**, not notification settings. OBSERVED: matthewdonsemail-lab/log/docs/platforms/slack/conversations/conversationsListPrefs.md:56-82
- **Stars:** retired for end users. Starred is the `stars` section plus `client.userBoot.starred`. OBSERVED: https://docs.slack.dev/reference/methods/stars.add; ankarhem/slackless/.omo/research/findings.md:87; gammons/slk/internal/slack/client.go:1937-1955
- **Edge `channels/info`:** treat ids in `failed_ids` as failed, not unchanged, or you keep stale records forever. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2258
- **Pagination traps:**
  - `search.modules.channels` page numbers past 100 wrap to page 1. OBSERVED: rusq/slackdump/internal/edge/edge.go:456-463
  - `conversations.list` filters after paging. OBSERVED: https://docs.slack.dev/reference/methods/conversations.list
  - A section's `channel_ids_page.count` includes archived channels. OBSERVED: tammersaleh/slack-cli/docs/sections.md:19
- **Response-shape traps:**
  - `mpim.list` returns `groups`. OBSERVED: rusq/slackdump/internal/edge/mpims.go:34
  - `conversations.invite` is all-or-nothing. OBSERVED: https://docs.slack.dev/reference/methods/conversations.invite
  - The `setPurpose` response differs between the docs and slack-go. OBSERVED: beeper/slackgo/conversation.go:300-314
  - `im_open` carries `channel` as an id in slack-go but as an object in slk, which decodes `mpim_open` the same way, so decode both. OBSERVED: beeper/slackgo/websocket_dm.go:13-14; gammons/slk/internal/slack/events.go:434-440,768
- **The boot channel list is not a directory:** `client.userBoot.channels` held 200 channels while `client.counts` listed 264. Hydrate missing ids through edge `channels/info` or `conversations.genericInfo`. OBSERVED: ankarhem/slackless/.omo/research/findings.md:75,97

## LEADS

Beliefs not yet observed; capture each from the live web client before relying on it.

1. `users.channelSections.set` probably accepts `is_collapsed` (loafer guesses this, in 0xdeafcafe/loafer/internal/slack/manage.go:69-77). Test whether sending it alone clears `name` and `emoji`.
2. Reordering sections is probably `users.channelSections.set` (or `.create`) with `next_channel_section_id`.
3. `users.channelSections.create` probably takes `channel_ids` (a JSON list) and `next_channel_section_id`, mirroring the thunk's `channelIds` and `nextChannelSectionId`.
4. Starring a conversation in the current client is probably `users.channelSections.channels.bulkUpdate` with an `insert` into the section whose `type` is `stars`, and unstarring a `remove` from it (loafer guesses this). Also check whether `stars.add channel=` still updates `client.userBoot.starred`.
5. Deleting a section probably returns its channels to the default buckets and emits `channel_section_deleted`, possibly with `channel_sections_channels_removed` as well.
6. `channel_section_reset` probably means "refetch `users.channelSections.list`".
7. `entities` (in `.list` and in the `channel_sections_entities_*` events) probably holds non-conversation sidebar items such as lists, canvases or files pinned to the sidebar.
8. The meaning of `is_redacted` on a section is unknown.
9. `channel_sort`, `sidebar_behavior` and the `channel_sections` pref JSON (per-section `sort` and `sidebar` filter) are probably written with `users.prefs.set`. The `channel_sections` format is unobserved, because it was an empty string in the captures.
10. The `priority` sort probably uses `client.userBoot.channels_priority`.
11. `lazy_channels=1` probably makes the boot channel list deliberately partial, with rows hydrated on demand through edge `channels/info`.
12. The conversion methods probably emit `channel_updated` with `updates.is_private`. Capture what `convertToPrivate` and `convertToPublic` actually send.
13. `conversations.validateName` probably answers `ok`, `name_taken` or `invalid_name_*`, as a live check.
14. `sidebar.dms` probably takes `count` and `cursor`, like `client.dms`.
15. The args of `conversations.disconnectShared` (channel plus leaving team?), `conversations.moveTab` (`channel_id`, `id`, `after`?) and `bulkJoin` are unknown.
16. `channels.prefs.get/set` probably manage the channel-level permissions that `conversations.listPrefs` reads (such as `who_can_post`).
17. Joining a private channel probably fails with `method_not_supported_for_channel_type`, and the official client switches to a join request (`conversations.joinRequests.*`) where that is enabled.
18. `search.modules.channels` `channel_type` may also accept `org_wide`, matching the thunk value `search_channel_types:["exclude_archived","org_wide"]`.
19. A closed DM probably reopens (`im_open`) when a new message arrives (per loafer's comment in 0xdeafcafe/loafer/internal/slack/manage.go:49).
20. On Grid, calls made with xoxc session tokens may need `team_id` or the right workspace subdomain for `conversations.create` and `conversations.list`. Verify which `slack_route` the official client uses for org-wide channels.
21. `star_removed` may still be emitted, since the docs only retire `star_added`.
22. `group_open` / `group_close` arrive on the client websocket as `{"type","channel","user","ts"}`; slack-go maps both names (beeper/slackgo/websocket_managed_conn.go:606,609; beeper/slackgo/websocket_groups.go:13-17). unverified: neither name is in the web client's handler registry (3kh0/slack-datamine/build/rtm-events.txt), so the official client may never receive them.
