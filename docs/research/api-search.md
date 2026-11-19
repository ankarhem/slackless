# Search - Slack web-client API

## Summary

- Covers full-text search (messages, DMs, files, canvases, channels, people), the Cmd+K quick switcher, typeahead, recent searches / search history, and the frecency data that ranks switcher results.
- The web client's results page uses PRIVATE `search.modules.<module>` (form POST, one call per tab, all sharing one `search_session_id`); typeahead uses private `search.inline`, `search.autocomplete`, `search.autocomplete.files`; switcher people/channel lookups use the private Edge API (`edgeapi.slack.com/cache/<id>/users/search`, `channels/search`).
- PUBLIC `search.messages` / `search.files` / `search.all` still answer a session token but are marked legacy, return a flat `matches` shape and stop at page 100.
- Recent searches are server-side history (`search.autocomplete` with an empty query, `search.save`, `search.delete`), or the `recents` user pref when the `desktop_search_recents_*` experiments are on. No source uses a `recent_searches` pref.
- Biggest risks: (1) every private parameter comes from reverse-engineered clients (clack, slack-undoc-client, slackdump) plus one teardown, with no contract behind it; (2) Enterprise Grid needs the enterprise host for `search.modules.*` and `search.inline` and stores frecency as `frecency_ent_jumper`; (3) page numbers cap at 100; (4) experiments silently change which endpoints are called.

## Endpoints

Shared transport for every workspace-API method below: `POST https://<ws>.slack.com/api/<method>`, body `application/x-www-form-urlencoded` with `token=<xoxc>`, header `Cookie: d=<xoxd>`. OBSERVED: rusq/slackdump/internal/edge/edge.go:247-252, rusq/slackdump/internal/edge/edge.go:90. The web client itself sends multipart form-data and adds `_x_*` query params (`_x_id`, `_x_csid`, `slack_route=<E>:<E>`, `_x_version_ts`, ...). OBSERVED: ankarhem/slackless/.omo/research/gist.md:77-93. On Enterprise Grid some methods must go to `https://<org>.enterprise.slack.com/api/<method>` with the org-level token; the team host answers `team_is_restricted`. Each subsection says which. OBSERVED: some-du6e/clack/internal/slack/client.go:111-130, some-du6e/clack/internal/slack/client.go:155-156. Optional attribution fields the web client adds to form bodies: `_x_reason`, `_x_mode=online`, `_x_sonic=true`, `_x_app_name=client`. OBSERVED: rusq/slackdump/internal/edge/edge.go:433-447, rusq/slackdump/internal/edge/search.go:156-161.

One results-page search fires the active tab's module plus count-only calls for the inactive tabs (files and canvases `count=0`, dms 40, channels and people 20), all with the same `search_session_id` and `client_req_id`. OBSERVED: some-du6e/clack/internal/core/search.go:6-9, some-du6e/clack/internal/core/search.go:740-759, ankarhem/slackless/.omo/research/gist.md:481. `search_session_id` persists across pagination and refinements. OBSERVED: ankarhem/slackless/.omo/research/gist.md:626.

### search.modules.messages

