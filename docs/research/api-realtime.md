# Boot and realtime websocket - Slack web-client API

## Summary

This area covers state hydration at startup (`client.userBoot`, `client.boot`, `client.counts`, `client.shouldReload`, `client.dms`) and the message-server websocket at `wss://wss-primary.slack.com/` (connect URL, hello/goodbye/reconnect_url, ping/tickle/presence_sub/user_typing frames, reconnect + resync). Everything here except `rtm.connect`/`rtm.start` is private/undocumented; the two RTM methods are public but deprecated for new apps. The proven working recipe (mautrix-slack, slackdump): fetch client version data from `https://app.slack.com/client` HTML, call `client.userBoot` with the `xoxc` token + `d=` cookie, dial the flannel websocket with the full parameter set, send `presence_sub` + `tickle` after `hello`, and resync via `client.counts` + `conversations.history` after reconnect. Biggest risks: the boot endpoint is being renamed (`client.init` already observed server-side), the socket dies without warning via `goodbye`, and event payloads drift silently (fields appear/disappear by workspace, plan and Grid org).

## Endpoints

### client.userBoot

The main boot method. Returns the full hydrate-on-startup payload for the current user+team. The web client calls it first (after version checks); mautrix-slack calls it before dialing the websocket and keeps the response as connection context.

