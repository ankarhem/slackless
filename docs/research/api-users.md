# Users, profiles, presence, status, DND and prefs - Slack web-client API

## Summary

- Covers user objects and lookup (`users.info`, `users.list`, edge `users/*`), profile + custom status (`users.profile.get/set`, `team.profile.get`), presence (REST + websocket), DND/snooze (`dnd.*`), and user preferences (`users.prefs.get/set`), plus usergroups, team info, bots, and avatar URL formats.
- Split: `users.info/list`, `users.profile.*`, `users.getPresence/setPresence`, `dnd.*`, `usergroups.*`, `team.info`, `bots.info`, `team.profile.get` are **public documented methods** (work fine with an `xoxc-` session token sent as `token=` in the form body, plus `Cookie: d=...`). `presence.set`, `users.prefs.get/set` and the whole `edgeapi.slack.com/cache/...` surface are **private web-client only**.
- The web client actually hydrates users through the Flannel edge cache (`edgeapi.slack.com/cache/<team_id>/users/info|list|search`), with conditional `updated_ids` fetching; the workspace `/api/` methods are the fallback and the only way to _write_ (profile, prefs, presence).
- Live presence flows over the RTM websocket (`wss://wss-primary.slack.com/`): subscribe with `presence_sub`, keep-alive with `tickle`; REST presence calls are the slow path.
- Biggest risks: edge request/response shapes are undocumented and can change silently (guard on `ok` + `failed_ids`/`pending_ids`); pref names are a flat stringly-typed map that Slack adds to freely; `users.prefs.set` semantics (single `name`/`value` pairs) are easy to get wrong; presence keep-alive timing constants are empirical, not contractual.

## Endpoints

Conventions used below: `<ws>` = workspace subdomain (`acme.slack.com`; Enterprise Grid workspaces live on `<sub>.enterprise.slack.com` - both work). Auth for all `/api/` calls in this client: `Cookie: d=<xoxd-...>` header and `token=<xoxc-...>` in the urlencoded form body. wee-slack additionally sends `Authorization: Bearer <token>` + cookies and it works too - OBSERVED: wee-slack/wee-slack/slack/slack_api.py:69.

### users.info

- Transport: `POST https://<ws>.slack.com/api/users.info` (GET also accepted), `application/x-www-form-urlencoded`.
- Visibility: public (rate limit tier 4). OBSERVED: https://docs.slack.dev/reference/methods/users.info.md
- Request params:
  - `token` - string - required - `xoxc-` session token.
  - `user` - string (user id, `U...`/`W...`) - optional - user to fetch.
  - `users` - string, comma-separated user ids - **undocumented web-client extension**, batch form. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:303-315 (`params: Params = {"users": ",".join(user_ids)}`).
  - `include_locale` - boolean - optional - default false; returns `locale` field. OBSERVED: https://docs.slack.dev/reference/methods/users.info.md
- Response:

```json
{
  "ok": true,
  "user": {
    "id": "W012A3CDE",
    "team_id": "T012AB3C4",
    "name": "spengler",
    "deleted": false,
    "color": "9f69e7",
    "real_name": "Egon Spengler",
    "tz": "America/Los_Angeles",
    "tz_label": "Pacific Daylight Time",
    "tz_offset": -25200,
    "profile": {
      "avatar_hash": "ge3b51ca72de",
      "status_text": "Print is dead",
      "status_emoji": ":books:",
      "display_name": "spengler",
      "image_24": "https://.../avatar/e3b51ca72dee4ef87916ae2b9240df50.jpg",
      "image_48": "https://.../avatar/e3b51ca72dee4ef87916ae2b9240df50.jpg",
      "team": "T012AB3C4"
    },
    "is_admin": true,
    "is_bot": false,
    "is_app_user": false,
    "updated": 1502138686,
    "who_can_share_contact_card": "VISIBLE_TO_TEAM?EVERYONE?",
    "is_stranger": false,
    "enterprise_user": {
      "id": "W012A3CDE",
      "enterprise_id": "E1234A12AB",
      "enterprise_name": "Umbrella Corp",
      "is_admin": false,
      "teams": ["T012AB3C4"]
    }
  }
}
```

OBSERVED: https://docs.slack.dev/reference/methods/users.info.md and wee-slack/wee-slack/typings/slack_api/slack_users_info.pyi (`SlackUserInfoCommon`/`SlackUserInfoPerson`/`SlackUserInfoBot`, `SlackEnterpriseUser`).

- Errors: `user_not_found`, `user_not_visible`, plus generic auth errors. OBSERVED: https://docs.slack.dev/reference/methods/users.info.md
- Used for: resolving one/batch user objects, refreshing a user after `user_invalidated` (wee-slack refetches on that event - OBSERVED: wee-slack/wee-slack/slack/slack_workspace.py:708-717).
- Pagination: none.

### users.list

- Transport: `POST https://<ws>.slack.com/api/users.list`, urlencoded.
- Visibility: public (tier 2... effectively ~20/min; slackdump treats it as its Tier2 budget). OBSERVED: rusq/slackdump/users.go:38-47 (`l := s.limiter(network.Tier2)` around `client.GetUsersContext(ctx)`).
- Request params:
  - `token` - string - required.
  - `limit` - int - optional - page size, max 1000 (wee-slack caps at 1000). OBSERVED: wee-slack/wee-slack/slack/slack_api.py:152-156 (`cur_limit = 1000 if limit is None or limit > 1000 else limit`).
  - `cursor` - string - optional - `next_cursor` from previous page. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:155-168 (recursive `_fetch_list` walking `response_metadata.next_cursor`).
  - `include_locale` - boolean - optional. OBSERVED: slack-go/slack@users.go:441 (`GetUsersContext` sets `team_id`, `include_locale`).
  - `team_id` - string - optional - required with org-level tokens. OBSERVED: slack-go/slack@users.go:414-415.
- Response:

```json
{"ok": true,
 "members": [ { ...user object, same shape as users.info... } ],
 "cache_ts": 1700000000,
 "response_metadata": {"next_cursor": "dGVhbTpV..." }}
```

OBSERVED: slack-go/slack@users.go:224-243 (`type User struct` with `Members []User`, `ResponseMetadata`) and wee-slack/wee-slack/slack/slack_api.py:152-171.

- Pagination: cursor/limit; `next_cursor` empty string terminates.
- Used for: bulk user directory cache (slackdump `GetUsers`, 5-min local cache - OBSERVED: rusq/slackdump/users.go:19-58).
- Gotcha: bots/apps/profile-only users included by default; `email` requires `users:read.email` scope on OAuth apps (not enforced for session tokens). OBSERVED: https://docs.slack.dev/reference/methods/users.info.md