- Transport: shared transport; on Grid it must use the enterprise host. OBSERVED: some-du6e/clack/internal/slack/client.go:125-126, IzieStratt/personal-site/website/public/__subdomains/vs/redocumented/docs/methods/search.md:13
- Visibility: private
- Request params ("module base args" = what the web client's searchModules fetcher sends on every `search.modules.*` call):
  - `module` - string - yes - `messages`. OBSERVED: some-du6e/clack/internal/core/search.go:81
  - `query` - string - yes - search text plus modifiers (see Query modifier syntax below); calling with no args answers `invalid_arguments`. OBSERVED: some-du6e/clack/internal/core/search.go:81, IzieStratt/personal-site/website/public/__subdomains/vs/redocumented/docs/methods/search.md:14-15
  - `page` - int - no - 1-based, defaults to 1. OBSERVED: some-du6e/clack/internal/core/search.go:75-81
  - `count` - int - no - 20 on the results page, 10 for Cmd+F search-in-channel. OBSERVED: some-du6e/clack/internal/core/search.go:151-166, ankarhem/slackless/.omo/research/gist.md:503
  - `client_req_id`, `search_session_id` - UUID strings - no - one pair per query, shared by every module call. OBSERVED: some-du6e/clack/internal/core/search.go:39-47, some-du6e/clack/internal/core/search.go:82
  - `extracts`=1, `highlight`=1, `max_extract_len`=200, `extra_message_data`=1, `no_user_profile`=1 - int - no - module base args; `highlight=1` adds the U+E000/U+E001 markers. OBSERVED: some-du6e/clack/internal/core/search.go:83-84, some-du6e/clack/internal/core/search.go:211-217
  - `file_title_only`=false, `query_rewrite_disabled`=false, `include_files_shares`=1 - module base args. OBSERVED: some-du6e/clack/internal/core/search.go:84-85
  - `search_context` - string - no - `desktop_messages_tab` (results page), `search_in_channel` (Cmd+F), `desktop_link_details` (link details pane). OBSERVED: some-du6e/clack/internal/core/search.go:68-70, some-du6e/clack/internal/core/search.go:160-163, some-du6e/clack/internal/core/channel_files_more.go:130
  - `search_exclude_bots`, `search_only_my_channels` - bool - no - copied from the user prefs of the same names ("Exclude bot messages", "Only my channels"). OBSERVED: some-du6e/clack/internal/core/search.go:14-16, some-du6e/clack/internal/core/search.go:183-184
  - `spell_correction` - string - no - `FUZZY_MATCH`. OBSERVED: some-du6e/clack/internal/core/search.go:185, ankarhem/slackless/.omo/research/gist.md:514
  - `search_only_team` - string - no - sent empty by the web client. OBSERVED: some-du6e/clack/internal/core/search.go:189
  - `facets_result_count` - int - no - 5. OBSERVED: some-du6e/clack/internal/core/search.go:190, ankarhem/slackless/.omo/research/gist.md:516
  - `query_refinement_suggestions_version` - int - no - 1. OBSERVED: some-du6e/clack/internal/core/search.go:191, ankarhem/slackless/.omo/research/gist.md:517
  - `recent_channels` - comma-joined conversation ids - no - recently visited conversations, used for ranking. OBSERVED: some-du6e/clack/internal/core/search.go:192, ankarhem/slackless/.omo/research/gist.md:519
  - `sort` / `sort_dir` - string - no - `score`/`desc` (relevance, default), `timestamp`/`desc` (newest), `timestamp`/`asc` (oldest). OBSERVED: some-du6e/clack/internal/core/search.go:117-131, ankarhem/slackless/.omo/research/gist.md:515
  - `disable_sort_override` - bool - no - sent only when the user locked the sort. OBSERVED: some-du6e/clack/internal/core/search.go:186-188
  - `search_exact_matches_only` - bool - no - "Exact matches only". OBSERVED: some-du6e/clack/internal/core/search.go:194-197
  - `channel_type` - string - no - `exclude_archived` ("Exclude archived", experiment `search_explore_freshness_signals`). OBSERVED: some-du6e/clack/internal/core/search.go:198-200, some-du6e/clack/docs/SEARCH.md:24
  - `max_filter_suggestions` - int - no - 10. OBSERVED: some-du6e/clack/internal/core/search.go:201
  - `request_context` - JSON string - no - `{"active_cid":"<open conversation>","recent_filter_in":[],"recent_filter_from":[]}`. OBSERVED: some-du6e/clack/internal/core/search.go:133-141, ankarhem/slackless/.omo/research/gist.md:520
  - `search_tab_filter` = `messages`, `search_tab_sort` = same as `sort`. OBSERVED: some-du6e/clack/internal/core/search.go:203-204
- Response (shape; values are placeholders):
  ```json
  {
    "ok": true,
    "module": "messages",
    "query": "<echoed>",
    "filters": { "in": ["C.."], "from": ["U.."] },
    "manual_filters": { "in": [], "from": [] },
    "ai_filters": {},
    "pagination": {
      "total_count": 0,
      "page": 1,
      "per_page": 20,
      "page_count": 1,
      "first": 1,
      "last": 20
    },
    "items": [
      {
        "iid": "..",
        "team": "T..",
        "channel": {
          "id": "C..",
          "name": "general",
          "is_channel": true,
          "is_group": false,
          "is_im": false,
          "is_mpim": false,
          "is_private": false,
          "is_shared": false,
          "is_org_shared": false,
          "is_ext_shared": false,
          "user": "U.. (IMs)"
        },
        "messages": [
          {
            "ts": "..",
            "user": "U..",
            "username": "..",
            "iid": "..",
            "text": "..\ue000term\ue001..",
            "permalink": "https://..",
            "blocks": [],
            "blocks_extracts": [],
            "thread_ts": "..",
            "reply_count": 0,
            "latest_reply": "..",
            "attachments": [],
            "bot_id": "B..",
            "bot_profile": {},
            "requestor_replied": false
          }
        ]
      }
    ],
    "filter_suggestions": {
      "in": [{ "id": "C..", "facet_count": 0, "checked": false }],
      "from": []
    },
    "unfiltered_result_count": 0,
    "unfiltered_result_filter": {},
    "sort_forced_to_recent": false
  }
  ```
  - Envelope, `items[].channel`, `items[].messages[]`, `blocks_extracts` (whose `text` is `{text, truncated_head, truncated_tail}`): OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:20163-20279
  - Hits are grouped per channel: each item holds one `channel` and a `messages` array. OBSERVED: myurasov/Slackwright/src/slackwright/search.py:408-416, ItsFelix5/slock/packages/plugins/src/search/server/index.ts:175-179
  - `filter_suggestions.{in,from}[].{id,facet_count,checked}` feed the From/In menus; `unfiltered_result_count` + `unfiltered_result_filter` drive "Search without filters"; `sort_forced_to_recent` (counts only when it is literally `true`); `messages[].requestor_replied`. OBSERVED: some-du6e/clack/internal/core/search.go:364-386, some-du6e/clack/internal/core/search.go:418-421, some-du6e/clack/docs/SEARCH.md:24
  - Matched spans in Block Kit carry `"client_highlight": true` style. OBSERVED: ankarhem/slackless/.omo/research/gist.md:524
- Pagination: page-based (`page`, `count`; stop at `pagination.page_count` or on a short page). OBSERVED: myurasov/Slackwright/src/slackwright/search.py:355-362, some-du6e/clack/internal/core/search.go:381-393
- Used for: results page Messages tab, Cmd+F search-in-channel (`query = "in:<#C..> <q>"`, count 10, `search_exclude_bots=false`), the link-details pane. OBSERVED: some-du6e/clack/internal/core/search.go:21-22, some-du6e/clack/internal/core/search.go:160-181, some-du6e/clack/docs/SEARCH.md:29

#### Query modifier syntax (the `query` of every search method)

| Modifier                   | Request form / values                                                                                                                                                                | OBSERVED                                                                                                                                                         |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `"phrase"`                 | exact phrase                                                                                                                                                                         | https://slack.com/help/articles/202528808-Search-in-Slack                                                                                                        |
| `-word`, `-"phrase"`       | exclude                                                                                                                                                                              | https://slack.com/help/articles/202528808-Search-in-Slack, some-du6e/clack/docs/SEARCH.md:57-60                                                                  |
| `in:`                      | `in:#channel` in the UI; the request uses `in:<#C..>` (channel) or `in:<@U..>` (DM); a section name also works in the UI                                                             | https://docs.slack.dev/reference/methods/search.messages, some-du6e/clack/internal/core/search.go:162, https://slack.com/help/articles/202528808-Search-in-Slack |
| `from:`                    | `from:@Name` in the UI, `from:<@U..>` in the request; `from:botname`                                                                                                                 | https://docs.slack.dev/reference/methods/search.messages, some-du6e/clack/docs/SEARCH.md:100-101                                                                 |
| `-in:`, `-from:`           | negated scope                                                                                                                                                                        | https://slack.com/help/articles/202528808-Search-in-Slack                                                                                                        |
| `to:`                      | user value (people type it; no filter menu writes it)                                                                                                                                | some-du6e/clack/internal/ui/popups/search_rank.go:293-300                                                                                                        |
| `with:`                    | `with:<@U..>`; "Includes me" = `with:<@self>` (threads and DMs with that person)                                                                                                     | some-du6e/clack/docs/SEARCH.md:46, some-du6e/clack/docs/SEARCH.md:51, https://slack.com/help/articles/202528808-Search-in-Slack                                  |
| `has:`                     | `has:link`, `has:file`, `has:action`, `has:pin`, reaction `has::name:`                                                                                                               | some-du6e/clack/docs/SEARCH.md:47-52, https://slack.com/help/articles/202528808-Search-in-Slack                                                                  |
| `hasmy:`                   | your own reaction, `hasmy::name:`                                                                                                                                                    | rubencodes/EmojiWrapped/README.md:30, https://slack.com/help/articles/202528808-Search-in-Slack                                                                  |
| `is:`                      | `is:thread`, `is:saved`, `is:dm`, `is:todos`                                                                                                                                         | some-du6e/clack/docs/SEARCH.md:50, https://slack.com/help/articles/202528808-Search-in-Slack                                                                     |
| `in:activity`              | "In Activity" filter                                                                                                                                                                 | some-du6e/clack/docs/SEARCH.md:50                                                                                                                                |
| `before:`, `after:`, `on:` | dates such as `before:2026-11-05`; presets expand to ranges (Today = `after:<yesterday> before:<tomorrow>`)                                                                          | some-du6e/clack/docs/SEARCH.md:48, some-du6e/clack/docs/SEARCH.md:110-113, rubencodes/EmojiWrapped/README.md:30                                                  |
| `during:`                  | a month or year (`during:august`)                                                                                                                                                    | https://slack.com/help/articles/202528808-Search-in-Slack                                                                                                        |
| `type:`                    | `huddle_notes`, `lists`, `canvases`, `documents`, `emails`, `images`, `pdfs`, `presentations`, `snippets`, `spreadsheets`, `audio`, `videos`; the Canvases tab sends `type:quip <q>` | some-du6e/clack/docs/SEARCH.md:53, some-du6e/clack/internal/core/search.go:460-461                                                                               |
| `creator:`                 | canvas creator                                                                                                                                                                       | https://slack.com/help/articles/202528808-Search-in-Slack                                                                                                        |
| `threads:`, `dm:`          | added in web build 407aff01646b (`threads:` takes all / replies)                                                                                                                     | some-du6e/clack/docs/SEARCH.md:62-63, some-du6e/clack/internal/ui/popups/search_rank.go:295-296                                                                  |
| `word*`                    | prefix match, at least 3 characters                                                                                                                                                  | https://slack.com/help/articles/202528808-Search-in-Slack                                                                                                        |

The client's filter autocompleter tries modifier types in this order: `from to with in is has hasmy creator type before after on during dm threads`. OBSERVED: some-du6e/clack/internal/ui/popups/search_filter_ac.go:38-40. The web client puts plain terms first and modifiers after them. OBSERVED: some-du6e/clack/docs/SEARCH.md:57-58. "Exclude bot messages" and "Only my channels" are prefs/params, not modifiers. OBSERVED: some-du6e/clack/docs/SEARCH.md:54-55.

#### Highlight markers

U+E000 opens a match and U+E001 closes it (UTF-8 `EE 80 80` / `EE 80 81`). The public methods add them only with `highlight=true`; `search.modules.*` and `search.inline` add them with `highlight=1`. OBSERVED: https://docs.slack.dev/reference/methods/search.messages, ankarhem/slackless/.omo/research/gist.md:521-522. They arrive either raw or JSON-escaped (`\ue000`, in any hex case) and also inside `blocks`/`blocks_extracts`, so strip them before JSON decoding or rendering and keep the offsets for the highlight UI. OBSERVED: some-du6e/clack/internal/core/search.go:211-287, some-du6e/clack/internal/core/search.go:375-378

### search.modules.dms

- Transport: shared transport; on Grid it must use the enterprise host. OBSERVED: some-du6e/clack/internal/slack/client.go:127-130
- Visibility: private
- Request params: module base args (see search.modules.messages) with `module=dms`, `count=40`, `search_context=desktop_dms_tab`; no messages-only extras. OBSERVED: some-du6e/clack/internal/core/search.go:145-174, some-du6e/clack/internal/core/search.go:68-69, some-du6e/clack/docs/SEARCH.md:25, ImShyMike/slack-undoc-client/src/types.ts:1766-1782
- Response:
  ```json
  {
    "ok": true,
    "module": "dms",
    "query": "..",
    "filters": { "in": [], "from": [] },
    "pagination": {
      "total_count": 0,
      "page": 1,
      "per_page": 40,
      "page_count": 1,
      "first": 1,
      "last": 1
    },
    "items": [
      {
        "channel": {
          "id": "D..",
          "is_im": true,
          "created": 0,
          "updated": 0,
          "is_org_shared": false,
          "context_team_id": "T..",
          "channel_name_highlighted": ".."
        },
        "message": {
          "ts": "..",
          "user": "U..",
          "type": "message",
          "text": "..",
          "team": "T..",
          "client_msg_id": "..",
          "user_profile": {}
        }
      }
    ]
  }
  ```
  Unlike `messages`, each item carries a single `message` object. OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:19821-19862
- Pagination: page-based, as for messages. OBSERVED: some-du6e/clack/internal/core/search.go:171
- Used for: results page DMs tab, and its count in the source menu. OBSERVED: some-du6e/clack/internal/core/search.go:740-747, ankarhem/slackless/.omo/research/gist.md:507

### search.modules.files

- Transport: shared transport; on Grid it must use the enterprise host. OBSERVED: some-du6e/clack/internal/slack/client.go:126
- Visibility: private
- Request params: module base args with `module=files`, plus:
  - `count` - int - no - 20; `0` asks only for the total (inactive-tab count). OBSERVED: some-du6e/clack/internal/core/search.go:451-458
  - `query` - string - yes - the Canvases tab sends `type:quip <q>`. OBSERVED: some-du6e/clack/internal/core/search.go:459-462
  - `search_context` - string - no - `desktop_files_tab`, `desktop_canvases_tab`, `desktop_files_channel_tab` (a channel's Files tab), `desktop_files_browser`, `desktop_files_search`. OBSERVED: some-du6e/clack/internal/core/search.go:68-70, some-du6e/clack/internal/core/channel_files.go:118, some-du6e/clack/internal/core/browse.go:378, ItsFelix5/slock/packages/plugins/src/search/server/index.ts:121
  - `search_exclude_bots`, `search_only_my_channels` - bool - from prefs; `search_only_team`=""; `facets_result_count`=5. OBSERVED: some-du6e/clack/internal/core/search.go:464-467
  - `sort` / `sort_dir` - string - no - Sort select values `score` (default), `timestamp`, `timestamp_asc`, `created`, `name`, `name_desc` map to `score/desc`, `timestamp/desc`, `timestamp/asc`, `created/desc`, `name/asc`, `name/desc`; a count-only call is forced to `timestamp/desc`. OBSERVED: some-du6e/clack/internal/core/search.go:88-115, some-du6e/clack/internal/core/search.go:468-480
  - `max_filter_suggestions`=10, `request_context` (empty `active_cid`), `search_tab_filter`=`messages` (sic), `search_tab_sort`=`score`. OBSERVED: some-du6e/clack/internal/core/search.go:481-485, ImShyMike/slack-undoc-client/src/types.ts:1784-1810
- Response:
  ```json
  {
    "ok": true,
    "module": "files",
    "query": "..",
    "filters": { "in": [], "file_types": [], "from": [] },
    "manual_filters": {},
    "ai_filters": {},
    "pagination": {
      "total_count": 0,
      "page": 1,
      "per_page": 20,
      "page_count": 1,
      "first": 1,
      "last": 1
    },
    "items": [
      {
        "id": "F..",
        "created": 0,
        "timestamp": 0,
        "name": "..",
        "title": "..",
        "mimetype": "..",
        "filetype": "..",
        "pretty_type": "..",
        "user": "U..",
        "user_team": "T..",
        "size": 0,
        "mode": "..",
        "is_external": false,
        "is_public": false,
        "url_private": "..",
        "url_private_download": "..",
        "permalink": "..",
        "permalink_public": "..",
        "skipped_shares": false
      }
    ]
  }
  ```
  OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:19973-20024
- Pagination: page-based. OBSERVED: some-du6e/clack/internal/core/search.go:496-499
- Used for: results page Files and Canvases tabs, a channel's Files tab, the files browser. The teardown saw it fire twice per search (different file types). OBSERVED: ankarhem/slackless/.omo/research/gist.md:504, some-du6e/clack/internal/core/search.go:488-493

### search.modules.channels

- Transport: shared transport; on Grid it must use the enterprise host. OBSERVED: some-du6e/clack/internal/slack/client.go:125
- Visibility: private
- Request params: module base args with `module=channels`, `count=20`, `search_context=desktop_channels_tab`, plus:
  - `search_only_my_channels` - bool - pref; `exclude_my_channels` - 0/1 - `1` when the pref `search_hide_my_channels` is set ("Other channels"). OBSERVED: some-du6e/clack/internal/core/search.go:550-562
  - `channel_type` - string - no - `all`/empty, `private_exclude` (public only), `private`, `archived`, `external_shared`, `exclude_archived`. OBSERVED: some-du6e/clack/internal/core/search.go:540-547, rusq/slackdump/internal/edge/search.go:77-84
  - `external_shared_team_ids`="", `search_only_team`="", `request_context`, `spell_correction=FUZZY_MATCH`. OBSERVED: some-du6e/clack/internal/core/search.go:557-572
  - `sort` / `sort_dir` - string - no - Sort select values `relevant` (default), `created`, `created_asc`, `member_count`, `member_count_asc`, `name`, `name_desc` map to `relevant/desc`, `created/desc`, `created/asc`, `member_count/desc`, `member_count/asc`, `name/asc`, `name/desc` (the `_asc`/`_desc` values are never sent as `sort`). OBSERVED: some-du6e/clack/internal/core/search.go:88-115, some-du6e/clack/internal/core/search.go:565-571. In browse mode `sort=recommended` by default, or `name`/`created`/`member_count` with a `sort_dir`; slackdump sends `name`/`asc`. OBSERVED: some-du6e/clack/internal/core/browse.go:663-676, rusq/slackdump/internal/edge/search.go:94-99, rusq/slackdump/internal/edge/search.go:150-151
  - Channel browser mode: `browse=standard`, `search_context=desktop_channel_browser`; an empty `query` lists every channel. Other browser fields: `extracts=0`, `highlight=0`, `recommend_source=channel-browser`, `_x_reason=browser-query`. OBSERVED: some-du6e/clack/internal/slack/more.go:514-533, rusq/slackdump/internal/edge/search.go:131-162
  - `cursor` - string - no - `*` on the first call, then `pagination.next_cursor`. OBSERVED: rusq/slackdump/internal/edge/search.go:140, rusq/slackdump/internal/edge/search.go:180-185
- Response:
  ```json
  {
    "ok": true,
    "module": "channels",
    "query": "",
    "filters": {},
    "ai_filters": {},
    "manual_filters": {},
    "pagination": {
      "total_count": 6,
      "page": 1,
      "per_page": 50,
      "page_count": 1,
      "first": 1,
      "last": 6,
      "next_cursor": ".."
    },
    "items": [
      {
        "id": "C..",
        "iid": "..",
        "name": "random",
        "is_member": false,
        "member_count": 3,
        "context_team_id": "T..",
        "purpose": { "value": "..", "creator": "U..", "last_set": 0 },
        "timestamp": 0,
        "previous_name": ".."
      }
    ]
  }
  ```
  OBSERVED: rusq/slackdump/internal/edge/assets/search.module.channels.json:1-24, ImShyMike/slack-undoc-client/src/types.ts:19783-19819, rusq/slackdump/internal/edge/edge.go:464-472
- Pagination: cursor (`next_cursor`) or `page`/`page_count`. Asking for `page` > 100 silently returns page 1. OBSERVED: rusq/slackdump/internal/edge/edge.go:456-463, some-du6e/clack/internal/slack/more.go:534-542
- Used for: results page Channels tab and the channel browser. OBSERVED: some-du6e/clack/internal/core/search.go:576-600, some-du6e/clack/internal/core/browse.go:664

### search.modules.people

- Transport: shared transport; on Grid it must use the enterprise host. OBSERVED: some-du6e/clack/internal/slack/client.go:125
- Visibility: private
- Request params: module base args with `module=people`, `count=20`, `search_context=desktop_people_tab` (people browser: `desktop_people_browser`, `browse=standard`, `sort=recommended`), plus:
  - `tz`="", `external_shared_team_ids`="", `search_only_team`="", `profile_division`="", `profile_department`="". OBSERVED: some-du6e/clack/internal/core/search.go:659-665
  - `profile_location`, `profile_title` - comma-joined strings - no - the Location and Title pills; `profile_celebration` - `new_hire`, `anniversary`. OBSERVED: some-du6e/clack/internal/core/search.go:637-643, some-du6e/clack/internal/core/search.go:662-666
  - `custom_fields` - JSON string - no - `{}`. OBSERVED: some-du6e/clack/internal/core/search.go:667
  - `hide_deactivated_users` - 0/1 - pref `search_hide_deactivated_users`. OBSERVED: some-du6e/clack/internal/core/search.go:668-672
  - `request_context`, `spell_correction=FUZZY_MATCH`, `sort`/`sort_dir`: Sort select values `relevant` (default), `name`, `name_desc` map to `relevant/desc`, `name/asc`, `name/desc`. OBSERVED: some-du6e/clack/internal/core/search.go:673-681, some-du6e/clack/internal/core/search.go:88-115
  - `account_type` - comma-joined ints - no - owners `0,1`, admins `0,1,2`, full `0,1,2,3,7`, guests `4,5,8`, deactivated `6`, profile_only `9`, frontline `10`; omitted for "any". OBSERVED: some-du6e/clack/internal/core/search.go:646-651, some-du6e/clack/internal/core/search.go:682-684
- Response:
  ```json
  {
    "ok": true,
    "module": "people",
    "query": "..",
    "filters": {},
    "ai_filters": {},
    "manual_filters": {},
    "items": [
      {
        "iid": "..",
        "id": "U..",
        "username": "..",
        "is_restricted": false,
        "is_ultra_restricted": false,
        "phone": "",
        "timestamp": 0,
        "profile": {
          "real_name": "..",
          "display_name": "..",
          "title": "..",
          "status_text": "..",
          "status_emoji": "..",
          "image_72": "..",
          "team": "T..",
          "fields": {}
        },
        "top_terms": true,
        "similar": false
      }
    ],
    "pagination": {
      "total_count": 0,
      "page": 1,
      "per_page": 20,
      "page_count": 1,
      "first": 1,
      "last": 1
    },
    "filter_suggestions": {
      "title": [{ "value": "..", "facet_count": 0, "checked": false }],
      "department": [],
      "location": []
    }
  }
  ```
  OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:20433-20524, some-du6e/clack/docs/SEARCH.md:28
- Pagination: page-based. OBSERVED: some-du6e/clack/internal/core/search.go:710-712
- Used for: results page People tab and the people browser. Switcher typeahead people come from edge `users/search` instead (see below). OBSERVED: some-du6e/clack/internal/core/browse.go:775, some-du6e/clack/docs/SEARCH.md:16

### search.inline

- Transport: shared transport; on Grid it must use the enterprise host (the team host answers `team_is_restricted`). OBSERVED: some-du6e/clack/internal/slack/client.go:127-130
- Visibility: private
- Request params (switcher "Recent messages"):
  - `query` - string - yes; `count` - int - 3; `page` - int - 1. OBSERVED: some-du6e/clack/internal/core/search.go:776-790
  - `search_session_id`, `client_req_id` - UUIDs. OBSERVED: some-du6e/clack/internal/core/search.go:787
  - `max_ts` - unix seconds - now. OBSERVED: some-du6e/clack/internal/core/search.go:783-788
  - `thread_replies` - string - "" ; `extract_len` - int - 110. OBSERVED: some-du6e/clack/internal/core/search.go:789
  - `from_me`, `with_me` - bool. OBSERVED: some-du6e/clack/internal/core/search.go:790
  - `channel` or `user` - id - no - scopes to one conversation (from an `in:` filter, or the open channel when experiment `omni_quick_message_results` is off). karbassi reports exactly one of them is required. OBSERVED: some-du6e/clack/internal/core/search.go:791-796, some-du6e/clack/docs/SEARCH.md:18, karbassi/slack-mcp/docs/undocumented-endpoints.md:48
  - `recent_channels` - comma-joined ids - no - sent when `omni_quick_message_results` is on. OBSERVED: some-du6e/clack/internal/core/search.go:797-799, some-du6e/clack/docs/SEARCH.md:18
  - Find-in-thread mode: `query`, `channel`, `thread_ts`, `find_in_thread=true`, `count=100`, `cursor`. OBSERVED: some-du6e/clack/internal/core/search.go:997-1009
- Response:
  ```json
  {
    "ok": true,
    "query": "..",
    "pagination": {
      "total_count": 0,
      "page": 1,
      "per_page": 3,
      "page_count": 1,
      "first": 1,
      "last": 1
    },
    "items": [
      {
        "ts": "..",
        "user": "U..",
        "channel_id": "C..",
        "permalink": "..",
        "iid": "..",
        "subtype": "..",
        "bot_id": "B..",
        "extracts": [
          { "text": "..", "truncated_head": false, "truncated_tail": true }
        ]
      }
    ],
    "next_cursor": ".."
  }
  ```
  OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:19761-19781, some-du6e/clack/internal/core/search.go:820-831, some-du6e/clack/internal/core/search.go:1011-1016
- Pagination: `page` for the switcher; `cursor`/`next_cursor` in find-in-thread mode (clack stops after 500 pages). OBSERVED: some-du6e/clack/internal/core/search.go:1003-1031
- Used for: switcher popover "Recent messages" (only for plain words, or words plus one `in:`, of 3+ characters), find-in-thread, and the teardown's "quick 3-result preview". OBSERVED: some-du6e/clack/docs/SEARCH.md:18, ankarhem/slackless/.omo/research/gist.md:489

### search.autocomplete

- Transport: shared transport. OBSERVED: some-du6e/clack/internal/core/search_recents.go:971
- Visibility: private
- Request params:
  - `query` - string - no - `""` returns the user's search history. OBSERVED: some-du6e/clack/internal/core/search_recents.go:971, ImShyMike/slack-undoc-client/src/types.ts:1693-1697
  - `limit` - int - no - 100. OBSERVED: some-du6e/clack/internal/core/search_recents.go:971
  - `use_hash` - bool - no - `true` (returns a `hash` per entry, used by `search.delete`). OBSERVED: some-du6e/clack/internal/core/search_recents.go:971, some-du6e/clack/internal/core/search_delete.go:47-56
- Response:
  ```json
  {
    "ok": true,
    "suggestions": [
      {
        "hash": "..",
        "text": "<query>",
        "type": "..|slackbot",
        "ts": 0,
        "count": 0,
        "slackbot_thread_ts": ".."
      }
    ],
    "query_suggestions": []
  }
  ```
  The client keeps only entries that have all of count, hash, text, ts and type; `type == "slackbot"` marks a Slackbot AI conversation. OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:19207-19217, some-du6e/clack/internal/core/search_recents.go:961-985
- Pagination: none; `limit` only. OBSERVED: some-du6e/clack/internal/core/search_recents.go:971
- Used for: the switcher's "Recent searches" when `desktop_search_recents_read` is off. The client caches it for 5 minutes and drops the cache after `search.save` or `search.delete`. The teardown also lists it as the "suggestion dropdown as you type". OBSERVED: some-du6e/clack/internal/core/search_recents.go:19-26, some-du6e/clack/internal/core/search_recents.go:950-1000, ankarhem/slackless/.omo/research/gist.md:490

### search.autocomplete.files

- Transport: shared transport. OBSERVED: some-du6e/clack/internal/core/search.go:868
- Visibility: private
- Request params: `query` - string - yes (the client skips the call when blank); `include_shares` - bool - `true`. OBSERVED: some-du6e/clack/internal/core/search.go:862-867, ImShyMike/slack-undoc-client/src/types.ts:1699-1702
- Response: `{"ok": true, "files": [ <file objects> ]}`; highlight markers may appear. OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:19345-19349, some-du6e/clack/internal/core/search.go:871-876
- Pagination: none observed. OBSERVED: some-du6e/clack/internal/core/search.go:860-882
- Used for: the switcher popover's file and canvas rows. OBSERVED: some-du6e/clack/docs/SEARCH.md:19, ankarhem/slackless/.omo/research/gist.md:491

### search.save

- Transport: shared transport. OBSERVED: some-du6e/clack/internal/core/search.go:894
- Visibility: private
- Request params: `terms` - string - yes - the submitted query; `type` - string - yes - `default` (the switcher's search mode). OBSERVED: some-du6e/clack/internal/core/search.go:884-895, ImShyMike/slack-undoc-client/src/types.ts:1903-1906, karbassi/slack-mcp/docs/undocumented-endpoints.md:49
- Response: `{"ok": true}`. OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:20619-20621
- Pagination: n/a. OBSERVED: some-du6e/clack/internal/core/search.go:894
- Used for: adding a submitted search to the server-side history that `search.autocomplete` returns. OBSERVED: ankarhem/slackless/.omo/research/gist.md:508, some-du6e/clack/internal/core/search.go:884-887

### search.delete

- Transport: shared transport. OBSERVED: some-du6e/clack/internal/core/search_delete.go:60
- Visibility: private
- Request params: `hash` - string - one of hash/terms - the `hash` from `search.autocomplete`; `terms` - string - fallback when there is no hash; `type` - string - no - `slackbot` for a Slackbot conversation row. OBSERVED: some-du6e/clack/internal/core/search_delete.go:47-60
- Response: plain `ok` envelope; the observed client does not parse the body. OBSERVED: some-du6e/clack/internal/core/search_delete.go:60
- Pagination: n/a. OBSERVED: some-du6e/clack/internal/core/search_delete.go:47-63
- Used for: "Delete" on a Recent searches row. With the `recents` pref active, the client also removes the canonicalized query (channel refs without names) from the pref. OBSERVED: some-du6e/clack/docs/SEARCH.md:118-119, some-du6e/clack/internal/core/search_delete.go:13-45

### search.precache

- Transport: shared transport. slack-undoc-client sends every method in its type map, this one included, as a form-urlencoded POST with `token` and the `d` cookie (to `https://slack.com/api/<method>`); the web client fires it through the same `/api/` path. OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:25527, ImShyMike/slack-undoc-client/src/client.template.ts:223-252, ankarhem/slackless/.omo/research/gist.md:77-79, ankarhem/slackless/.omo/research/gist.md:509
- Visibility: private
- Request params: `enterprise_token` - string - no. OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:1899-1901
- Response: `{"ok": true}`. OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:20615-20617
- Pagination: n/a. OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:1899-1901
- Used for: fired by the web client after results are shown (pre-warming). Not needed for correctness. OBSERVED: ankarhem/slackless/.omo/research/gist.md:509, ankarhem/slackless/.omo/research/gist.md:622

### search.feedback

- Transport: shared transport. OBSERVED: some-du6e/clack/internal/core/search.go:994
- Visibility: private
- Request params: `client_token` - string - yes - a client-generated token; `type` - string - `search`; `payload` - JSON string. OBSERVED: some-du6e/clack/internal/core/search.go:984-995, some-du6e/clack/docs/SEARCH.md:31
- Response: not parsed; Slack answers by opening its feedback dialog for that `client_token` over the socket. OBSERVED: some-du6e/clack/internal/core/search.go:950-953, some-du6e/clack/internal/core/search.go:984-987
- Pagination: n/a. OBSERVED: some-du6e/clack/internal/core/search.go:984-995
- Used for: "Give feedback" in search-in-channel. The popover and results page instead call `feedback.submitEnterpriseSearchFeedback` (`type=feedback`, `feedback_type=omniswitcher|search`, `text`, `contact`, `category`, `query`, `payload`, `is_test=false`). OBSERVED: some-du6e/clack/internal/core/search.go:970-982, some-du6e/clack/docs/SEARCH.md:30

### search.autocomplete.topEmojis

- Transport: shared transport, called at boot. OBSERVED: ankarhem/slackless/.omo/research/gist.md:171-172
- Visibility: private
- Request params: `count` - int - no. OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:1704-1706
- Response: `{"ok": true, "user": ["eyes", ".."], "org": [".."]}` (emoji names). OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:19565-19569
- Pagination: n/a. OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:1704-1706
- Used for: the hover toolbar's personalized quick reactions (cross-area: reactions). OBSERVED: ankarhem/slackless/.omo/research/gist.md:614

### search.autocomplete.model

- Transport: shared transport, no params (`search.autocomplete.offlineFeatures` and `search.autocomplete.intentModel` are likewise param-less). OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:25513-25515, some-du6e/clack/internal/core/autocomplete.go:84, some-du6e/clack/internal/core/autocomplete.go:133
- Visibility: private
- Request params: none. OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:25514
- Response: `{"ok": true, "model": {"<feature_name>": <weight>, ...}}` (a name-to-number map). OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:19410, ImShyMike/slack-undoc-client/src/types.ts:19524-19527. `intentModel` returns `{"ok": true, "intent_model": {"weights": {..}, "bias": <n>, "threshold": <n>}}`. OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:19401-19408
- Pagination: n/a. OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:25514
- Used for: ranking weights for composer/switcher autocomplete together with the `frecency` pref (cross-area: composer). OBSERVED: some-du6e/clack/internal/ui/composer/autocomplete_slack_test.go:24-28

### search.messages

- Transport: `GET` or `POST https://slack.com/api/search.messages`, form-encoded. OBSERVED: https://docs.slack.dev/reference/methods/search.messages. Session clients POST it like any Web API method, on either host: emacs-slack to `https://slack.com/api/search.messages` (form fields `query`, `sort`, `sort_dir`, `page`), clack to the team host `https://<ws>.slack.com/api/search.messages`. A team xoxc works. OBSERVED: emacs-slack/emacs-slack/slack-search.el:216-217, emacs-slack/emacs-slack/slack-search.el:249-257, some-du6e/clack/internal/slack/more.go:357, some-du6e/clack/internal/slack/client.go:492-497, IzieStratt/personal-site/website/public/__subdomains/vs/redocumented/docs/methods/block-actions-and-modals-2026-09.md:114
- Visibility: public (legacy; Slack points to `assistant.search.context`). OBSERVED: https://docs.slack.dev/reference/methods/search.messages
- Request params:
  - `query` - string - yes. OBSERVED: https://docs.slack.dev/reference/methods/search.messages
  - `count` - int - no - default 20, max 100. OBSERVED: https://docs.slack.dev/reference/methods/search.messages
  - `page` - int - no - default 1, max 100. OBSERVED: https://docs.slack.dev/reference/methods/search.messages
  - `cursor` - string - no - `*` first, then `next_cursor`. OBSERVED: https://docs.slack.dev/reference/methods/search.messages, rusq/slack/search.go:118-119
  - `highlight` - bool - no - adds U+E000/U+E001. OBSERVED: https://docs.slack.dev/reference/methods/search.messages
  - `sort` - `score` (default) | `timestamp`; `sort_dir` - `desc` (default) | `asc`. OBSERVED: https://docs.slack.dev/reference/methods/search.messages, emacs-slack/emacs-slack/slack-search.el:198-214
  - `team_id` - string - only with org-level tokens. OBSERVED: https://docs.slack.dev/reference/methods/search.messages
- Response:
  ```json
  {
    "ok": true,
    "query": "..",
    "messages": {
      "total": 2,
      "matches": [
        {
          "channel": {
            "id": "C..",
            "name": "general",
            "is_private": false,
            "is_mpim": false,
            "is_shared": false,
            "is_ext_shared": false,
            "is_org_shared": false,
            "pending_shared": []
          },
          "iid": "..",
          "permalink": "..",
          "team": "T..",
          "text": "..",
          "ts": "..",
          "type": "message",
          "user": "U..",
          "username": ".."
        }
      ],
      "pagination": {
        "first": 1,
        "last": 2,
        "page": 1,
        "page_count": 1,
        "per_page": 20,
        "total_count": 2,
        "next_cursor": ".."
      },
      "paging": { "count": 20, "page": 1, "pages": 1, "total": 2 }
    }
  }
  ```
  IM hits have `type: "im"` with `channel.name` set to the other user's id; private group hits have `type: "group"`. OBSERVED: https://docs.slack.dev/reference/methods/search.messages, rusq/slack/pagination.go:20. slack-go also decodes `blocks`, `attachments`. OBSERVED: beeper/slackgo/search.go:45-59
- Pagination: `page`+`count` (both capped at 100) or cursor. slackdump pages with `cursor="*"`, `sort=timestamp`, `count=100` until `next_cursor` is empty. OBSERVED: rusq/slackdump/stream/search.go:111-135. emacs-slack uses page numbers: next page = min(page+1, page_count). OBSERVED: emacs-slack/emacs-slack/slack-search.el:114-119
- Used for: fallback or bulk export; not what the web client's results page calls. OBSERVED: ankarhem/slackless/.omo/research/gist.md:498-509

### search.files

- Transport: as search.messages. OBSERVED: https://docs.slack.dev/reference/methods/search.files
- Visibility: public (legacy). OBSERVED: https://docs.slack.dev/reference/methods/search.files
- Request params: `query` (required), `count` (max 100), `page` (max 100), `highlight`, `sort` (`score`|`timestamp`), `sort_dir`, `team_id`; the doc lists no `cursor`, but slackdump sends one and follows `next_cursor`. OBSERVED: https://docs.slack.dev/reference/methods/search.files, rusq/slackdump/stream/search.go:149-176
- Response: `{"ok": true, "query": "..", "files": {"total": 3, "matches": [<file objects>], "pagination": {..}, "paging": {..}}}`. OBSERVED: https://docs.slack.dev/reference/methods/search.files, emacs-slack/emacs-slack/slack-search.el:185-196
- Pagination: as search.messages. OBSERVED: https://docs.slack.dev/reference/methods/search.files
- Used for: fallback or bulk export. OBSERVED: rusq/slackdump/cmd/slackdump/internal/archive/assets/search.md:8-13

### search.all

- Transport: as search.messages. OBSERVED: https://docs.slack.dev/reference/methods/search.all
- Visibility: public (legacy). OBSERVED: https://docs.slack.dev/reference/methods/search.all
- Request params: `query`, `count`, `page`, `highlight`, `sort`, `sort_dir`, `team_id`. OBSERVED: https://docs.slack.dev/reference/methods/search.all, beeper/slackgo/search.go:92-117
- Response: `{"ok": true, "query": "..", "messages": {..as search.messages..}, "files": {..as search.files..}, "posts": {"matches": [], "total": 0}}`. OBSERVED: https://docs.slack.dev/reference/methods/search.all, beeper/slackgo/search.go:75-80
- Pagination: one page of each type; use search.messages or search.files for more. OBSERVED: https://docs.slack.dev/reference/methods/search.all
- Used for: nothing in the web client; slack-go's `Search()`. OBSERVED: beeper/slackgo/search.go:129-139

### edgeapi users/search

- Transport: `POST https://edgeapi.slack.com/cache/<enterprise_id or team_id>/users/search`, JSON body. The token goes either in the body as `"token"` (slack-go, slackdump) or as an `Authorization: Bearer` header (wee-slack), always with the `d` cookie. OBSERVED: ankarhem/slackless/.omo/research/gist.md:109-111, wee-slack/wee-slack/slack/slack_api.py:66-84, beeper/slackgo/slack.go:278-279, beeper/slackgo/search.go:172-192, rusq/slackdump/internal/edge/edge.go:166-168, rusq/slackdump/internal/edge/edge.go:208-216. clack puts the same token in the body twice, as `token` and `enterprise_token`, appends `?_x_app_name=client` and sends the JSON as `text/plain;charset=UTF-8`; karbassi lists `enterprise_token` as required on Enterprise Grid. OBSERVED: some-du6e/clack/internal/slack/client.go:583-601, karbassi/slack-mcp/docs/undocumented-endpoints.md:81
- Visibility: private (wee-slack enables it only for session tokens). OBSERVED: wee-slack/wee-slack/slack/slack_api.py:77
- Request params (switcher popover):
  - `query` - string - yes; `count` - int - 30; `fuzz` - int - 1; `include_profile_only_users` - bool - true; `enable_workspace_ranking` - bool - true. OBSERVED: some-du6e/clack/internal/slack/more.go:429-432, some-du6e/clack/docs/SEARCH.md:16
  - `search_email` - bool - true in the switcher. OBSERVED: some-du6e/clack/internal/slack/more.go:433-435, beeper/slackgo/search.go:191
  - `top_users` - array of user ids - the frecency list; `default_workspace` - team id. OBSERVED: some-du6e/clack/internal/slack/more.go:408-447
  - `current_channel` - id - composer @-autocomplete only. OBSERVED: some-du6e/clack/internal/slack/more.go:416-418, some-du6e/clack/internal/slack/more.go:436-438
  - Other clients send `filter="NOT deactivated"` and `uax29_tokenizer=false` (count 24 or 25). OBSERVED: wee-slack/wee-slack/slack/slack_api.py:104-111, beeper/slackgo/search.go:185-194
- Response: `{"ok": true, "results": [<user objects as users.info, incl. "deleted">], "presence_active_ids": ["U.."]}`. OBSERVED: wee-slack/wee-slack/typings/slack_edgeapi/slack_users_search.pyi:7-10, beeper/slackgo/search.go:176-179, some-du6e/clack/internal/core/search.go:912-918
- Pagination: none; `count` only. OBSERVED: some-du6e/clack/internal/slack/more.go:421-447
- Used for: the Cmd+K switcher's people, @-mention autocomplete, wee-slack's `/slack search users`. OBSERVED: ankarhem/slackless/.omo/research/gist.md:624, wee-slack/wee-slack/slack/completions.py:265, wee-slack/wee-slack/slack/slack_search_buffer.py:209-210

### edgeapi channels/search

- Transport: `POST https://edgeapi.slack.com/cache/<enterprise_id or team_id>/channels/search`, JSON body (auth as users/search). OBSERVED: wee-slack/wee-slack/slack/slack_api.py:79-84, wee-slack/wee-slack/slack/slack_api.py:117-127
- Visibility: private. OBSERVED: ankarhem/slackless/.omo/research/gist.md:493-495
- Request params (switcher popover):
  - `query` - string - yes; `count` - int - 30; `fuzz` - int - 1; `filter` - string - `xws`; `include_record_channels` - bool - true; `check_membership` - bool - true. OBSERVED: some-du6e/clack/internal/slack/more.go:458-471, some-du6e/clack/docs/SEARCH.md:17
  - `top_channels` - array of channel ids - the frecency list; `default_workspace` - team id. OBSERVED: some-du6e/clack/internal/slack/more.go:472-477
  - Variants: wee-slack count 25 with `uax29_tokenizer=false`; slock count 40 with `include_record_channels=false`. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:119-126, ItsFelix5/slock/packages/plugins/src/search/server/index.ts:91-103
- Response: `{"ok": true, "results": [{"id": "C..", "name": "..", "is_private": false, "is_archived": false, "is_member": true, "is_channel": true, "is_general": false, "is_mpim": false, "num_members": 0, "member_count": 0, "created": 0, "creator": "U..", "topic": {}, "purpose": {}}], "member_channels": ["C.."]}`. OBSERVED: some-du6e/clack/internal/slack/more.go:482-498, wee-slack/wee-slack/typings/slack_edgeapi/slack_channels_search.pyi:7-10
- Pagination: none; `count` only. OBSERVED: some-du6e/clack/internal/slack/more.go:458-480
- Used for: the Cmd+K switcher's channels, wee-slack's `/slack search channels` and `/slack join`. OBSERVED: ankarhem/slackless/.omo/research/gist.md:624, wee-slack/wee-slack/slack/slack_search_buffer.py:196-203, wee-slack/wee-slack/slack/commands.py:408-409

### users.prefs.get / users.prefs.set (search-owned prefs only)

- Transport: shared transport (prefs area owns the methods; on Grid it must use the enterprise host). OBSERVED: some-du6e/clack/internal/slack/client.go:123
- Visibility: private (session-token prefs API)
- Request params:
  - `users.prefs.get` `prefs=frecency` (or `emoji_use,frecency`) - named request; the client never trusts the `frecency` value inside a full prefs map. OBSERVED: some-du6e/clack/internal/core/frecency.go:162-200
  - `users.prefs.set` `name=frecency`, `value=<JSON object>` - written at most once a minute. OBSERVED: some-du6e/clack/internal/core/frecency.go:18-30, some-du6e/clack/internal/core/frecency.go:279-295
  - `users.prefs.get` `prefs=recents`; `users.prefs.set` `name=recents`, `value=<JSON>` - only with experiments `desktop_search_recents_read` / `desktop_search_recents_write`, written at most once a minute. OBSERVED: some-du6e/clack/internal/core/search_recents.go:6-23, some-du6e/clack/internal/core/search_recents.go:438-454, some-du6e/clack/docs/SEARCH.md:22-23
- Response / pref formats:
  - `frecency`: `{"E1f44d": {"id": "E1f44d", "count": 3, "visits": [<ms>, ...]}, "<normalized query>": [{"id": "C..", "count": 2, "visits": [...]}]}`; at most 10 visits per record; `INF_SEARCH` aggregates search counts. OBSERVED: some-du6e/clack/internal/frecency/frecency.go:1-22, some-du6e/clack/internal/frecency/frecency.go:37-49
  - Grid: the account's frecency is stored as `frecency_ent_jumper`; a full prefs map shows `frecency: ""`, but a named `prefs=frecency` request returns it. Other keys seen: `frecency_jumper`, `frecency_ent_jumper_backup`. OBSERVED: some-du6e/clack/internal/core/frecency.go:173-181, beeper/slackgo/info.go:177-179, ItsFelix5/slock/packages/plugins/src/slack/client/api/preferences.ts:57-61
  - `recents`: `{"navigation": [{"id": "C..", "object_type": "CHANNEL|DM|THREAD|FILE|USER|PAGE|...", "timestamp": <ms>, "thread_ts": ".."}], "search": [{"query": "..", "type": "..", "timestamp": <ms>, "thread_ts": ".."}]}`, capped at 40 navigation events (at most 10 threads) and 100 searches, newest first. OBSERVED: some-du6e/clack/internal/core/search_recents.go:9-12, some-du6e/clack/internal/core/search_recents.go:49-60, some-du6e/clack/internal/core/search_recents.go:93-104
  - Search filter prefs (read from boot prefs, sent as request params): `search_sort` (`not_set`), `search_file_sort` (`score`), `search_channel_sort` (`relevant`), `search_people_sort` (`relevant`), `search_only_my_channels`, `search_only_current_team`, `search_hide_my_channels`, `search_only_show_online`, `search_hide_deactivated_users`, `search_exclude_channels` (comma-separated ids), `search_exclude_bots`. OBSERVED: rusq/slackdump/internal/edge/assets/client.userBoot.json:1003-1026, rusq/slackdump/internal/edge/assets/client.userBoot.json:1149, beeper/slackgo/info.go:31-53, some-du6e/clack/internal/core/search.go:327-336
  - No observed source reads or writes a `recent_searches` pref; super-platinum uses that name only for its own local cache. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/cache.rs:46
- Pagination: n/a. OBSERVED: some-du6e/clack/internal/core/search_recents.go:438-454
- Used for: switcher ranking (`top_users` / `top_channels`), Recent searches and Recent places. Cross-area: `visits.list` (`limit=40`, `checksum`) and `visits.record` (`visits=[{entity_id, entity_type, thread_ts, client_event_time_ms}]`, `client_now_ms`) feed Recent places. OBSERVED: some-du6e/clack/docs/SEARCH.md:22-23, some-du6e/clack/internal/core/search_recents.go:456-462, some-du6e/clack/internal/core/search.go:897-935

## Websocket events

- **`search_frecency`** - payload `{"type": "search_frecency", "value": "updated"}`. Fires when the frecency prefs change server-side (for example after another client saved). Only `value == "updated"` should trigger a refetch: `users.prefs.get prefs=emoji_use,frecency`. OBSERVED: some-du6e/clack/internal/core/events.go:35-41, IzieStratt/personal-site/website/public/__subdomains/vs/redocumented/docs/methods/gateway-events-2026-10.md:2559-2569, some-du6e/clack/internal/core/frecency.go:297-300
- **`search_recents`** - payload `{"type": "search_recents", "value": "updated"}`. Fires when the `recents` pref changes server-side. Refetch `users.prefs.get prefs=recents`, gated by the recents-read feature check. OBSERVED: some-du6e/clack/internal/core/events.go:42-48, IzieStratt/personal-site/website/public/__subdomains/vs/redocumented/docs/methods/gateway-events-2026-10.md:2571-2582

## Gotchas

- Enterprise Grid host routing: `search.modules.{messages,dms,files,channels,people}` and `search.inline` answer `team_is_restricted` on the team host and must be sent to `https://<org>.enterprise.slack.com/api/` with the org-level xoxc. clack learns at runtime: `team_is_restricted` moves a method to the enterprise host, `enterprise_is_restricted` moves it back. OBSERVED: some-du6e/clack/internal/slack/client.go:111-130, IzieStratt/personal-site/website/public/__subdomains/vs/redocumented/docs/methods/search.md:13. Public `search.*` need `team_id` with an org-level token. OBSERVED: https://docs.slack.dev/reference/methods/search.messages. The Edge API path uses the enterprise id on Grid and the team id otherwise. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:80, ankarhem/slackless/.omo/research/gist.md:116-123
- Grid frecency trap: reading `frecency` from a full prefs blob gives `""` on Grid, so request it by name, or read `frecency_ent_jumper` → `frecency_jumper` → `frecency` as slock does. slack-go leaves `frecency_jumper` untyped (commented out). OBSERVED: some-du6e/clack/internal/core/frecency.go:173-191, ItsFelix5/slock/packages/plugins/src/slack/client/api/preferences.ts:57-61, beeper/slackgo/info.go:177
- Page cap: public search methods cap `count` and `page` at 100. OBSERVED: https://docs.slack.dev/reference/methods/search.messages. On `search.modules.channels`, `page` > 100 silently returns page 1, so a page-number loop never ends; use `cursor`. OBSERVED: rusq/slackdump/internal/edge/edge.go:456-463
- Fixture shows `per_page: 50` although slackdump asks for `count=100` on `search.modules.channels`, so treat `count` as a request and read `pagination` back. OBSERVED: rusq/slackdump/internal/edge/assets/search.module.channels.json:6-12, rusq/slackdump/internal/edge/search.go:31
- Rate limits: public search methods are Tier 2 (20+/min). OBSERVED: https://docs.slack.dev/reference/methods/search.messages. No published tier exists for `search.modules.*`. slackdump throttles `search.modules.channels` to 1 per 300 ms with burst 5, and its search diag tool to 1 per 3 s with burst 5. OBSERVED: rusq/slackdump/internal/edge/limits.go:38, rusq/slackdump/internal/edge/search.go:165, rusq/slackdump/cmd/slackdump/internal/diag/search.go:97. Limits count per method per minute, and one search fans out to about 7 calls, so debounce typeahead. OBSERVED: some-du6e/clack/internal/slack/client.go:141-144, ankarhem/slackless/.omo/research/gist.md:481
- Shape mismatch: public `search.messages` returns flat `messages.matches`; `search.modules.messages` groups hits per channel (`items[].messages[]`); `search.modules.dms` uses a singular `items[].message`; `search.inline` is flat with `channel_id` and `extracts`. Normalize in one decoder. OBSERVED: https://docs.slack.dev/reference/methods/search.messages, myurasov/Slackwright/src/slackwright/search.py:389-401, ImShyMike/slack-undoc-client/src/types.ts:19839-19849, some-du6e/clack/internal/core/search.go:820-831
- Deprecated fields: `previous`, `previous_2`, `next`, `next_2` on public matches stopped being sent on 2020-12-03; slack-go still declares them, so expect zero values. OBSERVED: https://docs.slack.dev/reference/methods/search.messages, beeper/slackgo/search.go:55-58
- Server-side filtering: for user tokens, results already honor the UI's search filters (e.g. `search_exclude_channels`), and several matches close together collapse into one. OBSERVED: https://docs.slack.dev/reference/methods/search.messages, some-du6e/clack/internal/core/search.go:309-312
- Empty windows may come back as errors rather than empty lists: Slackwright treats `no_results`, `no_search_results` and `empty` as a soft empty. OBSERVED: myurasov/Slackwright/src/slackwright/search.py:384-388
- `search.save` parameter conflict: clack (citing the web client's `searchSave`), slack-undoc-client and karbassi use `{terms, type}`; slock sends `{module: "messages", query}` but swallows the error. Use `terms`/`type`. OBSERVED: some-du6e/clack/internal/core/search.go:884-895, ItsFelix5/slock/packages/plugins/src/search/server/index.ts:210-218
- Experiments change the endpoint set: `desktop_search_recents_read/write` switch Recent searches between `search.autocomplete` and the `recents` pref plus `visits.*`; `omni_quick_message_results` changes `search.inline` scoping; `search_explore_freshness_signals` adds `channel_type=exclude_archived`. Read them from `experiments.getByUser`. OBSERVED: some-du6e/clack/internal/core/search_recents.go:6-26, some-du6e/clack/docs/SEARCH.md:18, some-du6e/clack/docs/SEARCH.md:24, ankarhem/slackless/.omo/research/gist.md:215-237
- Prior art is thin: mautrix-slack calls no `search.*` method (its only search is slack-go's Edge `users/search`, for starting chats); slack-libpurple only forwards `/search` as a server-side slash command via `chat.command`; emacs-slack uses only the public `search.messages`/`search.files`; slack-go adds Edge `users/search` to the public methods. Most private details here come from clack, slack-undoc-client, slackdump, wee-slack, slock and the local teardown, so re-capture from the live client before relying on rarely used params. OBSERVED: dylex/slack-libpurple/slack-cmd.c:37, dylex/slack-libpurple/slack-cmd.c:80, emacs-slack/emacs-slack/slack-search.el:216-220, mautrix/slack/pkg/connector/startchat.go:187-191, beeper/slackgo/search.go:181-194
- slack-undoc-client types are generated from responses and may be incomplete for other inputs; treat its optional fields as "seen at least once". OBSERVED: ImShyMike/slack-undoc-client/README.md:5-6
- Other `search.*` methods exist but sit outside core search parity: `search.autocomplete.triggers` and `search.modules.triggers` (workflow triggers, owned by the workflows area), `search.autocomplete.topEngagedFiles`, `search.enterprise` (enterprise team picker). OBSERVED: ImShyMike/slack-undoc-client/src/types.ts:25039-25056, some-du6e/clack/internal/core/workflows.go:534, 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/workflow_management/settings_acquisition.rs:128
- Cross-area: `users.priority.list` (ranked contacts for @-mention autocomplete) belongs to the users area. OBSERVED: ankarhem/slackless/.omo/research/gist.md:175
- Evidence revisions (line numbers refer to these commits): some-du6e/clack 6548bfbe, ImShyMike/slack-undoc-client 868fd4d7, rusq/slackdump fec2e68b, rusq/slack 8f5f9f44, beeper/slackgo d0ae74c3 (the slack-go fork mautrix-slack 38d69d5d pins in go.mod:48), wee-slack/wee-slack aa904c24, emacs-slack/emacs-slack eafa2d48, dylex/slack-libpurple 1115ec11, ItsFelix5/slock abb335d3, myurasov/Slackwright d747aebe, karbassi/slack-mcp 7abc8af0, IzieStratt/personal-site d25b38d8, rubencodes/EmojiWrapped 82b816cc, 3kh0/super-platinum 58e74550. OBSERVED: mautrix/slack/go.mod:48

## LEADS

- `search.modules.messages` probably accepts `cursor`/`next_cursor` like `search.modules.channels`; only page numbers were observed for messages.
- The real per-call maximum `count` for `search.modules.*` is unknown (50 is suggested by the channels fixture); the rate tier for `search.modules.*` is likely similar to Tier 2 or 3 but unpublished.
- `search_only_team` and `external_shared_team_ids` (sent empty) probably scope results to one Grid workspace or connected org; `search.precache`'s `enterprise_token` presumably carries the org token on Grid.
- `to:` presumably finds messages sent to a user (DMs/mentions); its exact semantics were not observed. A bare `has:reaction` (without an emoji name) was not observed; only `has::name:` and `hasmy::name:` were.
- `search.autocomplete` with a non-empty `query` likely fills `query_suggestions` (typeahead completions); only the empty-query history use was observed.
- The socket message that opens the `search.feedback` dialog (keyed by `client_token`) is probably a standard dialog-open event; its type was not captured.
- Edge `mpims/search` and `usergroups/search` exist on the same host (super-platinum allowlists them) and may back switcher group-DM rows and @-group mentions: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/directory_endpoint.rs:103.
- `search.saved.create`, `search.saved.delete`, `search.saved.list` and `search.modules.{ai,external,topResults,workObjects}` are listed as existence-only in a third-party index (IzieStratt/personal-site/.../docs/methods/INDEX.md:1568-1582); they probably back saved searches and AI/external-source tabs.
- `client.userBoot`'s `channels_priority` map (sidebar relevance scores) may also feed switcher ranking; only sidebar use was observed (ankarhem/slackless/.omo/research/gist.md:203-209).
- On non-Grid workspaces the plain `frecency` pref is presumably the store, with `frecency_jumper` a legacy key; this was only observed via slock's fallback order.