- Transport: `POST https://<workspace>.slack.com/api/client.userBoot`, form-encoded. Sends `Cookie: d=<xoxd>` and `token=<xoxc>` in the form body. Non-standard fields `_x_reason`, `_x_mode`, `_x_sonic`, `_x_app_name` are added alongside.
  OBSERVED: beeper/slackgo/users.go:684 (`api.postMethod(ctx, "client.userBoot", values, response)`)
  OBSERVED: rusq/slackdump v4 internal/edge/client_boot.go:56 (slackdump's copy posts to `"client.init"` — same form, same response shape, validated with `validate("client.userBoot")`; see LEADS)
  OBSERVED: .omo/research/gist.md:162 (`client.userBoot` is boot call #7, the "big one")
- Visibility: private (no docs page).
- Request params:
  - `token` - string (xoxc-...) - required - user session token
  - `version_all_channels` - "false" - optional - slackgo hardcodes `false`
    OBSERVED: beeper/slackgo/users.go:660
  - `omit_channels` - "false" - optional - set `true` for a warm boot that returns only `unchanged_channel_ids` instead of full channel objects
    OBSERVED: beeper/slackgo/users.go:661
  - `include_min_version_bump_check` - "0"|"1" - optional - `1` when doing incremental refresh
    OBSERVED: beeper/slackgo/users.go:662
  - `version_ts`, `build_version_ts`, `build_manifest_last_modified` - int (unix secs) - optional - values scraped from the `app.slack.com/client` HTML `<html data-version-ts=...>` attributes; present when version data was fetched
    OBSERVED: beeper/slackgo/users.go:670-672; beeper/slackgo/versiondata.go:11-19 (VersionData fields `data-version-ts`, `data-version-hash`, `data-build-version-ts`, `data-build-manifest-last-modified`, `data-frontend-build-type`, `data-gantry` parsed from HTML attributes at versiondata.go:98-115)
  - `min_channel_updated` - int (unix millis) - optional - with `include_min_version_bump_check=1` and `_x_reason=deferred-data`, returns only channels updated after this ts
    OBSERVED: beeper/slackgo/users.go:676-680 (sets `_x_reason: "deferred-data"`)
  - `include_relevant_onboarding` - bool - optional (JSON form) - slackdump sends `true`
    OBSERVED: rusq/slackdump v4 internal/edge/client_boot.go:33,53
  - `_x_reason` - "initial-data" | "deferred-data" - optional - Sonic boot phase marker
    OBSERVED: beeper/slackgo/users.go:665,677; rusq/slackdump v4 internal/edge/client_boot.go:54 (`webclientReason("initial-data")`)
  - `_x_sonic` - true - optional - "client supports Sonic boot"
    OBSERVED: beeper/slackgo/users.go:666 (`Sonic: true`); .omo/research/gist.md:95
- Response (key fields; long-form excerpt reconstructed from struct definitions):
  OBSERVED: beeper/slackgo/users.go:201-233 (ClientUserBootResponse)
  OBSERVED: rusq/slackdump v4 internal/edge/client_boot.go:80-115 (independent Go mapping of the same JSON; wider field list)
  OBSERVED: .omo/research/findings.md:72-80 (live counts: `channels: list[200]`, `ims: list[38]`, `prefs: dict[676]`)
  OBSERVED: rusq/slackdump v4 internal/edge/assets/client.userBoot.json (captured raw response shipped in-repo as a test asset)

```json
{
  "ok": true,
  "self": { "id": "U...", "name": "...", "profile": {...} },
  "team": { "id": "T...", "name": "...", "domain": "...", "prefs": {...} },
  "workspaces": [ { "id": "T...", "name": "..." } ],
  "default_workspace": "T...",
  "is_open": ["C...","D..."],
  "starred": [],
  "channels": [ { "id": "C...", "name": "...", "is_channel": true, "is_group": false,
                  "is_im": false, "is_mpim": false, "is_private": false,
                  "is_archived": false, "is_general": false, "is_shared": false,
                  "is_org_shared": false, "is_member": true, "context_team_id": "T...",
                  "created": 1234567890, "updated": 1699999999999,
                  "topic": {...}, "purpose": {...}, "members": ["U..."],
                  "last_read": "1699999999.000100", "latest": "1700000000.000100" } ],
  "ims": [ { "id": "D...", "user": "U...", "is_im": true, "is_open": true,
             "last_read": "...", "latest": "...", "updated": 1699999999 } ],
  "channels_priority": { "C...": 0.93, "D...": 0.71 },
  "subteams": { "self": ["S..."] },
  "dnd": { ... },
  "prefs": { "display_real_names_override": 0, ... },
  "slack_route": "E...:E...",
  "is_europe": false,
  "should_reload": false,
  "can_access_client_v2": true,
  "client_min_version": 0,
  "recommended_build_version": 0,
  "emoji_cache_ts": "1699000000.000000",
  "translations_cache_ts": "...", "app_commands_cache_ts": "...",
  "cache_ts_version": "...", "cache_version": "...",
  "auth_min_last_fetched": 1699999999,
  "account_types": { "is_admin": [], "is_owner": [] },
  "accept_tos_url": null,
  "read_only_channels": [], "non_threadable_channels": [], "thread_only_channels": [],
  "mobile_app_requires_upgrade": false,
  "links": {...},
  "unchanged_channel_ids": []
}
```

- `channels_priority` maps channel id -> relevance score 0.0-1.0 used for sidebar ordering.
  OBSERVED: .omo/research/gist.md:203-208 ("assigns every channel a relevance score between 0 and 1 ... scores approaching 1.0 are channels the user interacts with most frequently"); beeper/slackgo/users.go:213 (`ChannelsPriority map[string]float64`)
- Warm boot: sending `min_channel_updated` returns `unchanged_channel_ids` for channels whose data the client may keep cached.
  OBSERVED: beeper/slackgo/users.go:227-228 ("Only for warm boots: UnchangedChannelIDs"); rusq/slackdump v4 internal/edge/client_boot.go:105
- Pagination: none.
- Used for: startup hydration of self, team, channel/DM list, priorities, prefs, DND; the object mautrix-slack keeps as `BootResp` for the life of the connection.
  OBSERVED: mautrix/slack pkg/connector/client.go:302-328 (`boot()` -> `ClientUserBootContext(ctx, time.Time{})`, then `connectAfterBoot`)
- OBSERVED: rusq/slackdump v4 internal/edge/client_boot.go:41-62

### client.boot

The older, lighter boot method (pre-Sonic). Returns self, team, channel list with `latest`, IM list, and `is_open` ids. Still accepted by the server (slackgo ships it and mautrix-slack's TODO marks it as the intended RTM-replacement).

- Transport: `POST https://<workspace>.slack.com/api/client.boot`, form-encoded.
  OBSERVED: beeper/slackgo/users.go:642 (`api.postMethod(ctx, "client.boot", values, response)`)
- Visibility: private.
- Request params:
  - `token` - string - required
    OBSERVED: beeper/slackgo/users.go:635-636
  - `flannel_api_ver` - "4" - optional - edge-cache protocol version selector
    OBSERVED: beeper/slackgo/users.go:636
- Response excerpt (ClientBootResponse):
  OBSERVED: beeper/slackgo/users.go:171-183

```json
{
  "ok": true,
  "self": { "id": "U...", "name": "...", "created": 1600000000,
            "manual_presence": "active", "prefs": { "...676 keys..." } },
  "team": { "id": "T...", "name": "...", "domain": "...", "icon": {...} },
  "is_open": ["C...", "D..."],
  "channels_latest": { "C...": "1699999999.000100" },
  "channels": [ { "...channel object...", "latest": "1699999999.000100" } ],
  "ims":      [ { "...im object...",    "latest": "1699999999.000100" } ]
}
```

- Pagination: none.
- Used for: legacy/simple client boot; slackgo comment "returns info needed to start a Slack client" (users.go:626). The live web client now uses `client.userBoot`; keep as fallback.
- OBSERVED: beeper/slackgo/users.go:626-652

### client.counts

Unread/mention state for every open conversation plus thread-level counts. The web client calls it at boot and periodically to reconcile badges; mautrix-slack calls it as the first step of reconnect resync.

- Transport: `POST https://<workspace>.slack.com/api/client.counts`, form-encoded.
  OBSERVED: beeper/slackgo/users.go:769 (`api.postMethod(ctx, "client.counts", values, response)`)
  OBSERVED: rusq/slackdump v4 internal/edge/client.go:64 (`cl.PostForm(ctx, "client.counts", ...)`)
- Visibility: private.
- Request params:
  - `token` - string - required
    OBSERVED: beeper/slackgo/users.go:750
  - `thread_counts_by_channel` - "true" - optional - include per-channel thread unread counts
    OBSERVED: beeper/slackgo/users.go:751-753; rusq/slackdump v4 internal/edge/client.go:30,58
  - `org_wide_aware` - "true" - optional - include org-wide-channel state (Grid)
    OBSERVED: beeper/slackgo/users.go:754-756; rusq/slackdump v4 internal/edge/client.go:31,59
  - `include_file_channels` - "true" - optional - include file-comment channels
    OBSERVED: beeper/slackgo/users.go:757-759; rusq/slackdump v4 internal/edge/client.go:32,60
  - `_x_reason` - e.g. "client-counts-api/fetchClientCounts"
    OBSERVED: rusq/slackdump v4 internal/edge/client.go:61
- Response excerpt:
  OBSERVED: beeper/slackgo/users.go:727-742 (ClientCountsChannel/ClientCountsResponse; comment: "also contains threads, channel_badges, file_channels, alerts and saved")
  OBSERVED: rusq/slackdump v4 internal/edge/client.go:36-50
  OBSERVED: .omo/research/findings.md:94-103 (live capture: `channels: list[264]`, `ims: list[38]`, `mpims: list[0]`; "Items included id, latest, last_read, mention_count, has_unreads, history_invalid, and updated")
  OBSERVED: emacs-slack/emacs-slack slack-counts.el:31,34-51 (threads {has_unreads, mention_count} + per-conversation {id, has_unreads, mention_count, latest})

```json
{
  "ok": true,
  "channels": [ { "id": "C...", "last_read": "1699999999.000100",
                  "latest": "1700000000.000100", "updated": "1700000000",
                  "history_invalid": "0", "mention_count": 2, "has_unreads": true } ],
  "ims":   [ { "...same shape..." } ],
  "mpims": [ { "...same shape..." } ],
  "threads": { "has_unreads": true, "mention_count": 3 },
  "channel_badges": {}, "file_channels": [], "alerts": {}, "saved": {}
}
```

- Pagination: none.
- Used for: sidebar badges/unread state; after reconnect, the `latest` values decide which channels need history resync (see Gotchas).
- OBSERVED: mautrix/slack pkg/connector/client.go:567-599 (getLatestMessageIDs reads exactly channels+mpims+ims `latest`/`last_read`)

### client.shouldReload

Deployment/rollback check. The very first boot call: "is my client build stale?" The server compares the client's version ts (sent as `_x_version_ts` on every request) against the current deployment.

- Transport: `POST https://<workspace>.slack.com/api/client.shouldReload` (form-encoded; token in body).
  OBSERVED: .omo/research/gist.md:156 (boot call #1); .omo/research/gist.md:636 ("the server compares `_x_version_ts` against the current deployment and tells the client to reload if needed"); .omo/research/findings.md:51 (observed in the startup call burst)
- Visibility: private.
- Request params: `token`; version identity travels in the shared `_x_version_ts`/`_x_frontend_build_type` query params (same values as every other call in the burst).
  OBSERVED: .omo/research/gist.md:82-94 (`_x_version_ts`, `_x_frontend_build_type`, `fp` query params on boot-phase calls)
- Response: boolean reload verdict + min/recommended versions (same fields appear in the `client.userBoot` response, so a client can rely on userBoot alone):
  OBSERVED: beeper/slackgo/users.go:215-217 (`ShouldReload`, `ClientMinVersion`, `RecommendedBuildVersion` in ClientUserBootResponse)
  OBSERVED: rusq/slackdump v4 internal/edge/client_boot.go:110-112 (`should_reload`, `client_min_version`, `build_version_enabled`)
- Pagination: none.
- Used for: detecting stale clients / server-forced reload; if `should_reload` is true or `client_min_version` > your build, re-fetch `https://app.slack.com/client` for fresh version data.
- OBSERVED: .omo/research/gist.md:156; .omo/research/gist.md:636

### client.dms

Paged list of the user's DMs and MPIMs with latest-message info; powers the DM tab of the sidebar.

- Transport: `POST https://<workspace>.slack.com/api/client.dms`, form-encoded.
  OBSERVED: rusq/slackdump v4 internal/edge/client.go:146 (`cl.PostFormRaw(ctx, cl.webapiURL("client.dms"), ...)`)
- Visibility: private.
- Request params:
  - `token` - string - required
    OBSERVED: rusq/slackdump v4 internal/edge/client.go:122,137
  - `count` - int - optional - page size (slackdump sends 250)
    OBSERVED: rusq/slackdump v4 internal/edge/client.go:123,138
  - `include_closed` - bool - optional - include closed conversations
    OBSERVED: rusq/slackdump v4 internal/edge/client.go:124,139
  - `include_channel` - bool - optional
    OBSERVED: rusq/slackdump v4 internal/edge/client.go:125,140
  - `exclude_bots` - bool - optional
    OBSERVED: rusq/slackdump v4 internal/edge/client.go:126,141
  - `cursor` - string - optional - pagination cursor from previous page
    OBSERVED: rusq/slackdump v4 internal/edge/client.go:127,142,157-159
  - `_x_reason` - e.g. "dms-tab-populate"
    OBSERVED: rusq/slackdump v4 internal/edge/client.go:128,143
- Response excerpt:
  OBSERVED: rusq/slackdump v4 internal/edge/client.go:112-133 (clientDMsResponse/ClientDM/IM)

```json
{
  "ok": true,
  "ims": [
    {
      "id": "D...",
      "channel": {
        "id": "D...",
        "created": 1650000000,
        "is_im": true,
        "is_org_shared": false,
        "context_team_id": "T...",
        "updated": 1699999999,
        "user": "U...",
        "last_read": "...",
        "latest": "...",
        "is_open": true
      },
      "latest": "1700000000.037269"
    }
  ],
  "mpims": [],
  "response_metadata": { "next_cursor": "" }
}
```

- Pagination: `cursor` / `response_metadata.next_cursor` (slackdump loops until empty).
  OBSERVED: rusq/slackdump v4 internal/edge/client.go:150-161
- Used for: complete DM/MPIM sidebar section (the DM tab); `client.userBoot`'s `ims` array is a partial view, not the whole set.
  OBSERVED: .omo/research/findings.md:36-41, 75-77 (userBoot ims was only 38 entries; sidebar can be larger)
- OBSERVED: rusq/slackdump v4 internal/edge/client.go:115-162

### rtm.connect

Public, documented, deprecated-for-new-apps method that returns a one-shot websocket URL plus compact identity. Alternative to dialing `wss-primary` directly; the session-token URL in the web client is formed client-side instead.

- Transport: `POST https://slack.com/api/rtm.connect` (also GET), form-encoded, token in body or header.
  OBSERVED: beeper/slackgo/rtm.go:92-108 (`api.postMethod(ctx, "rtm.connect", ...)`)
  OBSERVED: https://docs.slack.dev/reference/methods/rtm.connect/ (public docs page: "Starts a Real Time Messaging session", Tier 1 rate limit, no scopes required)
- Visibility: public.
- Request params:
  - `token` - string - required
    OBSERVED: beeper/slackgo/rtm.go:106 (`url.Values{"token": {api.token}}`); docs page
  - `batch_presence_aware` - "1" - optional - "Batch presence deliveries via subscription. Enabling changes the shape of presence_change events."
    OBSERVED: dylex/slack-libpurple slack-rtm.c:241-243 (`"rtm.connect", "batch_presence_aware", "1", "presence_sub", "true"`)
    OBSERVED: https://docs.slack.dev/reference/methods/rtm.connect/ (optional arg list)
  - `presence_sub` - "true" - optional - subscribe to presence of all visible users (documented as an optional arg on the public docs page; used by libpurple)
    OBSERVED: dylex/slack-libpurple slack-rtm.c:241-243
    OBSERVED: https://docs.slack.dev/reference/methods/rtm.connect/ (presence_sub listed under Optional arguments)
- Response excerpt (captured mock):
  OBSERVED: wee-slack/wee-slack mock_data/slack_rtm_connect.json (whole file)
  OBSERVED: https://docs.slack.dev/reference/methods/rtm.connect/ ("The url property contains a WebSocket Message Server URL... only valid for 30 seconds"; "self... an id and their name"; "team... id, name, domain, and if it's part of an Enterprise organization, the corresponding enterprise_id")

```json
{
  "ok": true,
  "url": "wss://wss-primary.slack.com/websocket/...",
  "team": {
    "id": "T0FC8BFQR",
    "name": "wee-slack-test",
    "domain": "wee-slack-test"
  },
  "self": { "id": "U7JNGMGEB", "name": "trygveaa" }
}
```

- Pagination: none.
- Used for: obtaining a websocket URL when you do not want to form the flannel URL yourself; wee-slack uses it for classic (non-session) tokens, mautrix/slackgo uses it as the RTM entry point.
  OBSERVED: wee-slack/wee-slack slack_workspace.py:389-394 (`fetch_rtm_connect()` path for non-session tokens); beeper/slackgo/rtm.go:92
- OBSERVED: beeper/slackgo/rtm.go:92-108

### rtm.start

The deprecated heavyweight variant: returns the websocket URL AND the entire workspace state (channels, users, ims, groups...) in one call. Documented as deprecated ("rtm-start-to-stop" changelog); still functional.

- Transport: `POST https://slack.com/api/rtm.start`, form-encoded.
  OBSERVED: beeper/slackgo/rtm.go:57-76 (`api.postMethod(ctx, "rtm.start", url.Values{"token": {api.token}}, response)`)
- Visibility: public, deprecated.
  OBSERVED: beeper/slackgo/rtm.go:42-44 ("Deprecated: Use [ConnectRTM] instead. For more details, see: https://api.slack.com/changelog/2021-10-rtm-start-to-stop")
- Request params: `token` - string - required.
  OBSERVED: beeper/slackgo/rtm.go:72
- Response: same `url`/`self`/`team` as rtm.connect plus full workspace snapshot (slackgo decodes it into the same `Info` wrapper; the massive payload is why it was deprecated).
  OBSERVED: beeper/slackgo/info.go:440-449 (`Info{URL, Self *UserDetails, Team *Team}` returned by both rtm.start and rtm.connect)
- Pagination: none.
- Used for: legacy clients; wee-slack RTM mode historically. Avoid.
- OBSERVED: beeper/slackgo/rtm.go:53-76

### subscriptions.thread.mark

Session-token-only method to update the read cursor of one thread; this is what generates/echoes `thread_marked`/thread state on the socket.

- Transport: `POST https://<workspace>.slack.com/api/subscriptions.thread.mark`, form-encoded.
  OBSERVED: wee-slack/wee-slack slack_api.py:449-462
- Visibility: private ("only available for session tokens" - xoxc/xoxd).
  OBSERVED: wee-slack/wee-slack slack/slack_thread.py:127 ("subscriptions.thread.mark is only available for session tokens")
- Request params:
  - `token` - string - required (session token)
  - `channel` - string - required - conversation id
  - `thread_ts` - string - required - parent message ts
  - `ts` - string - required - new read cursor (last read reply ts)
  - `read` - 1 - optional - mark read
    OBSERVED: wee-slack/wee-slack slack_api.py:452-458 (`{"channel": ..., "thread_ts": ..., "ts": ..., "read": 1}`)
- Response: `{"ok": true}`.
  OBSERVED: wee-slack/wee-slack slack_api.py:459-461 (checks `response["ok"]`)
- Pagination: none.
- Used for: marking a thread read; pairs with the `thread_marked` socket event whose `subscription.last_read` carries the same cursor.
  OBSERVED: beeper/slackgo websocket_channels.go:79-95 (ThreadSubscription{channel, thread_ts, date_created, active, last_read})
- OBSERVED: wee-slack/wee-slack slack_api.py:449-462

### wss://wss-primary.slack.com/ (message server socket)

The realtime socket. Two equivalent ways in: form the flannel URL client-side (what the web desktop client and slackdump do), or get a one-shot URL from `rtm.connect`/`rtm.start`.

- Transport: WebSocket (wss). Handshake headers that work:
  - `Origin: https://app.slack.com`
    OBSERVED: beeper/slackgo websocket_managed_conn.go:249-250 (`upgradeHeader.Add("Origin", "https://app.slack.com")`)
  - `Cookie: d=<xoxd value>` (session-token auth)
    OBSERVED: dylex/slack-libpurple slack-rtm.c:193-199 (`cookie = g_strconcat("d=", sa->d_cookie, NULL)` passed to the websocket connect)
    OBSERVED: beeper/slackgo websocket_managed_conn.go:255-259 (installs `rtm.cookies` into the dialer cookie jar for slack.com)
  - modern browser-like `User-Agent` (slackdump pairs requests with `slackauth.DefaultUserAgent`)
    OBSERVED: rusq/slackdump v4 internal/edge/edge.go:283 (`User-Agent`, `slackauth.DefaultUserAgent`); rusq/slackdump v4 internal/presence/keeper.go:180 ("header contains the headers to send on the websocket handshake (i.e. Cookie, User-Agent, Origin)")
- Visibility: private.
- URL, full parameter set (query string on `wss://wss-primary.slack.com/`):
  OBSERVED: rusq/slackdump v4 internal/presence/keeper.go:225-247 (`FlannelURL` builds exactly this; "as used by the Slack web client")
  OBSERVED: beeper/slackgo websocket_managed_conn.go:227-243 (same param set in the slackgo source as commented-out `start_args`/`sync_desync`/`flannel`/`lazy_channels`/`gateway_server`/`batch_presence_aware` entries)
  OBSERVED: wee-slack/wee-slack slack_workspace.py:388 (`token`, `gateway_server=<team_id>-1`, `slack_client=desktop`, `batch_presence_aware=1`)
  OBSERVED: .omo/research/findings.md:108-110 (live web client capture showed `lazy_channels=1` and `no_query_on_subscribe=1`)

```
wss://wss-primary.slack.com/?token=<xoxc->            user session token
  &sync_desync=1                                      sync flag (slackgo comment + slackdump)
  &slack_client=desktop                               client identity string
  &start_args=%3Fagent%3Dclient%26org_wide_aware%3Dtrue%26eac_cache_ts%3Dtrue
    %26cache_ts%3D0%26name_tagging%3Dtrue%26only_self_subteams%3Dtrue
    %26connect_only%3Dtrue%26ms_latest%3Dtrue         nested query: boot agent options
  &no_query_on_subscribe=1                            don't re-query on channel subscribe
  &flannel=3                                          flannel protocol version
  &lazy_channels=1                                    defer channel hydration to client.counts/API
  &gateway_server=<team_id>-1                         message-server shard for the team
  &batch_presence_aware=1                             presence_change arrives batched (users[] array)
```

- `start_args` decoded: `?agent=client&org_wide_aware=true&eac_cache_ts=true&cache_ts=0&name_tagging=true&only_self_subteams=true&connect_only=true&ms_latest=true`
  OBSERVED: rusq/slackdump v4 internal/presence/keeper.go:214-224; beeper/slackgo websocket_managed_conn.go:236
- `gateway_server` value `<team_id>-1` also observed for org-level teams (wee-slack uses `self.id` which it sets to the enterprise id when the team is org-level).
  OBSERVED: wee-slack/wee-slack slack_workspace.py:374-388
- Outgoing frames (all JSON text frames):
  - ping: `{"id":N,"type":"ping"}` every ~10-30s; server replies `{"type":"pong","reply_to":N}` (reply_to echoes your id).
    OBSERVED: rusq/slackdump v4 internal/presence/keeper.go:23-25 (doc comment: `{"type":"ping","id":N}` every 10 seconds, reply `{"type":"pong","reply_to":N}`), keeper.go:66 (DefaultPing = 10s)
    OBSERVED: beeper/slackgo rtm.go:14 (`defaultPingInterval = 30 * time.Second`), websocket_managed_conn.go:396 + messages.go:213-217 (`Ping{ID, Type: "ping", Timestamp}`), websocket_misc.go:9-16 (AckMessage{reply_to, ts, text, ok})
    OBSERVED: emacs-slack/emacs-slack slack-websocket.el:224-231 (`(ping-message (list :id message-id :type "ping" ...))`)
    OBSERVED: wee-slack/wee-slack slack_workspace.py:899-911 (`self.ws_send({"type": "ping"})`)
  - tickle (activity keepalive): `{"id":N,"type":"tickle","reason":"focus"|"mousedown"|"keydown"|"polling"}`. No server reply. Throttled to at most one per 19s; keeps you "active". Desktop app polls OS idle time and sends `polling`.
    OBSERVED: rusq/slackdump v4 internal/presence/keeper.go:24-31,52-66 (reasons, MinInterval = 19s, DefaultInterval = 4min, DefaultReason = "polling"; "An open connection alone does not keep the user active: ... auto_away about 30 minutes after the last tickle")
    OBSERVED: wee-slack/wee-slack slack_workspace.py:916-919 (`{"type": "tickle"}` at most once per 20s)
    OBSERVED: dylex/slack-libpurple slack-rtm.c:118-129 (60s timer sends "tickle" when active, else websocket-level PONG)
  - presence_sub: `{"id":N,"type":"presence_sub","ids":["U...","U..."]}` - subscribe to presence for specific users; sent right after `hello`; delivery arrives as (batched) `presence_change`.
    OBSERVED: rusq/slackdump v4 internal/presence/keeper.go:392-395 (handle "hello" -> `outMsg{Type: "presence_sub", IDs: []string{k.userID}}`)
    OBSERVED: dylex/slack-libpurple slack-rtm.c:241-243 (requests it at rtm.connect time via `presence_sub=true` + `batch_presence_aware=1`)
  - user_typing (typing indicator): `{"id":N,"type":"user_typing","channel":"C...","thread_ts":"..."}`
    OBSERVED: wee-slack/wee-slack slack_workspace.py:921-928 (`{"type": "user_typing", "channel": ...}` plus `thread_ts` when typing in a thread)
- Incoming control frames: `hello`, `pong`, `reconnect_url`, `goodbye`, `error` - documented under Websocket events below.
- Reconnect/resync after disconnect (worked recipe):
  1. Treat `goodbye` or read error as disconnect; reconnect with exponential backoff + jitter; reset the backoff after ~1 min of healthy connection. Respect `Retry-After` on HTTP 429 during boot calls.
     OBSERVED: rusq/slackdump v4 internal/presence/keeper.go:268-284 (`Run`: backoff `min(backoff*2, maxBackoff)`, reset after `healthySession = time.Minute`), keeper.go:71-73 (min 1s, max 1min)
     OBSERVED: mautrix/slack pkg/connector/client.go:246-254 (`connectRetryDelay`: `2s<<attempt` capped at 5min, jittered, raised to `RetryAfter` on rate limit)
     OBSERVED: beeper/slackgo websocket_managed_conn.go:63-75 (abort after >20 quick reconnections), 142-146 (`boff` max 5min), 205-214 (`connectionCount = 0` if the session lasted > 1min)
  2. If you still hold a `reconnect_url`, dial it first (hello then arrives with `fast_reconnect: true`; resume without full state reload). Error `code: 1` ("Socket URL has expired") invalidates the stored URL.
     OBSERVED: beeper/slackgo websocket_managed_conn.go:225-228 (`u := rtm.reconnectURL` preferred), 526-531 (store `reconnect_url` from event)
     OBSERVED: wee-slack/wee-slack slack_workspace.py:367-373 (reconnect dials `_reconnect_url` first), 679-690 (`hello.fast_reconnect` gates full re-init; code 1 clears the URL), 692-694 (stores reconnect_url)
  3. On hello after a real disconnect: re-send `presence_sub`, re-tickle.
     OBSERVED: rusq/slackdump v4 internal/presence/keeper.go:392-401
  4. Resync missed messages: call `client.counts`, compare each channel's `latest` with your last-seen ts, then fetch `conversations.history` (cross-area method) for channels whose `latest` moved forward.
     OBSERVED: mautrix/slack pkg/connector/client.go:567-599 (getLatestMessageIDs from client.counts), client.go:738-754 (catchUpAfterReconnect -> queueCatchUpResync per portal), pkg/connector/handleslack.go:101-110 (trigger: hello with `resume == false` after a previous hello)
- Pagination: n/a.
- Used for: all live message/reaction/typing/presence/notification delivery.
- OBSERVED: rusq/slackdump v4 internal/presence/keeper.go:225-247; beeper/slackgo websocket_managed_conn.go:222-262

## Websocket events

Every frame is one JSON object. Control frames use `type`; most state frames add `channel`/`user`/`event_ts`. Send-side frames (ping/tickle/presence_sub/user_typing) are documented under the socket endpoint above. Struct excerpts are Go field lists from slackgo's RTM decoder (fields map 1:1 to JSON keys).

### hello

Fires once the socket is really connected. Fields: `type`, `fast_reconnect` (bool, true when you reconnected via reconnect_url and server kept your session state). slackgo tracks `resume` client-side for the same purpose.

```json
{ "type": "hello", "fast_reconnect": false }
```

OBSERVED: wee-slack/wee-slack slack_workspace.py:679-685 (`data["fast_reconnect"]` gates full re-initialization)
OBSERVED: beeper/slackgo websocket_managed_conn.go:456-463 (HelloEvent{Resume} handling); beeper/slackgo rtm.go:24 (`rtmEventTypeHello = "hello"`)
OBSERVED: mautrix/slack pkg/connector/handleslack.go:101-110 (hello with resume=false after a previous hello triggers catch-up resync)

### goodbye

Server-requested reconnect: the connection is finished, dial again (fresh URL if needed).

```json
{ "type": "goodbye" }
```

OBSERVED: beeper/slackgo rtm.go:20 (`rtmEventTypeGoodbye`), websocket_managed_conn.go:318-327 (goodbye -> killConnection(false, ErrRTMGoodbye) then reconnect loop)
OBSERVED: rusq/slackdump v4 internal/presence/keeper.go:81 (`ErrGoodbye = errors.New("server said goodbye")`), keeper.go:416-417

### reconnect_url

Periodic/new websocket URL to use for the next reconnect (the current URL expires).

```json
{ "type": "reconnect_url", "url": "wss://wss-primary.slack.com/websocket/..." }
```

OBSERVED: beeper/slackgo websocket_managed_conn.go:524-531 (ReconnectUrlEvent{URL} -> rtm.reconnectURL), websocket_misc.go:153-157
OBSERVED: wee-slack/wee-slack slack_workspace.py:692-694, 245

### pong

Reply to your ping; `reply_to` is your ping `id`. Use for latency + liveness (3 missed pongs -> reconnect).

```json
{ "type": "pong", "reply_to": 7 }
```

OBSERVED: rusq/slackdump v4 internal/presence/keeper.go:23-25, 403-405, 366-370 (lastPong check, missedPongs = 3)
OBSERVED: beeper/slackgo websocket_managed_conn.go:536-550 (handlePong computes latency), messages.go:220-224 (Pong struct)

### error

Socket-level error. `code: 1` = socket URL expired (clear reconnect_url). `code: 17` = rate limited ("slow down, too many messages"); sleep ~1 min. `code: 401` = invalid auth (kill connection).

```json
{ "type": "error", "error": { "code": 1, "msg": "Socket URL has expired" } }
```

OBSERVED: wee-slack/wee-slack slack_workspace.py:685-688 (code 1 handling)
OBSERVED: beeper/slackgo websocket_managed_conn.go:495-520 (codes 1/17/401; msg string `"slow down, too many messages..."` for code -1 acks at websocket_managed_conn.go:478-484)

### message (+ every subtype)

The workhorse. Base fields (Msg): `client_msg_id`, `type`, `channel`, `user`, `text`, `ts`, `thread_ts`, `is_starred`, `pinned_to`, `attachments`, `edited`, `last_read`, `subscribed`, `unread_count`, `subtype`, `hidden`, `deleted_ts`, `event_ts`, `bot_id`, `username`, `icons`, `bot_profile`, `trigger_id`, `workflow_id`, `inviter`, `topic`, `purpose`, `name`, `old_name`, `members`, `reply_count`, `reply_users`, `replies`, `parent_user_id`, `latest_reply`, `files`, `upload`, `comment`, `item_type`, plus `message`, `previous_message`, `root` wrappers on the event envelope.
OBSERVED: beeper/slackgo messages.go:60-135 (Msg struct field-for-field), messages.go:49-59 (MessageEvent = Message{Msg, SubMessage *Msg `json:"message"`, PreviousMessage *Msg `json:"previous_message"`, Root *Msg `json:"root"`})

```json
{
  "type": "message",
  "channel": "C...",
  "user": "U...",
  "text": "hello",
  "ts": "1700000000.000100",
  "event_ts": "1700000000.000100",
  "channel_type": "channel"
}
```

Subtypes (the full catalog from slackgo constants, each arrives as `type:"message", subtype:"<name>"`; "[RTM]" ones are observed on this socket):
OBSERVED: beeper/slackgo messages.go:27-135

- plain message (no subtype), `bot_message` (bot_id/username/icons/bot_profile), `me_message`
- `message_changed` - edit; carries `hidden:true`, `message:{...new Msg...}`, `previous_message:{...old Msg...}`, `client_msg_id` of the edit
  OBSERVED: wee-slack/wee-slack slack_workspace.py:830-833 (routes to change_message); beeper/slackgo messages.go:30,49-59
- `message_deleted` - delete; carries `hidden:true`, `deleted_ts:"<ts>"` (no `message` body)
  OBSERVED: beeper/slackgo messages.go:31,81-82 (`deleted_ts` "message_deleted"); wee-slack slack_workspace.py:834-836
- `message_replied` - a thread got a reply; carries `thread_ts`, `reply_count`, `replies`, `latest_reply`, `parent_user_id` (and the reply itself when the channel is open)
  OBSERVED: wee-slack/wee-slack slack_workspace.py:837-839; beeper/slackgo messages.go:32,117-122
- `thread_broadcast` - a thread reply broadcast to the channel; `root` carries the parent message
  OBSERVED: beeper/slackgo messages.go:33-34,53-55
- `reply_broadcast` - deprecated predecessor of thread_broadcast
  OBSERVED: beeper/slackgo messages.go:33 ("@Deprecated (No longer served)")
- `channel_join`/`channel_leave` (+`group_join`/`group_leave`) - membership notices; `inviter` set on joins
  OBSERVED: beeper/slackgo messages.go:35-48,101-102
- `channel_topic`/`channel_purpose`/`channel_name` (+`group_*` variants) - carry `topic`/`purpose`/`name`+`old_name`
  OBSERVED: beeper/slackgo messages.go:37-46,104-112; mautrix/slack pkg/connector/handleslack.go:273-282 (treats all six as chat-info resync)
- `channel_archive`/`channel_unarchive`, `group_archive`/`group_unarchive` - carry `members` list
  OBSERVED: beeper/slackgo messages.go:41-48,114-115
- `file_share` (`files[]`, `upload`), `file_comment` (`comment`), `file_mention`
  OBSERVED: beeper/slackgo messages.go:49-51,124-131
- `pinned_item`/`unpinned_item` (`hidden:true`, `item_type`, `attached_to`, `pinned_info`)
  OBSERVED: beeper/slackgo messages.go:52-53,81,133-135
- `ekm_access_denied` (Enterprise Key Management redaction), `channel_posting_permissions`, `assistant_app_thread`
  OBSERVED: beeper/slackgo messages.go:54-56
- `huddle_thread` (arrives inside message_changed on huddle threads; carries `room` with the huddle state incl. `has_ended`, `date_end`)
  OBSERVED: mautrix/slack pkg/connector/handleslack.go:253-271
- Messages for huddle rooms carry `room` directly (SHRoom shape under sh_room_* below).
  OBSERVED: mautrix/slack pkg/connector/handleslack.go:294-296 (`evt.Room != nil` sender fallback)
  Used for: rendering all chat content; `hidden:true` subtypes are UI metadata, never render as messages.
  OBSERVED: beeper/slackgo messages.go:81 (`Hidden bool // message_changed, message_deleted, unpinned_item`)

### presence_change

Presence for one user or a batch. Shape depends on `batch_presence_aware`: without it `user` is a string; with it `users` is an array. `presence` is `active`|`away`.

```json
{ "type": "presence_change", "user": "U...", "presence": "active" }
{ "type": "presence_change", "users": ["U...","U..."], "presence": "away" }
```

OBSERVED: beeper/slackgo websocket_misc.go:61-68 (PresenceChangeEvent has BOTH `User string` and `Users []string`)
OBSERVED: dylex/slack-libpurple slack-user.c:173-186 (`users` array or `user` string, both handled)
OBSERVED: dylex/slack-libpurple slack-rtm.c:26-28 (`presence_change` and `presence_change_batch` both dispatched to the same handler)

### user_typing

Someone is typing in a channel/thread (5s TTL).

```json
{ "type": "user_typing", "channel": "C...", "user": "U..." }
```

OBSERVED: beeper/slackgo websocket_misc.go:60-64; mautrix/slack pkg/connector/handleslack.go:323-328 (Timeout: 5 * time.Second)

### manual_presence_change

Your own manual active/away toggle.

```json
{ "type": "manual_presence_change", "presence": "away" }
```

OBSERVED: beeper/slackgo websocket_misc.go:76-80; beeper/slackgo websocket_managed_conn.go:662 (EventMapping)
OBSERVED: rusq/slackdump v4 internal/presence/keeper.go:410-415 (handles away vs active)

### channel_marked / im_marked / group_marked / mpim_marked

Read cursor moved (usually your own action echoed back, but also fires for other devices). Payload: `channel`, `ts` (the new `last_read`), `event_ts`.

```json
{
  "type": "channel_marked",
  "channel": "C...",
  "ts": "1700000000.000100",
  "event_ts": "..."
}
```

OBSERVED: beeper/slackgo websocket_channels.go:45-52 (ChannelInfoEvent{Type, Channel, User, ts} + `ChannelMarkedEvent ChannelInfoEvent`), websocket_dm.go:20-24, websocket_groups.go:14-16
OBSERVED: mautrix/slack pkg/connector/handleslack.go:330-340 (channel_marked/im_marked/group_marked/thread_marked -> read receipts, `evt.Timestamp` used as the cursor)

### thread_marked / thread_subscribed / thread_unsubscribed

Thread-level read state and subscription changes. Payload is a `subscription` object.

```json
{
  "type": "thread_marked",
  "subscription": {
    "type": "thread",
    "channel": "C...",
    "thread_ts": "1699999000.000100",
    "date_created": 1699999000,
    "active": true,
    "last_read": "1699999999.000100"
  },
  "event_ts": "..."
}
```

OBSERVED: beeper/slackgo websocket_channels.go:79-95 (ThreadSubscription + ThreadInfoEvent; ThreadMarkedEvent/ThreadSubscribedEvent/ThreadUnsubscribedEvent)
OBSERVED: wee-slack/wee-slack slack_workspace.py:755-764 (subscription.type=="thread", channel lookup), 847-866 (marked/subscribed/unsubscribed handling)
OBSERVED: emacs-slack/emacs-slack slack-websocket.el:536-543 (thread_marked/thread_subscribed/thread_unsubscribed handlers)

### channel_created

```json
{
  "type": "channel_created",
  "channel": {
    "id": "C...",
    "is_channel": true,
    "name": "new-channel",
    "created": 1700000000,
    "creator": "U..."
  },
  "event_ts": "..."
}
```

OBSERVED: beeper/slackgo websocket_channels.go:6-21

### channel_rename / group_rename

```json
{
  "type": "channel_rename",
  "channel": { "id": "C...", "name": "renamed", "created": 1700000000 },
  "event_ts": "..."
}
```

OBSERVED: beeper/slackgo websocket_channels.go:23-36; emacs-slack slack-websocket.el:490-493

### channel_joined / group_joined

Full channel object arrives (you joined or were added). `{"type":"channel_joined","channel":{...Channel...}}`
OBSERVED: beeper/slackgo websocket_channels.go:10-13 (ChannelJoinedEvent{Channel Channel}); emacs-slack slack-websocket.el:496-502

### channel_left / channel_deleted / channel_archive / channel_unarchive / group_left / group_archive / group_unarchive

Lightweight notices; payload is the ChannelInfoEvent shape (`channel`, optional `user`/`ts`).
OBSERVED: beeper/slackgo websocket_channels.go:53-73; emacs-slack slack-websocket.el:494-495 (archive/unarchive), 486-489 (deleted/left)

### channel_updated

Channel property update (tabs, canvas, etc.); carries an `updates` map and can address multiple channels.

```json
{ "type": "channel_updated", "channel": "C...", "channels": ["C..."],
  "updates": { "...property updates..." }, "ts": "...", "event_ts": "..." }
```

OBSERVED: beeper/slackgo websocket_channels.go:74-87, websocket_misc.go:190-197; mautrix/slack pkg/connector/handleslack.go:377-380 (handled as remote event / chat-info resync)

### channel_history_changed / im_history_changed / group_history_changed

History was rewritten server-side (compliance exports, admin deletion): drop cached history and refetch `conversations.history` from `latest`.

```json
{
  "type": "channel_history_changed",
  "latest": "1700000000.000100",
  "ts": "...",
  "event_ts": "..."
}
```

OBSERVED: beeper/slackgo websocket_channels.go:38-43 (also im_marked... im_history_changed at websocket_dm.go:15-16, group_history_changed websocket_groups.go:56-57); beeper/slackgo websocket_managed_conn.go:598,612,630 (EventMapping entries)
OBSERVED: .omo/research/findings.md:59 (`conversations.historyChanges` observed in the live client's boot-call burst - the web client performs the same reconciliation at boot)

### im_created / im_open / im_close / group_open / group_close / group_joined

DM/MPIM lifecycle. `im_created` carries `user` + full `channel`; open/close carry the channel id.
OBSERVED: beeper/slackgo websocket_dm.go:6-24 (IMCreatedEvent{User, Channel}, IMOpenEvent/IMCloseEvent = ChannelInfoEvent); websocket_groups.go:18-39
OBSERVED: wee-slack/wee-slack slack_workspace.py:796-810 (im_open/mpim_open/group_open triggers conversation open + buffer)

### member_joined_channel / member_left_channel

Any member join/leave on public or private channels (not a message subtype).

```json
{
  "type": "member_joined_channel",
  "user": "U...",
  "channel": "C...",
  "channel_type": "C",
  "team": "T...",
  "inviter": "U...",
  "event_ts": "..."
}
```

OBSERVED: beeper/slackgo websocket_misc.go:160-176 (both events; `channel_type` is "C"/"G")

### reaction_added / reaction_removed

```json
{
  "type": "reaction_added",
  "user": "U...",
  "reaction": "thumbsup",
  "item": { "type": "message", "channel": "C...", "ts": "1700000000.000100" },
  "item_user": "U...",
  "event_ts": "1700000001.000005"
}
```

`item.type` can also be `file`/`file_comment` (then `file`/`file_comment` ids replace channel/ts).
OBSERVED: beeper/slackgo websocket_reactions.go:5-25 (ReactionItem{Type, Channel, File, FileComment, ts}, ReactionEvent{User, ItemUser, Item, Reaction})
OBSERVED: wee-slack/wee-slack slack_workspace.py:752-753 (channel id taken from `data["item"]["channel"]`)
OBSERVED: emacs-slack slack-websocket.el:467-470

### pin_added / pin_removed

```json
{
  "type": "pin_added",
  "user": "U...",
  "item": { "type": "message", "...": "..." },
  "channel_id": "C...",
  "event_ts": "...",
  "has_pins": true
}
```

OBSERVED: beeper/slackgo websocket_pins.go:4-15 (note the JSON key is `channel_id`, not `channel`)
OBSERVED: emacs-slack slack-websocket.el (pins handled via slack-pinned-item.el; pin_added dispatched in slack-ws-on-message)

### star_added / star_removed

```json
{
  "type": "star_added",
  "user": "U...",
  "item": { "type": "message", "...": "..." },
  "event_ts": "..."
}
```

OBSERVED: beeper/slackgo websocket_stars.go:4-14 (StarredItem payload); beeper/slackgo websocket_managed_conn.go:626-627 (EventMapping)

### pref_change

One of your own prefs changed; `name` + `value` (value shape varies per pref - string, bool, or JSON).

```json
{ "type": "pref_change", "name": "muted_channels", "value": "C01,C02" }
```

OBSERVED: beeper/slackgo websocket_misc.go:76-80 (PrefChangeEvent{Name, Value json.RawMessage})
OBSERVED: wee-slack/wee-slack slack_workspace.py:696-703 (`muted_channels` and `all_notifications_prefs` handled)

### dnd_updated / dnd_updated_user

Do-not-disturb state changed for you (`dnd_updated`) or another user (`dnd_updated_user`).

```json
{ "type": "dnd_updated", "user": "U...", "dnd_status": { "...DNDStatus..." } }
```

OBSERVED: beeper/slackgo websocket_dnd.go:6-11 (DNDUpdatedEvent{User, Status DNDStatus `json:"dnd_status"}`); both types map to the same struct (websocket_managed_conn.go:606-607)

### user_change / user_profile_changed / user_status_changed / user_huddle_changed / user_invalidated

User object updates (profile edit, status, huddle participation). First four carry the full `user` object + `cache_ts` + `event_ts`; `user_invalidated` carries only `{ "user": { "id": "U..." } }` - refetch via users/info (edge api, cross-area).

```json
{ "type": "user_change", "user": { "id": "U...", "...": "full user object..." },
  "cache_ts": 1700000000, "event_ts": "..." }
{ "type": "user_invalidated", "user": { "id": "U..." }, "event_ts": "..." }
```

OBSERVED: beeper/slackgo websocket_misc.go:79-115 (all five structs); mautrix/slack pkg/connector/handleslack.go:161-166 (user_change/user_profile_changed -> ghost update; user_invalidated -> refetch)

### team_join / team_rename / team_pref_change / team_domain_change / team_migration_started

Workspace-level events. team_join carries a full `user` object; domain_change carries `url` + `domain`; migration_started means the team is being migrated (stop and re-auth).
OBSERVED: beeper/slackgo websocket_teams.go:5-35; beeper/slackgo websocket_managed_conn.go:637-643 (EventMapping)
OBSERVED: dylex/slack-libpurple slack-rtm.c:51-53 (team_join handled like user_change)

### emoji_changed

Custom emoji set changed. `subtype: "add"` carries `name`+`value` (URL); `remove` carries `names[]`; `rename` carries `old_name`+`new_name`.

```json
{
  "type": "emoji_changed",
  "subtype": "add",
  "name": "parrot",
  "value": "https://emoji.slack-edge.com/...",
  "event_ts": "..."
}
```

OBSERVED: beeper/slackgo websocket_misc.go:117-127 (EmojiChangedEvent{SubType, Name, Names, OldName, NewName, Value})
OBSERVED: wee-slack/wee-slack slack_workspace.py:741-746 (add/remove handling only; no rename branch - rename fields from the slackgo struct)
OBSERVED: mautrix/slack pkg/connector/emoji.go:93 (emoji_changed -> async refetch)

### file_created / file_shared / file_public / file_private / file_unshared / file_change / file_deleted

File lifecycle; all carry the full `file` object; deleted carries `file_id` + `user_id` instead.
OBSERVED: beeper/slackgo websocket_files.go:6-50 (fileActionEvent{File, FileID, UserID})
OBSERVED: emacs-slack slack-websocket.el:506-511 (created/deleted/unshared handlers)
OBSERVED: mautrix/slack pkg/connector/handleslack.go:140-150 (file_created used as upload-completion signal), handleslack.go:156-160 (file_* + sh_room_* explicitly ignored as duplicates)

### file_comment_added / file_comment_edited / file_comment_deleted

Legacy file comments (file channels); carry `comment` (object or, for deleted, its id).
OBSERVED: beeper/slackgo websocket_files.go:31-50

### bot_added / bot_changed

Integration bot records; carry full `bot` object.
OBSERVED: beeper/slackgo websocket_misc.go:143-152 (BotAddedEvent/BotChangedEvent{Bot}); emacs-slack slack-websocket.el:503-505

### subteam_created / subteam_updated / subteam_members_changed / subteam_self_added / subteam_self_removed

User groups. created/updated carry full `subteam`; members_changed carries deltas; self_added/removed carry `subteam_id`.

```json
{
  "type": "subteam_members_changed",
  "subteam_id": "S...",
  "team_id": "T...",
  "date_previous_update": 1699999000,
  "date_update": 1700000000,
  "added_users": ["U..."],
  "added_users_count": 1,
  "removed_users": [],
  "removed_users_count": 0
}
```

OBSERVED: beeper/slackgo websocket_subteam.go:5-45 (all five structs); beeper/slackgo websocket_managed_conn.go:648-656 (EventMapping)
OBSERVED: rusq/slackdump v4 internal/presence/keeper.go (client requests `only_self_subteams=true` in start_args, so self_added/removed are the membership deltas that arrive)

### accounts_changed / apps_uninstalled / commands_changed / email_domain_changed

Misc workspace-wide invalidation events; mostly just `{"type": ...}` (+ `event_ts`), meaning "refetch the affected list".
OBSERVED: beeper/slackgo websocket_misc.go:154-176 (AccountsChangedEvent), websocket_misc.go:270-293 (AppsUninstalledEvent, ActivityEvent, BadgeCountsUpdatedEvent), websocket_managed_conn.go:661-664 (EventMapping)

### badge_counts_updated

Notification badge counts changed (sums across devices). slackgo decodes only `type`; treat as "re-pull client.counts".
OBSERVED: beeper/slackgo websocket_misc.go:294-297 ("badge_counts_updated event sent via RTM when notification badge counts change"); beeper/slackgo websocket_managed_conn.go:664

### activity

Internal Slack activity feed event (fires during normal workspace usage).

```json
{ "type": "activity", "subtype": "...", "key": "...", "entry": {...}, "event_ts": "..." }
```

OBSERVED: beeper/slackgo websocket_misc.go:278-286 ("internal Slack event that fires during normal workspace usage (e.g. new messages, bundle updates)")

### desktop_notification

Server-computed desktop notification for your client. Rich payload:

```json
{
  "type": "desktop_notification",
  "title": "...",
  "subtitle": "...",
  "msg": "...",
  "ts": "1700000000.000100",
  "content": "...",
  "channel": "C...",
  "launchUri": "slack://channel?id=...&message=...",
  "avatarImage": "https://...",
  "ssbFilename": "...",
  "imageUri": "...",
  "is_shared": false,
  "is_channel_invite": false,
  "event_ts": "..."
}
```

OBSERVED: beeper/slackgo websocket_desktop_notification.go:6-24
OBSERVED: mautrix/slack pkg/connector/handleslack.go:156-160 (ignored as duplicate of the underlying message)

### mobile_in_app_notification

Same idea for mobile; carries channel_name, author fields, push_id, mobileLaunchUri.
OBSERVED: beeper/slackgo websocket_mobile_in_app_notification.go:5-23

### sh_room_join / sh_room_leave / sh_room_update (huddles)

Huddle room state machine. Each carries `room` (full SHRoom) + `user` + optional `huddle: {channel_id}`.

```json
{
  "type": "sh_room_join",
  "room": {
    "id": "R...",
    "media_server": "...",
    "created_by": "U...",
    "date_start": 1700000000,
    "date_end": 0,
    "participants": ["U..."],
    "participant_history": ["U..."],
    "participants_camera_on": [],
    "participants_screenshare_on": [],
    "channels": ["C..."],
    "is_dm_call": false,
    "has_ended": false,
    "canvas_thread_ts": "...",
    "app_id": "...",
    "display_id": "..."
  },
  "user": "U...",
  "huddle": { "channel_id": "C..." },
  "event_ts": "...",
  "ts": "..."
}
```

OBSERVED: beeper/slackgo websocket_misc.go:188-271 (SHRoom + SHRoomJoinEvent/SHRoomLeaveEvent/SHRoomUpdateEvent with all fields incl. recording, locale, huddle_link, was_missed/accepted/rejected)
OBSERVED: wee-slack/wee-slack slack_workspace.py:765-766 (huddle channel id from `data["huddle"]["channel_id"]`)

### user_huddle_changed

Full `user` object with updated huddle participation (cross-ref user_change above).
OBSERVED: beeper/slackgo websocket_misc.go:95-101; mautrix/slack pkg/connector/handleslack.go:159 (explicitly ignored)

## Gotchas

- The websocket URL from rtm.connect is valid for 30 seconds only - dial immediately. The client-formed flannel URL does not have this expiry but the _socket_ does expire server-side; the server pushes fresh `reconnect_url` frames periodically and error code 1 means your stored URL died.
  OBSERVED: https://docs.slack.dev/reference/methods/rtm.connect/; wee-slack slack_workspace.py:689-690; beeper/slackgo websocket_managed_conn.go:506-510
- An open socket alone does not keep you "active": slackdump measured auto_away ~30 minutes after the last tickle (docs claim 10). Send tickle (>=19s apart) to stay active.
  OBSERVED: rusq/slackdump v4 internal/presence/keeper.go:29-31 ("tested with connection_count=1, Slack set auto_away about 30 minutes after the last tickle"), keeper.go:52-53 (MinInterval 19s)
- Slack rate-limits socket sends with ack error `{code:-1, msg:"slow down, too many messages..."}` and socket error code 17; HTTP APIs return 429 + `Retry-After` (slackdump accepts both seconds and HTTP-date forms, defaults to 5s if missing, single retry).
  OBSERVED: beeper/slackgo websocket_managed_conn.go:480-493, 506-512; rusq/slackdump v4 internal/edge/edge.go:266-276, 308-322 (defRetryAfter 5s, maxRetryAfter 1h, maxAttempts 2)
- `presence_sub` only delivers when `batch_presence_aware=1` was set at connect (per public docs the flag "changes the shape of presence_change events"); handle BOTH `user` and `users` shapes forever - old servers/paths still emit single-user events.
  OBSERVED: https://docs.slack.dev/reference/methods/rtm.connect/; beeper/slackgo websocket_misc.go:61-68; dylex/slack-libpurple slack-user.c:173-186
- Enterprise Grid: the boot request goes to `<workspace>.enterprise.slack.com`, `slack_route=E...:E...` is echoed in `client.userBoot` response (`slack_route` field) and should be sent back as a query param on subsequent calls; `org_wide_aware=true` on client.counts and in start_args covers org-wide channels.
  OBSERVED: .omo/research/gist.md:78-94 (slack_route query param), gist.md:341 (Edge API path `/cache/<enterprise_id>/`), beeper/slackgo users.go:692-694 (api.versionData.SlackRoute = response.SlackRoute), beeper/slackgo users.go:760-762
- Reconnect loops must be capped: slackgo treats >20 fast reconnections as fatal, resets the counter after 1 minute of healthy session; mautrix uses 2s<<attempt capped at 5min with jitter, and respects Retry-After.
  OBSERVED: beeper/slackgo websocket_managed_conn.go:64-73, 109-111; mautrix/slack pkg/connector/client.go:246-254
- Fatal auth errors must not be retried: `invalid_auth`, `account_inactive`, `not_authed`, `token_revoked` (and 404 status) abort reconnection; `user_removed_from_team`, `team_disabled` similarly terminal in mautrix.
  OBSERVED: beeper/slackgo websocket_managed_conn.go:164-178; mautrix/slack pkg/connector/client.go:204-244
- Message subtypes with `hidden:true` (message_changed, message_deleted, pinned_item/unpinned_item) must update existing messages in place, never render.
  OBSERVED: beeper/slackgo messages.go:81; wee-slack slack_workspace.py:830-839
- `channel_marked`/`thread_marked` arrive for your own marks echoed from other devices - use `ts`/`subscription.last_read` to reconcile, and don't loop (don't re-mark on echo).
  OBSERVED: mautrix/slack pkg/connector/handleslack.go:330-340 (events consumed as read receipts)
- Duplicates: `desktop_notification`, `mobile_in_app_notification`, and file events overlap with `message`/`file_share` - mautrix deliberately ignores the notification ones to avoid double-rendering.
  OBSERVED: mautrix/slack pkg/connector/handleslack.go:156-160 ("ignored intentionally, these are duplicates or do not contain useful information")
- `client.userBoot` `channels` is partial (one live capture: 200 channels, 38 ims while sidebar counts were larger); always complement with `client.dms` + `client.counts` (+ `users.channelSections.list`, owned by the sidebar area).
  OBSERVED: .omo/research/findings.md:72-80, 94-103, 36-41
- Token rotation: `xoxc` travels in request bodies so it can be rotated per request; the `d=xoxd` cookie is HttpOnly and is what the websocket handshake needs.
  OBSERVED: .omo/research/gist.md:335 ("xoxc-* token ... sent in the request body ... can be rotated per-request"); dylex/slack-libpurple slack-rtm.c:193-199 (d cookie on wss handshake)

## LEADS

- `enterprise_id` as an explicit wss URL query param (per the task brief) was NOT observed in any mined source; wee-slack only feeds it into `gateway_server` indirectly for org-level teams (wee-slack slack_workspace.py:374-388). Verify against a live Grid org before relying on it.
- `client.init`: slackdump's current code posts its `client.userBoot`-shaped form to the endpoint name `client.init` while validating the response as client.userBoot (rusq/slackdump v4 internal/edge/client_boot.go:56,63). Either an accepted alias or a rename rolling out - confirm which name the live client uses.
- `update_thread_state` websocket event: seen in the wild in the Slack web client (2017 teardown, https://labs.detectify.com/writeups/hacking-slack-using-postmessage-and-websocket-reconnect-to-steal-your-precious-token). Verified 2026-11-19: emacs-slack decodes it (emacs-slack/emacs-slack slack-websocket.el:533-541: carries `has_unreads` + `mention_count`, updates thread counts) but it is absent from slackgo/wee-slack/slackdump/libpurple decoders; capture live payload for the full shape before implementing.
- `conversations.historyChanges` (exact casing from live capture, .omo/research/findings.md:59) - presumably the boot-time history-invalidation endpoint; shape unknown, capture it.
- `client.extras`, `client.appCommands`, `drafts.list`, `drafts.listActive` - boot-burst calls observed live (.omo/research/findings.md:54-58, .omo/research/gist.md:171) but owned by sibling areas / not yet shaped.
- `start_args` semantics per flag (`eac_cache_ts`, `ms_latest`, `connect_only`, `only_self_subteams`, `name_tagging`) are inferred from names + usage context only; all observed together at rusq/slackdump v4 internal/presence/keeper.go:214-224.
- `flannel` version values other than `3` and `flannel_api_ver` other than `4` unverified; both observed once each (keeper.go:239; beeper/slackgo users.go:636).
- `no_unreads`/`channel_badges`/`alerts`/`saved` sections of client.counts response: only named in slackgo's comment (beeper/slackgo/users.go:741-742), shape unknown.
- `sync_desync` flag semantics unknown (name observed in slackgo comment + slackdump); `lazy_channels=1` observed in a live capture but not in slackdump's FlannelURL (keeper.go:240 has it - keeper and live capture agree; slackgo has it commented).
- Exact `hello` payload beyond `fast_reconnect` (e.g. `ms_hello`, server ts) - capture live; wee-slack reads only `fast_reconnect`.
- `mpim_open`/`mpim_joined` events: wee-slack handles `mpim_open` (slack_workspace.py:800) but no mined Go/EL client maps it; treat as open/close-family event.
- User `presence_query` frame (bulk presence fetch instead of subscribe): the task brief mentions it; only its name surfaced in unrelated repos during search - no credible implementation found; verify in web client JS.
- The `edgeapi.slack.com/cache/<team>/...` Flannel conditional-fetch requests (`updated_ids` with per-entity timestamps) that the real web client uses for boot are documented in .omo/research/gist.md:92-108, 340 but are owned by the edge-cache API area, not this one.