### users.profile.get

- Transport: `POST https://<ws>.slack.com/api/users.profile.get`, urlencoded.
- Visibility: public. OBSERVED: slack-go/slack@users.go:813-824 (`postMethod(ctx, "users.profile.get", values, resp)`).
- Request params:
  - `token` - string - required.
  - `user` - string - optional - defaults to caller.
  - `include_labels` - boolean - optional - include custom-field labels. OBSERVED: slack-go/slack@users.go:813-824 (`GetUserProfileParameters`).
- Response:

```json
{
  "ok": true,
  "profile": {
    "title": "Paranormal Investigator",
    "phone": "",
    "skype": "",
    "real_name": "Egon Spengler",
    "real_name_normalized": "Egon Spengler",
    "display_name": "spengler",
    "display_name_normalized": "spengler",
    "fields": { "Xf0123AB": { "value": "8", "alt": "" } },
    "status_text": "Print is dead",
    "status_emoji": ":books:",
    "status_emoji_display_info": [
      { "emoji_name": "books", "display_url": "...", "unicode": "1f4da" }
    ],
    "status_expiration": 0,
    "avatar_hash": "ge3b51ca72de",
    "image_24": "https://avatars.slack-edge.com/...",
    "image_512": "https://...",
    "email": "spengler@ghostbusters.example.com",
    "first_name": "Egon",
    "last_name": "Spengler",
    "team": "T012AB3C4"
  }
}
```

OBSERVED: field set mirrors wee-slack/wee-slack/typings/slack_api/slack_profile.pyi (`SlackProfileCommon`/`SlackProfilePerson`/`SlackProfileBot`, `SlackProfileField {value, alt}`) and slack-go/slack@users.go:20-131 (`UserProfile`, `UserProfileCustomField`, `UserProfileStatusEmojiDisplayInfo`).

- Used for: profile popup, custom profile fields (see `team.profile.get` for field _definitions_).
- Pagination: none.

### users.profile.set

- Transport: `POST https://<ws>.slack.com/api/users.profile.set`. Both encodings work: urlencoded with `profile=<json>` (slack-go) or JSON body `{"profile": {...}}` (wee-slack). OBSERVED: slack-go/slack@users.go:582-607 vs wee-slack/wee-slack/slack/slack_api.py:577-583 (`body = {"profile": profile}` + `Content-Type: application/json`).
- Visibility: public.
- Request params:
  - `token` - string - required.
  - `profile` - JSON string/object - required - sparse; only supplied keys change. Keys observed: `display_name`, `email`, `first_name`, `last_name`, `phone`, `pronouns`, `real_name`, `start_date`, `title`, `status_text`, `status_emoji`, `status_expiration`. OBSERVED: wee-slack/wee-slack/typings/slack_api/slack_profile.pyi (`SlackSetProfile`).
  - `user` - string - optional - only admins may set for others; "should not be set if empty". OBSERVED: slack-go/slack@users.go:598-605.
- Setting status: `status_text` + `status_emoji` (`:name:`) + `status_expiration` (unix seconds, 0 = never). Clear status by sending `{"status_emoji": "", "status_text": ""}`. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:585-589 (`set_user_status`, `clear_user_status`).
- Response: `{"ok": true, "profile": { ...updated profile... }}`, also (web client) emits `user_status_changed` on the websocket. OBSERVED: wee-slack/wee-slack/typings/slack_api/slack_profile.pyi (`SlackUsersProfileSetSuccessResponse`).
- Used for: "Set status" dialog, profile editor.
- Pagination: none.

### users.setPhoto (and users.deletePhoto)

- Transport: `POST https://<ws>.slack.com/api/users.setPhoto`, `multipart/form-data` with the image file.
- Visibility: public per slack-go doc-comment (docs page not fetched - see LEADS). OBSERVED: slack-go/slack@users.go:532-553.
- Request params: `token`; image file; `crop_x`, `crop_y`, `crop_w` ints (crop square). OBSERVED: slack-go/slack@users.go:222-229 (`UserSetPhotoParams`).
- `users.deletePhoto` - POST, token only. OBSERVED: slack-go/slack@users.go:561-573.
- Response: `{"ok": true}` minimum - slack-go parses only a bare `SlackResponse` (`ok`), not a profile/avatarHash shape. OBSERVED: slack-go/slack@users.go:532-551 (`SetUserPhotoContext` parses `&SlackResponse{}`). A richer `profile`/avatar-hash success shape is unverified: not reproducible from any cited source (moved to LEADS).
- Used for: "Change avatar" flow.
- Pagination: none.

### users.setPresence

- Transport: `POST https://<ws>.slack.com/api/users.setPresence`, urlencoded.
- Visibility: public.
- Request params: `token` - required; `presence` - `"active"` or `"away"` - required. OBSERVED: slack-go/slack@users.go:488-495.
- Response: `{"ok": true}`. Sets _manual_ presence; auto-away still applies afterwards.
- Used for: the away/active toggle. Note: the actual web client uses private `presence.set` (below) instead; prefer that for parity.
- Pagination: none.

### users.getPresence

- Transport: `POST https://<ws>.slack.com/api/users.getPresence`, urlencoded.
- Visibility: public.
- Request params: `token` - required; `user` - string - required. OBSERVED: slack-go/slack@users.go:252-263.
- Response:

```json
{
  "ok": true,
  "presence": "active",
  "online": true,
  "auto_away": false,
  "manual_away": false,
  "connection_count": 1,
  "last_activity": 1700000000
}
```

OBSERVED: slack-go/slack@users.go:158-165 (`UserPresence` struct: `presence`, `online`, `auto_away`, `manual_away`, `connection_count`, `last_activity`).

- Used for: verifying presence after tickles (slackdump `lookalive` does exactly this). OBSERVED: rusq/slackdump/internal/presence/keeper.go:488-492.
- Pagination: none.

### presence.set (private, web client)

- Transport: `POST https://<ws>.slack.com/api/presence.set`, urlencoded.
- Visibility: private (undocumented web-client method; the user-session equivalent of users.setPresence).
- Request params: `token` - required; `presence` - `"active"|"away"` - required. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:561-567 (`method = "presence.set"`, `params = {"presence": presence}`).
- Response: `{"ok": true}`.
- Used for: manual presence toggle in the web client UI.
- Pagination: none.

### users.prefs.get

- Transport: `POST https://<ws>.slack.com/api/users.prefs.get`, urlencoded (slack-go has also used GET). OBSERVED: slack-go/slack@info.go:336-339 (`getMethod(ctx, "users.prefs.get", ...)`).
- Visibility: private (undocumented; requires a "client"-type token - `xoxc-` qualifies). OBSERVED: ErikKalkoken/slackApiDoc@users.prefs.get.md ("needs client token").
- Request params:
  - `token` - required.
  - `prefs` - string - optional - comma-separated pref names to fetch a subset. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:196-199 (`params = {"prefs": prefs} if prefs else {}`).
- Response: `{"ok": true, "prefs": { ...flat map of ~600 keys... }}`. The web client boot (`client.userBoot`) carries the same map under `prefs`. OBSERVED: .omo/research/findings.md (client.userBoot returns `prefs: dict[676]`) and slack-go/slack@info.go:20-302 (`type UserPrefs struct`, ~200 typed keys).
- Key pref names observed (stringly-typed values):
  - Theme/appearance: `sidebar_theme` (e.g. `"ocean_theme"`), `sidebar_theme_custom_values` (JSON object with `column_bg`, `menu_bg`, `active_item`, `active_item_text`, `hover_item`, `text_color`, `active_presence`, `badge`), `messages_theme` (e.g. `"light_with_avatars"`), `user_colors`, `color_names_in_list`, `a11y_font_size`. OBSERVED: ErikKalkoken/slackApiDoc@users.prefs.get.md (example JSON).
  - Sidebar: `sidebar_behavior`, `channel_sort`, `custom_channel_sort`, `separate_private_channels`, `separate_shared_channels`, `ent_org_wide_channels_sidebar`, `channel_sections` (sidebar sections JSON). OBSERVED: slack-go/slack@info.go (`SidebarBehavior`, `ChannelSort`, `ChannelSections` fields).
  - Notifications: `all_notifications_prefs` (**JSON-encoded string** holding `{channels: {<cid>: {desktop, mobile, muted, suppress_at_channel, follow_all_threads}}, global: {desktop_sound, global_desktop, global_keywords, global_mobile, global_mpdm_desktop, global_mpdm_mobile, mobile_sound, no_text_in_notifications, push_idle_wait, push_show_preview, threads_everything}}`), `push_everything`, `push_dm_alert`, `push_mention_alert`, `push_sound`, `push_idle_wait`, `loud_channels`, `loud_channels_set`, `never_channels`, `at_channel_suppressed_channels`, `email_alerts`, `growls_enabled`. OBSERVED: wee-slack/wee-slack/typings/slack_api/slack_users_prefs.pyi (`AllNotificationsPrefs`) and ErikKalkoken/slackApiDoc@users.prefs.get.md.
  - Mute lists: `muted_channels` (comma-separated channel ids), `search_exclude_channels`, `at_channel_suppressed_channels`. OBSERVED: wee-slack/wee-slack/typings/slack_api/slack_users_prefs.pyi (`muted_channels`) + ErikKalkoken/slackApiDoc@users.prefs.get.md.
  - Emoji/frecency: `emoji_mode` (`"default"`), `emoji_use` (JSON object mapping emoji name -> use count, used by the emoji picker's frecency sort), `frecency_jumper` (JSON frecency entries powering Cmd+K/jump; frequently `""` in older captures), `jumbomoji`, `emoji_autocomplete_big`, `preferred_skin_tone`. OBSERVED: ErikKalkoken/slackApiDoc@users.prefs.get.md (`emoji_use: {"simple_smile":161,...}`, `frecency_jumper: ""`), slack-go/slack@info.go (`EmojiUse`, `FrecencyJumper`).
  - DND schedule: `dnd_enabled`, `dnd_start_hour` (`"23:00"`), `dnd_end_hour`, plus per-weekday `dnd_enabled_monday`, `dnd_before_monday`, `dnd_after_monday`, ... OBSERVED: ErikKalkoken/slackApiDoc@users.prefs.get.md and slack-go/slack@info.go (Dnd* fields).
  - Misc: `display_display_names`, `display_real_names_override`, `time24`, `tz`, `convert_emoticons`, `graphic_emoticons`, `show_typing`, `mute_sounds`, `new_msg_snd`, `mark_msgs_read_immediately`, `start_scroll_at_oldest`, `search_sort`, `f_key_search`, `k_key_omnibox`. OBSERVED: slack-go/slack@info.go:20-302.
- Used for: applying all workspace-local settings in the client UI; diffing after `pref_change` events.
- Pagination: none (single map; filter with `prefs=`).

### users.prefs.set

- Transport: `POST https://<ws>.slack.com/api/users.prefs.set`, urlencoded.
- Visibility: private (undocumented).
- Request params:
  - `token` - required.
  - `name` - string - required - pref key to set (e.g. `muted_channels`). OBSERVED: wee-slack/wee-slack/slack/slack_api.py:569-575 (`params = {"name": "muted_channels", "value": ",".join(channel_ids)}`).
  - `value` - string - required - new value (JSON-stringified for composite prefs like `all_notifications_prefs`).
  - `reason` - string - optional - e.g. `"update-muted-channels"`. OBSERVED: slack-go/slack@info.go:361-366 (`values := url.Values{"token": ..., "muted_channels": {newChnls}, "reason": {"update-muted-channels"}}` posted to `users.prefs.set`).
  - Legacy variant observed in slack-go: pass the pref key directly as a param (`muted_channels=<csv>`, `reason=...`) instead of `name`/`value`. OBSERVED: slack-go/slack@info.go:366-368 and :390-393.
- Response: `{"ok": true, "prefs": {<changed keys>}}`; fires a `pref_change` websocket event per key. OBSERVED: wee-slack/wee-slack/slack/slack_workspace.py:695-700 (handles `pref_change` with `data["name"]`, `data["value"]`).
- Used for: settings UI (mute channel, theme picker, notification config).
- Pagination: none.

### dnd.info

- Transport: `POST https://<ws>.slack.com/api/dnd.info`, urlencoded.
- Visibility: public. OBSERVED: https://docs.slack.dev/reference/methods/dnd.info.md and slack-go/slack@dnd.go:105-117.
- Request params: `token` - required; `user` - string - optional (defaults to caller). OBSERVED: slack-go/slack@dnd.go:110-117.
- Response:

```json
{
  "ok": true,
  "dnd_enabled": true,
  "next_dnd_start_ts": 1709250000,
  "next_dnd_end_ts": 1709293200,
  "snooze_enabled": true,
  "snooze_endtime": 1700000000,
  "snooze_remaining": 3600,
  "snooze_debug": { "snooze_end_date": "2024-03-01T12:00:00Z" }
}
```

OBSERVED: slack-go/slack@dnd.go:22-34 (`SnoozeInfo`, `DNDStatus` structs).

- Used for: showing the crescent moon on a user, snooze banner.
- Pagination: none.

### dnd.teamInfo

- Transport: `POST https://<ws>.slack.com/api/dnd.teamInfo`, urlencoded.
- Visibility: public. OBSERVED: https://docs.slack.dev/reference/methods/dnd.teaminfo.md
- Request params: `token`; `users` - comma-separated user ids - **max 50**; `team_id` - optional (required after workspace migration when the API returns `missing_argument: team_id`). OBSERVED: https://docs.slack.dev/reference/methods/dnd.teaminfo.md ("up to 50 users"), slack-go/slack@dnd.go:10-12 (`DNDOptionTeamID`), slack-go/slack@dnd.go:137-142 (`strings.Join(users, ",")`), emacs-slack: yuya373/emacs-slack/slack-dnd-status.el:33,77 (`dnd.teamInfo` with `users` joined by comma, built from open IMs).
- Response: `{"ok": true, "users": {"U123": {dnd status object per dnd.info}}}`. OBSERVED: slack-go/slack@dnd.go:36-40 (`dndTeamInfoResponse{Users map[string]DNDStatus}`).
- Used for: bulk DND badges on the DM list.
- Pagination: none (batch by 50).

### dnd.setSnooze

- Transport: `POST https://<ws>.slack.com/api/dnd.setSnooze`, urlencoded.
- Visibility: public. OBSERVED: https://docs.slack.dev/reference/methods/dnd.setsnooze.md
- Request params: `token`; `num_minutes` - int - required - snooze duration (starts a snooze if none active). OBSERVED: slack-go/slack@dnd.go:162-172.
- Response: same `DNDStatus` shape (`snooze_enabled`, `snooze_endtime`, `snooze_remaining`, `snooze_debug`). OBSERVED: slack-go/slack@dnd.go:22-28.
- Used for: "Snooze notifications for 1h".
- Pagination: none.

### dnd.endSnooze

- Transport: `POST https://<ws>.slack.com/api/dnd.endSnooze`, urlencoded, token only.
- Visibility: public. OBSERVED: https://docs.slack.dev/reference/methods/dnd.endsnooze.md and slack-go/slack@dnd.go:85-91.
- Response: `DNDStatus` (snooze disabled, schedule retained). OBSERVED: slack-go/slack@dnd.go:85-96.
- Used for: "End snooze" button.
- Pagination: none.

### dnd.endDnd

- Transport: `POST https://<ws>.slack.com/api/dnd.endDnd`, urlencoded, token only.
- Visibility: public. OBSERVED: https://docs.slack.dev/reference/methods/dnd.enddnd.md and slack-go/slack@dnd.go:63-71.
- Response: `{"ok": true}` only. OBSERVED: slack-go/slack@dnd.go:66-73 (parses plain `SlackResponse`).
- Used for: turning the scheduled/manual DND off.
- Pagination: none.

### team.profile.get

- Transport: `POST https://<ws>.slack.com/api/team.profile.get`, urlencoded.
- Visibility: public (documented; primary use on Enterprise Grid). OBSERVED: slack-go/slack@team.go:184-193 and slack-ruby/slack-api-ref@docs.slack.dev/methods/team.profile.get.json.
- Request params: `token`; `visibility` - `"all"|"visible"|"hidden"` - optional; `team_id` - optional. CORRECTED during verification: the `visibility` enum is `all`/`visible`/`hidden`; `"visible_to_user"` appears in no cited source. OBSERVED: slack-ruby/slack-api-ref@docs.slack.dev/methods/team.profile.get.json (`visibility` enum all/visible/hidden), slack-go/slack@team.go:185-198 (`GetTeamProfileContext`, token + optional team_id).
- Response: `{"ok": true, "profile": {"fields": [{"id": "Xf0123AB", "ordering": 1, "label": "Level", "name": "level", "type": "text|options", "options": [...], "is_hidden": false}]}}` - the _definitions_ of the custom profile fields; per-user values live in `users.profile.get` → `profile.fields` keyed by field id. OBSERVED: slack-go/slack@team.go (`TeamProfileField` handling) for the method; field shape corroborated by wee-slack/wee-slack/typings/slack_api/slack_profile.pyi (`SlackProfileField {value, alt}` keyed by `Xf...` id).
- Used for: rendering custom profile fields in the profile pane.
- Pagination: none.

### usergroups.list

- Transport: `POST https://<ws>.slack.com/api/usergroups.list`, urlencoded.
- Visibility: public (tier 2). OBSERVED: https://docs.slack.dev/reference/methods/usergroups.list.md
- Request params: `token`; `include_count` - bool; `include_disabled` - bool; `include_users` - bool (embeds member list, expensive); `team_id` - required with org tokens, ignored otherwise. OBSERVED: https://docs.slack.dev/reference/methods/usergroups.list.md and slack-go/slack@usergroups.go:297-343.
- Response:

```json
{
  "ok": true,
  "usergroups": [
    {
      "id": "S0614TZR7",
      "team_id": "T060RNRCH",
      "is_usergroup": true,
      "is_subteam": true,
      "name": "Team Admins",
      "description": "...",
      "handle": "admins",
      "is_external": false,
      "date_create": 1446598059,
      "date_update": 1446670362,
      "date_delete": 0,
      "auto_type": "admin",
      "auto_provision": false,
      "enterprise_subteam_id": "",
      "created_by": "USLACKBOT",
      "updated_by": "U060RNRCZ",
      "prefs": { "channels": [], "groups": [] },
      "user_count": 2,
      "channel_count": 0,
      "users": ["U060RNRCZ"]
    }
  ]
}
```

OBSERVED: https://docs.slack.dev/reference/methods/usergroups.list.md (response example) and wee-slack/wee-slack/typings/slack_api/slack_usergroups_info.pyi (`SlackUsergroupInfoCommon` incl. `enterprise_subteam_id`, `channel_count`).

- Used for: `@group` completion, usergroup picker.
- Pagination: none.
- Live updates: `subteam_created`/`subteam_updated`/`subteam_members_changed`/`subteam_self_added`/`subteam_self_removed` websocket events. OBSERVED: wee-slack/wee-slack/slack/slack_workspace.py:720-739.

### usergroups.users.list

- Transport: `POST https://<ws>.slack.com/api/usergroups.users.list`, urlencoded.
- Visibility: public. OBSERVED: slack-go/slack@usergroups.go:506-532.
- Request params: `token`; `usergroup` - string (S-id) - required; `include_disabled` - bool - optional. OBSERVED: slack-go/slack@usergroups.go:506-532.
- Response: `{"ok": true, "users": ["U060RNRCZ", ...]}`. OBSERVED: slack-go/slack@usergroups.go:506-532 (`userGroupRequest` returns `Users []string`).
- Used for: expanding `@group` mentions to members.
- Pagination: none.

### team.info

- Transport: `POST https://<ws>.slack.com/api/team.info`, urlencoded (JSON also accepted per docs content-types).
- Visibility: public (tier 3). OBSERVED: https://docs.slack.dev/reference/methods/team.info.md
- Request params: `token`; `team` - optional; `domain` - optional (same-enterprise lookup); `include_slackbot_availability` - bool - optional. OBSERVED: https://docs.slack.dev/reference/methods/team.info.md
- Response:

```json
{
  "ok": true,
  "team": {
    "id": "T012AB3C4",
    "name": "My Team",
    "url": "https://my.slack.com/",
    "domain": "example",
    "email_domain": "example.com",
    "icon": {
      "image_34": "https://...",
      "image_44": "https://...",
      "image_68": "https://...",
      "image_88": "https://...",
      "image_102": "https://...",
      "image_132": "https://...",
      "image_230": "https://...",
      "image_default": true
    },
    "avatar_base_url": "https://avatars.slack-edge.com/",
    "is_verified": true,
    "enterprise_id": "E1234A12AB",
    "enterprise_name": "Umbrella Corp"
  }
}
```

OBSERVED: https://docs.slack.dev/reference/methods/team.info.md (base fields) and wee-slack/wee-slack/typings/slack_api/slack_team_info.pyi (`SlackTeamInfo` incl. `avatar_base_url`, `is_verified`, `public_url`, `discoverable`, `enterprise_*`).

- Used for: workspace switcher, team icon, `avatar_base_url` for relative image paths.
- Pagination: none. (Multi-workspace listing via `team.list`/`auth.teams.list` is a sibling area's concern.)

### bots.info

- Transport: `POST https://<ws>.slack.com/api/bots.info`, urlencoded.
- Visibility: public. OBSERVED: https://docs.slack.dev/reference/methods/bots.info.md and slack-go/slack@bots.go:64.
- Request params: `token`; `bot` - string (B-id) - optional; `team_id` - string - optional. Web client batch variant observed: wee-slack calls with a list of bot ids (`bots`); response has `bots` array. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:364+ (`fetch_bots_info`) and wee-slack/wee-slack/typings/slack_api/slack_bots_info.pyi (`SlackBotsInfoSuccessResponse {ok, bots: [SlackBotInfo]}`).
- Response:

```json
{
  "ok": true,
  "bot": {
    "id": "B061F7JD2",
    "deleted": false,
    "name": "pollbot",
    "updated": 1483891491,
    "app_id": "A0EEKTQ7H",
    "user_id": "U0FcAdcD1",
    "icons": {
      "image_36": "https://avatars.slack-edge.com/..._36.png",
      "image_48": "https://...",
      "image_72": "https://..."
    }
  }
}
```

OBSERVED: wee-slack/wee-slack/typings/slack_api/slack_bots_info.pyi (`SlackBotInfo`) and slack-go/slack@bots.go:10-17 (`Bot` struct).

- Used for: resolving bot senders to app identity + icons (user objects of bots only carry `bot_id`/`app_id`).
- Pagination: none.

### edgeapi: users/info (Flannel batch user fetch)

- Transport: `POST https://edgeapi.slack.com/cache/<team_id>/users/info`, `Content-Type: application/json`, JSON body (NOT form). Cookie `d=...` carries auth; `token` is a field in the JSON body. OBSERVED: rusq/slackdump/internal/edge/edge.go:91,133 (`edgeAPI: fmt.Sprintf("https://edgeapi.slack.com/cache/%s/", teamID)`) and edge.go:166-168 (`BaseRequest{Token string \`json:"token"\`}`); wee-slack/wee-slack/slack/slack_api.py:81-84.
- Visibility: private (Flannel edge cache; `x-backend: flannel-prod-pdx-*` backed by Loom). OBSERVED: .omo/research/gist.md (Edge API section, users/info listed with loom index headers).
- Request body:

```json
{
  "token": "xoxc-...",
  "check_interaction": true,
  "include_profile_only_users": true,
  "updated_ids": { "U012A3CDE": 1502138686, "U456": 0 }
}
```

OBSERVED: rusq/slackdump/internal/edge/userlist.go:92-97 (`UsersInfoRequest`). `updated_ids` maps user id -> last-seen `updated` ts; `0` forces full fetch (conditional fetching: server returns only entities changed since the ts). OBSERVED: rusq/slackdump/internal/edge/userlist.go:159-165 (sets `updatedIds[id] = 0`) and .omo/research/gist.md (edge uses conditional `updated_ids`).

- Response:

```json
{"ok": true,
 "results": [{"id": "U012A3CDE", "team_id": "T012AB3C4", "name": "spengler",
   "color": "9f69e7", "is_bot": false, "is_app_user": false, "deleted": false,
   "is_stranger": false, "updated": 1502138686,
   "profile": { ...same Profile shape as public API... },
   "who_can_share_contact_card": "VISIBLE_TO_TEAM?EVERYONE?"}],
 "failed_ids": [], "pending_ids": ["U789"], "can_interact": {"U789": false}}
```

OBSERVED: rusq/slackdump/internal/edge/userlist.go:99-118 (`UserInfoResponse`, `UserInfo`, `Profile`).

- Quirk: responses can arrive in _rounds_. If `pending_ids` is non-empty, re-call with refreshed `updated_ids` until it empties. OBSERVED: rusq/slackdump/internal/edge/userlist.go:139-158 (`Client.GetUsers` loop over `PendingIDS`).
- Used for: hydrating user objects after boot / when a channel's members arrive.
- Pagination: none (batch by request size; the client loops internally).

### edgeapi: users/list (Flannel directory listing)

- Transport: `POST https://edgeapi.slack.com/cache/<team_id>/users/list`, JSON body + `token` field.
- Visibility: private. OBSERVED: rusq/slackdump/internal/edge/edge.go:91 and userlist.go:237-241 (`cl.callEdgeAPI(ctx, &ur, "users/list", &req)`).
- Request body:

```json
{
  "token": "xoxc-...",
  "channels": ["C012A3CDE"],
  "present_first": false,
  "filter": "everyone",
  "index": "users_by_display_name",
  "locale": "en-US",
  "include_profile_only_users": false,
  "marker": "",
  "count": 50
}
```

OBSERVED: rusq/slackdump/internal/edge/userlist.go:26-42 (`UsersListRequest`) and userlist.go:229-251 (`publicUserList` constants: `filter: "everyone"`, `index: "users_by_display_name"`, `count: 50`).

- Response: `{"ok": true, "results": [ {user objects, full `User` shape incl. tz/is_admin/updated} ], "next_marker": "..."}`. OBSERVED: rusq/slackdump/internal/edge/userlist.go:44-89 (`User`, `Profile` structs) and userlist.go:26-42 (`NextMarker`).
- Pagination: opaque `marker` (echo `next_marker` back as `marker`); done when `next_marker == ""` and no results. OBSERVED: rusq/slackdump/internal/edge/userlist.go:253-268 (marker loop).
- Used for: listing members of channels without `conversations.members` calls.
- Gotcha: users for DM channels (`D...`) are NOT returned via `channels`; the client falls back to per-DM `conversations.view` (`users` in the channel object). OBSERVED: rusq/slackdump/internal/edge/userlist.go:196-227 (`splitDMs`, `directUserList`).

### edgeapi: users/search (Flannel people search)

- Transport: `POST https://edgeapi.slack.com/cache/<team_id>/users/search`, JSON body + `token`.
- Visibility: private. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:81,103-111.
- Request body (the exact params the web client sends):

```json
{
  "token": "xoxc-...",
  "include_profile_only_users": true,
  "query": "ego",
  "count": 25,
  "fuzz": 1,
  "uax29_tokenizer": false,
  "filter": "NOT deactivated"
}
```

OBSERVED: wee-slack/wee-slack/slack/slack_api.py:102-111 (`fetch_users_search`).

- Response:

```json
{"ok": true, "results": [ ...SlackUserInfo objects... ],
 "presence_active_ids": ["U012A3CDE"]}
```

OBSERVED: wee-slack/wee-slack/typings/slack_edgeapi/slack_users_search.pyi (`SlackUsersSearchSuccessResponse` incl. `presence_active_ids` - instant green dots for the results).

- Used for: Cmd+K / people picker autocomplete.
- Pagination: none observed (single page of `count`).
- Cross-area sibling: `channels/search` on the same host with `filter: "xws"` is documented in the channels area doc; wee-slack shows the pattern. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:113-122.

### edgeapi: usergroups/info (Flannel batch usergroup fetch)

- Transport: `POST https://edgeapi.slack.com/cache/<team_id>/usergroups/info`, JSON body + `token`.
- Visibility: private. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:81,93-96.
- Request body: `{"token": "xoxc-...", "ids": ["S0614TZR7", ...]}`. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:92-96.
- Response: `{"ok": true, "results": [ ...SlackUsergroupInfo... ], "failed_ids": ["S..."]}`. OBSERVED: wee-slack/wee-slack/typings/slack_edgeapi/slack_usergroups_info.pyi (`SlackEdgeUsergroupsInfoSuccessResponse` with `failed_ids`).
- Used for: resolving `@group` handles already present in rendered messages.
- Pagination: none.

### edgeapi: users/counts

- Transport: `POST https://edgeapi.slack.com/cache/<team_id>/users/counts`.
- Visibility: private. OBSERVED: .omo/research/gist.md (Edge API resource list: `/cache/E03C83N442J/users/counts  # user count stats`).
- Request/response body shapes not captured in our teardown - see LEADS.
- Used for: user count stats in the web client (org directory counts).
- Pagination: none.

### users.priority.list

- Transport: `POST https://<ws>.slack.com/api/users.priority.list`, urlencoded, web-client only.
- Visibility: private. OBSERVED: .omo/research/gist.md (boot phase 2: "users.priority.list -- ranked contacts for @-mention autocomplete").
- Request params: not captured in teardown - see LEADS.
- Response: ranked user ids used to order people autocomplete (the counterpart of `channels_priority` in `client.userBoot`). OBSERVED: .omo/research/gist.md.
- Used for: @-mention ordering; the frecency data behind it lives in the `frecency_jumper`/`frecency` prefs (users.prefs.get).
- Pagination: none.

## Websocket events

Websocket endpoint (flannel gateway): `wss://wss-primary.slack.com/?token=<xoxc>&gateway_server=<team_id>-1&slack_client=desktop&batch_presence_aware=1&no_query_on_subscribe=1&flannel=3&lazy_channels=1&sync_desync=1&start_args=%3Fagent%3Dclient%26org_wide_aware%3Dtrue%26eac_cache_ts%3Dtrue%26cache_ts%3D0%26name_tagging%3Dtrue%26only_self_subteams%3Dtrue%26connect_only%3Dtrue%26ms_latest%3Dtrue`. OBSERVED: rusq/slackdump/internal/presence/keeper.go:227-249 (`FlannelURL`), wee-slack/wee-slack/slack/slack_workspace.py:388, emacs-slack: yuya373/emacs-slack/slack-websocket.el:78-83 (same URL with `gateway_server=<team>-4`).

Keep-alive frame pair: client sends `{"type":"ping","id":N}` every 10s, server replies `{"type":"pong","reply_to":N}`. OBSERVED: rusq/slackdump/internal/presence/keeper.go:24-26, 400-411.

### presence_sub (client -> server)

- Payload: `{"type": "presence_sub", "ids": ["U123", "U456"]}`. Sent right after `hello` (and whenever the visible user set changes). OBSERVED: rusq/slackdump/internal/presence/keeper.go:398-401 (sends `presence_sub` with own id on hello), emacs-slack: yuya373/emacs-slack/slack-websocket.el:179-183 (`slack-ws-payload-presence-sub-p`).
- Fires presence_change for the subscribed ids; `batch_presence_aware=1` in the connect query makes changes arrive batched (`users` array). OBSERVED: keeper.go:247 and 408-410.
- Subscriptions are capped: emacs-slack documents 499 users max per subscribe and batches beyond that. OBSERVED: yuya373/emacs-slack/slack-websocket.el:925-929 ("first 499 users ... maximum number supported by the websocket").

### presence_query (client -> server)

- Payload: `{"type": "presence_query", "ids": ["U123", ...]}` - one-shot presence fetch over the socket (sibling of `presence_sub`; doesn't subscribe). OBSERVED: yuya373/emacs-slack/slack-websocket.el:183-185, 916-923 (`:type "presence_query" :ids user-ids`).
- Used for: filling green/gray dots for a transiently visible user set.

### presence_change (server -> client)

- Payload (single): `{"type": "presence_change", "user": "U123", "presence": "active|away"}`; batched: `{"type": "presence_change", "users": ["U123","U456"], "presence": "active"}`. OBSERVED: rusq/slackdump/internal/presence/keeper.go:408-410 (reads both `ev.User` and `ev.Users`), slack-go/slack@websocket_managed_conn.go:554 (`"presence_change": PresenceChangeEvent{}`).
- Fires: whenever a subscribed user's computed presence flips (activity, auto-away, manual away).

### manual_presence_change (server -> client)

- Payload: `{"type": "manual_presence_change", "presence": "away"}` - confirms your own users.setPresence/presence.set. OBSERVED: rusq/slackdump/internal/presence/keeper.go:412-414 (warns when `presence == "away"`), slack-go/slack@websocket_managed_conn.go:614.

### tickle (client -> server, presence keep-alive)

- Payload: `{"type": "tickle", "reason": "focus|mousedown|keydown|polling", "id": N}` - no reply. Desktop client throttles to one per 19s; `polling` is what the desktop app sends from its OS-idle timer. An open socket alone does NOT keep you active: observed auto_away ~30 min after the last tickle; sending a tickle every few minutes keeps you green. OBSERVED: rusq/slackdump/internal/presence/keeper.go:23-31 (doc comment with empirical timings), 54 (`MinInterval = 19 * time.Second`), 65 (`DefaultReason = "polling"`), 428-431 (tickle send).
- Used for: keeping the local user `active`. Implement a 4-min ticker with jitter for parity. OBSERVED: keeper.go:56-57 (`DefaultInterval = 4 * time.Minute`), 60-61 (`DefaultJitter = 0.25`).

### dnd_updated / dnd_updated_user (server -> client)

- Payload: `{"type": "dnd_updated", "user": "U123", "dnd_status": { ...DNDStatus... }}` (self, includes snooze fields); `{"type": "dnd_updated_user", "user": "U123", "dnd_status": {"dnd_enabled": false, "next_dnd_start_ts": ..., "next_dnd_end_ts": ...}}` (others). OBSERVED: slack-go/slack@websocket_dnd.go:3-6 (`DNDUpdatedEvent{Type, User, Status DNDStatus}`), slack-go/slack@websocket_managed_conn.go:567-568 (both types registered), emacs-slack: yuya373/emacs-slack/slack-websocket.el:510-514 (handles `dnd_updated_user`).
- Fires: when anyone toggles DND/snooze; refresh dnd badges without REST calls.

### user_status_changed (server -> client)

- Payload: `{"type": "user_status_changed", "user": { ...full SlackUserInfoPerson with new profile... }, "cache_ts": "...", "event_ts": "..."}`. OBSERVED: wee-slack/wee-slack/typings/slack_rtm/slack_rtm_message.pyi:326-331 (`SlackUserStatusChanged`), handler at wee-slack/wee-slack/slack/slack_workspace.py:702-706 (replaces the cached user object), slack-go/slack@websocket_managed_conn.go:617.
- Fires: after any `users.profile.set` by that user (status, display name, custom fields).

### user_profile_changed (server -> client)

- Payload: `{"type": "user_profile_changed", "user": { ...user object... }, "event_ts": "..."}` - profile edit variant. OBSERVED: slack-go/slack@websocket_managed_conn.go:619 (`"user_profile_changed": UserProfileChangedEvent{}`), mautrix handler: mautrix/slack@pkg/connector/handleslack.go:163 (`case *slack.UserProfileChangedEvent:`).

### user_invalidated (server -> client)

- Payload: `{"type": "user_invalidated", "user": {"id": "U123"}, "event_ts": "..."}` - cached user object is stale (deactivation, profile overhaul, Slack Connect state change). Client should re-fetch `users.info` for that id. OBSERVED: wee-slack/wee-slack/typings/slack_rtm/slack_rtm_message.pyi:333-339 (`SlackUserInvalidated`), handler wee-slack/wee-slack/slack/slack_workspace.py:708-717.

### pref_change (server -> client)

- Payload: `{"type": "pref_change", "name": "muted_channels", "value": "C123,C456"}` - one event per changed pref key; `value` is a string (JSON-stringified for composite prefs like `all_notifications_prefs`). OBSERVED: wee-slack/wee-slack/typings/slack_rtm/slack_rtm_message.pyi:320-324 (`SlackPrefChange`), handler wee-slack/wee-slack/slack/slack_workspace.py:695-700 (`muted_channels` and `all_notifications_prefs` branches), slack-go/slack@websocket_managed_conn.go:606.
- Fires: after any users.prefs.set by the local user (including ones made by other devices).

### reconnect_url (server -> client)

- Payload: `{"type": "reconnect_url", "url": "wss://..."}` - short-lived replacement socket URL handed out after connect; used to reconnect when the socket reports error code 1 (socket URL expired). OBSERVED: wee-slack/wee-slack/slack/slack_workspace.py:245,368-373,690-693.

## Gotchas

- **Auth shape**: every `/api/` POST takes `token=<xoxc->` in the urlencoded body (rotatable per request, not a header), while the persistent session is the `Cookie: d=<xoxd-...>` (HttpOnly). `Authorization: Bearer <xoxc>` + cookie also works. OBSERVED: .omo/research/gist.md (Identity & Auth: xoxc in request body) and wee-slack/wee-slack/slack/slack_api.py:62-69.
- **Private web-client params**: the web client appends `_x_id`, `_x_csid`, `slack_route=<ent>:<ent>`, `_x_version_ts`, `_x_gantry=true`, `_x_sonic=true` etc. to `/api/` URLs; slackdump synthesizes `_x_reason`/`_x_mode: online`/`_x_sonic`/`_x_app_name: client` fields for edge calls and it works without them being exact. OBSERVED: .omo/research/gist.md (Workspace API query params) and rusq/slackdump/internal/edge/edge.go:432-443 (`WebClientFields`, `webclientReason`). They are not known to be validated - send minimal requests first, add if you get rejected.
- **Rate limits**: public tiers apply (`users.list` tier2 ~20+/min, `users.info` tier4 100+/min, `dnd.teamInfo` max 50 users/call). 429s carry `Retry-After` (seconds or HTTP-date); the edge layer rate limits too. OBSERVED: https://docs.slack.dev/reference/methods/users.info.md (t4), rusq/slackdump/internal/edge/edge.go:264-312 (Retry-After parsing incl. `defRetryAfter = 5s` when header missing), https://docs.slack.dev/reference/methods/dnd.teaminfo.md (50 users).
- **Enterprise Grid**: users carry `enterprise_user` with `teams[]`; the same user id appears in several workspaces. Edge cache path takes **either** the org/enterprise id or the team id (`enterprise_id or workspace id` in wee-slack). `dnd.*` may return `missing_argument: team_id` after workspace migration - pass `team_id`. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:79-80 (`id_for_path = enterprise_id or workspace id`), wee-slack/wee-slack/typings/slack_api/slack_users_info.pyi (`SlackEnterpriseUser.teams`), slack-go/slack@dnd.go:10-12.
- **`updated_ids` conditional fetch**: always send the full id->ts map (0 for unknown); missing entries can be dropped or returned as `pending_ids`. Loop until `pending_ids` is empty before trusting a hydration pass. OBSERVED: rusq/slackdump/internal/edge/userlist.go:139-158.
- **users.list cursor limits**: standard cursor pagination, `limit` max 1000; very large orgs are better served by edge `users/list` (marker-based, ~50/page). OBSERVED: wee-slack/wee-slack/slack/slack_api.py:152-156, rusq/slackdump/internal/edge/userlist.go:249-251.
- **Presence is not a REST property**: `users.getPresence` is point-in-time; the truth lives on the socket (`presence_sub` + `presence_change`). Keep-alive requires `tickle` (19s throttle, ~30 min auto-away observed, doc says 10). An idle socket shows `auto_away: true` in `users.getPresence` (`connection_count` tells you how many devices). OBSERVED: rusq/slackdump/internal/presence/keeper.go:23-31, slack-go/slack@users.go:158-165.
- **Presence query/sub caps**: 499 ids per frame; batch larger sets. OBSERVED: yuya373/emacs-slack/slack-websocket.el:925-929.
- **Prefs are strings**: `users.prefs.get` returns a flat map where composite values (`all_notifications_prefs`, `emoji_use`, `frecency_jumper`, `sidebar_theme_custom_values`) are JSON-encoded _strings_; `users.prefs.set` takes one `name`/`value` pair (or the legacy flat form) and never a nested object. OBSERVED: ErikKalkoken/slackApiDoc@users.prefs.get.md, wee-slack/wee-slack/slack/slack_api.py:569-575, slack-go/slack@info.go:361-366.
- **Mute lists**: `muted_channels` is the legacy mute pref; the modern client writes `all_notifications_prefs` (per-channel `muted: true`). Set both for parity, and expect a `pref_change` echo per key. OBSERVED: wee-slack/wee-slack/typings/slack_api/slack_users_prefs.pyi, wee-slack/wee-slack/slack/slack_workspace.py:695-700.
- **DND write ops return no info**: `dnd.endDnd` returns bare `ok`; `dnd.setSnooze`/`dnd.endSnooze` return `DNDStatus`. After writes, reconcile via `dnd_updated` events instead of polling. OBSERVED: slack-go/slack@dnd.go:63-73, 85-96, 162-172.
- **Bots vs users**: bot senders resolve through `bots.info` (keyed by `bot`/`app_id`), not `users.info`; user objects for bots embed `profile.bot_id`/`profile.api_app_id` and `always_active`. OBSERVED: wee-slack/wee-slack/typings/slack_api/slack_profile.pyi (`SlackProfileBot`) and slack_bots_info.pyi.
- **Avatar URLs**: user avatars are `https://avatars.slack-edge.com/<YYYY-MM-DD>/<numeric-id>_<avatar_hash>_<size>.<png|jpg>` with sizes 24/32/48/72/192/512/1024 (one URL per size in `profile.image_*`; `image_original` is the uncropped upload). Deactivated users keep URLs. Team icons use the same host with sizes 34/44/68/88/102/132/230 (`team.icon.*`) plus `team.avatar_base_url` for relative paths. Team icon default (no custom image): `image_default: true`. OBSERVED: rusq/slackdump@internal/fixtures/users.go (`https://avatars.slack-edge.com/2019-09-14/760289794272_e9c1970fd6c2187993b1_24.png`, sizes 24/32/48/72/192/512/original), rusq/slackdump@internal/edge/assets/client.userBoot.json (team icon `image_34/44/68/88/102/132/230`), https://docs.slack.dev/reference/methods/users.info.md (`image_original ... /avatar/<hash>.jpg`), wee-slack/wee-slack/typings/slack_api/slack_profile.pyi (`image_24/32/48/72/192/512/1024`), wee-slack/wee-slack/typings/slack_api/slack_team_info.pyi (`avatar_base_url`, icon sizes, `image_default`).

## LEADS

- `users.lookupByEmail` (public) - not mined here; pair it with users area lookups later. Unverified.
- `users.setPhoto` docs page: slack-go cites `api.slack.com/methods/users.setPhoto` but the equivalent page on docs.slack.dev was not fetched during this pass - verify before treating parameter names as final. Unverified: success-response shape beyond `ok` (the earlier `profile` + `avatarHash` "parsed by slack-go" claim - slack-go actually parses bare `ok` only, slack-go/slack@users.go:532-551).
- `users/profile.setPhoto`-style edge endpoints and `-32c` style size-suffixed avatar URL variants (rendering-time crops): suspected from the web client, never observed - verify with a teardown before use. Unverified: user-avatar sizes 36/44/68/88/102/132/230 and a user-level `image_default` - no fetched source shows these on user profiles (they appear only as team-icon sizes); trimmed from the Avatar URLs gotcha during verification.
- Edge `users/counts` request/response shape: endpoint observed in teardown only; capture a live call to document.
- `users.priority.list` request params and response shape: observed as a boot call name only; capture a live call.
- Edge `users/list` `filter` values beyond `"everyone"` (web client also uses `"people"`, `NOT deactivated` style filters) and `index` values (`users_by_name`?): the two values documented here are the ones slackdump observed; sweep the web client for the full set.
- `frecency_ent_jumper` (org-wide frecency for jump/omnibox) referenced by third-party clients (`some-du6e/clack@docs/EMOJI-PICKER.md`, `ItsFelix5/slock@packages/plugins/src/prefs/server/index.ts`) - confirm the pref name and shape via live `users.prefs.get` on a Grid org.
- `presence.set` vs `users.setPresence`: presence.set is the web client's choice; whether it also stops the auto-away clock (vs manual away only) needs a live test.
- Websocket `presence_change` batching rules (when single `user` vs `users[]`) were only inferred from slackdump reading both fields; a controlled capture with 2+ subscriptions would pin the server's batching policy.
- `user_huddle_changed` exists alongside `user_status_changed`/`user_profile_changed` (slack-go CHANGELOG) - belongs to the huddles area but touches profile display state; cross-reference when documenting huddles.
- emacs-slack `slack-vip.el` / `slack-user.el` presence modeline rendering (per-user presence timers) may encode legacy rtm.start `presence` semantics worth a look for fallback behavior.
- `client.userBoot` (cross-area: boot) is the primary source of the initial user map + `prefs` dict (`dict[676]` observed); its user-related subset should stay in sync with this doc. OBSERVED: .omo/research/findings.md.
- mautrix-slack relies on slack-go/slack v0.24.0 RTM (no presence_sub/tickle implementation found in its connector) - it is a good reference for message/RTM event handling, not for presence parity. OBSERVED: mautrix/slack@go.mod (`github.com/slack-go/slack v0.24.0`) and pkg/connector/client.go:112,396-403.
