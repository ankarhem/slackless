# Emoji, slash commands, apps, Block Kit and workflows - Slack web-client API

## Summary

- Scope: custom and standard emoji plus emoji frecency; slash commands (catalog, execution, built-ins); global and message shortcuts; Block Kit rendering and every interaction path (message blocks, blocks inside legacy attachments, modals, App Home); legacy attachment buttons and dialogs; app invite consent; Workflow Builder link triggers.
- Public: `emoji.list` is the only documented method a user client calls here. The app-side docs (Block Kit, views, workflow button) describe the same objects, but not the endpoints the client calls.
- Private: every call that acts on an app: `client.appCommands`, `chat.command`, `blocks.actions`, `blocks.suggestions`, `views.get/submit/close`, `apps.actions.v2.execute`, `apps.home.*`, `chat.attachmentAction`, `workflows.triggers.*`, `functions.workflows.*`.
- Modals are pushed by the server. The client never opens a view itself: it sends a request carrying a `client_token`, and the websocket then delivers `view_opened`/`view_updated` with the whole view and that same `client_token`.
- Biggest risks: (1) the modal flow is known from one non-Grid HAR set, and the close, `update` and `errors` paths were not captured; (2) Block Kit keeps adding block types, so the renderer needs an unknown-block fallback; (3) `_x_reason` tags differ between captures, and Grid anomaly detection reportedly flags unfamiliar request shapes.

## Endpoints

**Common transport** (applies to every method below unless its Transport line says otherwise): `POST https://<workspace>.slack.com/api/<method>`; on Enterprise Grid the host is `<org>.enterprise.slack.com`, and the URL carries the web client's `_x_*` query envelope (`_x_id`, `_x_csid`, `slack_route`, `_x_version_ts`, …). The web client sends `multipart/form-data`; `application/x-www-form-urlencoded` is accepted too. Auth is `token=xoxc-…` in the body plus the `Cookie: d=xoxd-…` header. Every web-client call ends with the envelope `_x_reason=<caller tag>`, `_x_mode=online`, `_x_sonic=true`, `_x_app_name=client`. Object-valued params (`actions`, `container`, `state`, `context`, `blocks`, `payload`) travel as JSON strings inside form fields. Failures look like `{"ok":false,"error":"<code>"}`. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:106-116; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_channel.json:3-47; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_unknown.json:45-48; gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2192-2201; slackless/.omo/research/gist.md:77-93; 3kh0/super-platinum/crates/super-platinum-core/src/slack/client.rs:58-61, 163-178

### emoji.list

- **Transport:** common form POST.
- **Visibility:** public.
- **Request params:**
  - `include_categories` - boolean - optional - also return the Unicode emoji categories and the names in each. OBSERVED: https://docs.slack.dev/reference/methods/emoji.list
  - super-platinum and wee-slack send no params at all. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/slack/api/emoji_catalog.rs:33; 3kh0/super-platinum/crates/super-platinum-core/src/slack/api/execute.rs:422-423; wee-slack/wee-slack/slack/slack_api.py:388-393
- **Response:**
  ```json
  {
    "ok": true,
    "emoji": { "shipit": "alias:squirrel", "<name>": "<image URL>" }
  }
  ```
  Every value is a string: an image URL, or `alias:<target>`. The target can be another custom emoji, a standard emoji name, or a name that no longer exists. OBSERVED: https://docs.slack.dev/reference/methods/emoji.list; 3kh0/super-platinum/crates/super-platinum-core/src/slack/api/emoji_catalog.rs:45-58; mautrix/slack/pkg/connector/emoji.go:143-147, 274-300; emacs-slack/emacs-slack/slack-emoji.el:123-135
- **Pagination:** none; the whole map comes in one response. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/slack/api/emoji_catalog.rs:51-58
- **Used for:** custom emoji in messages, reactions, the picker and `:` autocomplete; refetch after `emoji_changed`. Rate limit Tier 2 (20+/min). OBSERVED: https://docs.slack.dev/reference/methods/emoji.list; 3kh0/super-platinum/crates/super-platinum-core/src/composer_emoji/changes.rs:1-3; beeper/slackgo/emoji.go:20-30

#### Standard emoji set (no endpoint)

- Standard emoji are not in `emoji.list`; the client ships them. Slack's own catalog is an ordered list of `{"name":"100","unicode":"1f4af"}` entries (two bundled catalog versions, v16 and v17), plus an English keyword map used for search (`"grinning": ["cheerful", "cheery", …]`). OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/composer_emoji/standard.rs:1-2, 53-54, 62; 3kh0/super-platinum/crates/super-platinum-core/src/composer_emoji/slack-emoji-v17-ordered.json:1; 3kh0/super-platinum/crates/super-platinum-core/src/composer_emoji/slack-emoji-keywords-en-US.json:1
- Super-platinum's tests treat Slack's alias groups and English keywords as source data, distinct from gemoji. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/composer_emoji/standard.rs:158-165
- Rust-side source of names, shortcodes and skin tones: iamcal's `emoji.json`, published on npm as `emoji-datasource`. Per-emoji fields: `unified`, `non_qualified`, `image`, `short_names[]`, `category`, `subcategory`, `sort_order`, `has_img_apple/google/twitter/facebook`, `skin_variations` (keyed by tone codepoint, each with its own `unified` and `image`), `obsoleted_by`. OBSERVED: iamcal/emoji-data/README.md:19, 44-85, 97-110
- mautrix-slack generates its table from iamcal `emoji.json` plus Unicode's `emoji-variation-sequences.txt`, which decides when to append U+FE0F. emacs-slack downloads the same iamcal file at runtime. OBSERVED: mautrix/slack/pkg/emoji/emoji-generate.go:108, 130; emacs-slack/emacs-slack/slack-emoji.el:38-43
- Skin tones are spelled `<name>::skin-tone-<2..6>` in names (reactions, frecency). Rich-text `emoji` elements carry `name`, `unicode`, and optionally `skin_tone`. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-27-emoji-frecency-design.md:39-42; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/ws_remind_ephemeral.json:14-18; wee-slack/wee-slack/slack/slack_message.py:1282

### emoji.adminList

- **Transport:** common form POST, with `_x_reason=customize-emoji-new-query`. OBSERVED: rusq/slackdump/internal/edge/emoji.go:74-95; rusq/slackdump/internal/edge/edge.go:228-232, 433-447
- **Visibility:** private.
- **Request params:**
  - `page` - int - required - 1-based page number. OBSERVED: rusq/slackdump/internal/edge/emoji.go:57-62, 90-93
  - `count` - int - required - page size; slackdump uses 100. OBSERVED: rusq/slackdump/internal/edge/emoji.go:90-93
- **Response:** `{"ok":true,"emoji":[…],"disabled_emoji":[…],"custom_emoji_total_count":<int>,"paging":{"count","total","page","pages"}}`. Each emoji has `name`, `is_alias` (0/1), `alias_for`, `url`, `team_id`, `user_id`, `created`, `is_bad`, `user_display_name`, `avatar_hash`, `can_delete`, `synonyms[]`. OBSERVED: rusq/slackdump/internal/edge/emoji.go:24-62
- **Pagination:** page-based; stop when `paging.page >= paging.pages`. OBSERVED: rusq/slackdump/internal/edge/emoji.go:64-72, 115-118
- **Used for:** the "customize emoji" admin list (creator, creation date, alias target, disabled emoji). Rendering does not need it. OBSERVED: rusq/slackdump/internal/edge/emoji.go:94

### users.prefs.get (prefs=emoji_use,frecency)

General `users.prefs.get`/`users.prefs.set` are documented in api-users.md; this subsection covers only the emoji subset.

- **Transport:** common form POST; the cold-boot capture used `multipart/form-data` and `_x_reason=fetch-frecency-prefs`. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2188-2201
- **Visibility:** private (undocumented method).
- **Request params:**
  - `prefs` - comma-separated pref names - optional - the web client asks for `emoji_use,frecency` at boot. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2197
- **Response:**
  ```json
  {
    "ok": true,
    "prefs": {
      "emoji_use": "<JSON string, 514 chars>",
      "frecency": "<JSON string, 7355 chars>"
    }
  }
  ```
  OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2205-2209, 2251-2252
- **`frecency` format:** a JSON object encoded as a string, whose key order matters (it breaks score ties). Keys are normalized query text (a leading `:`, `@`, `#`, `/` or `+`, or the full-width variant, is stripped) and item ids. Each value is one record or an array of records `{"id":"…","count":N,"visits":[<epoch ms>, …]}`; `visits` keeps the last 10, and an optional `_reduced` flag halves the weight. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/composer_emoji/history.rs:1-60, 292-300, 346-348, 380-392; 3kh0/super-platinum/crates/super-platinum-core/src/composer_emoji/history_tests.rs:245
- **Scoring:** each visit is weighted by age (<4 h 100, <1 d 80, <3 d 60, <7 d 40, <30 d 20, <90 d 10, older 0), and the weights are averaged over visits. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/composer_emoji/history.rs:326-350
- **Related prefs:** `emoji_use` (string; no source decodes it), `emoji_mode` (`default`, or `as_text` to show emoji as text), `preferred_skin_tone` (`""` or 2-6), `show_all_skin_tones`. OBSERVED: beeper/slackgo/info.go:47-48, 174-175; 3kh0/super-platinum/crates/super-platinum-core/src/composer_emoji/preferences.rs:7, 29-34, 64-70; rusq/slackdump/internal/edge/assets/client.userBoot.json:1019, 1169
- **Pagination:** none.
- **Used for:** the "Frequently used" row of the picker, ranking in `:` autocomplete, and reaction suggestions. Live changes arrive as `pref_change`. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/composer_emoji/preferences.rs:53-62

### search.autocomplete.topEmojis

- **Transport:** common form POST (seen only in a boot-sequence list).
- **Visibility:** private.
- **Request params:** not captured.
- **Response:** not captured. The message hover toolbar's three "React with …" buttons are the user's most-used reactions, and they come from this call.
- **Pagination:** n/a.
- **Used for:** quick reactions on hover; fired in boot phase 2. OBSERVED: slackless/.omo/research/gist.md:172, 614

### edgeapi:emojis/info

- **Transport:** `POST https://edgeapi.slack.com/cache/<enterprise or team id>/emojis/info`, JSON body (Flannel edge cache). OBSERVED: slackless/.omo/research/gist.md:111, 123, 174; rusq/slackdump/internal/edge/edge.go:91
- **Visibility:** private.
- **Request params:** `updated_ids` - object id→timestamp - Flannel's conditional-fetch map; only entities that changed come back. Other fields not captured. OBSERVED: slackless/.omo/research/gist.md:143, 580
- **Response:** not captured.
- **Pagination:** conditional fetch instead of paging. OBSERVED: slackless/.omo/research/gist.md:143
- **Used for:** incremental custom-emoji metadata. Super-platinum treats it as partial and keeps `emoji.list` as the complete source. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/slack/api/emoji_catalog.rs:1-2; 3kh0/super-platinum/crates/super-platinum-core/src/composer_emoji/changes.rs:1-3

### client.appCommands

- **Transport:** common form POST.
- **Visibility:** private.
- **Request params:** none beyond the envelope. `_x_reason` was `dispatch-client-extras` in the boot capture and `app-commands-conditional-fetching` in super-platinum. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/client_app_commands.json:1-24; 3kh0/super-platinum/crates/super-platinum-core/src/slash_commands/api.rs:76-82; 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/shortcuts.rs:184-190
- **Response:**
  ```json
  {
    "ok": true,
    "commands": [
      {
        "name": "/shrug",
        "canonical_name": "/shrug",
        "name_normalized": "/shrug",
        "type": "core",
        "desc": "Appends ¯\\_(ツ)_/¯ to your message",
        "usage": "your text",
        "is_workspace_agnostic": true,
        "teams": ["T0EXAMPLE01"]
      },
      { "alias_of": "/msg", "name": "/dm", "type": "core" },
      {
        "name": "/linear",
        "type": "app",
        "app": "AEMQ3Q4F4",
        "app_name": "Linear",
        "desc": "Create new issue on Linear",
        "usage": "issue title (optional) ",
        "icons": {
          "image_32": "…",
          "image_48": "…",
          "image_64": "…",
          "image_72": "…"
        },
        "teams": ["T0EXAMPLE01"]
      },
      {
        "name": "/giphy",
        "type": "service",
        "service_name": "Giphy",
        "desc": "Post a random gif to channel",
        "usage": "[text]",
        "is_workspace_agnostic": true,
        "teams": ["T0EXAMPLE01"]
      }
    ],
    "cache_ts": "1786000104.056000",
    "app_actions": [
      {
        "app_id": "A…",
        "app_name": "…",
        "teams": ["T…"],
        "is_workspace_agnostic": false,
        "actions": [
          {
            "type": "global_action",
            "action_id": "…",
            "name": "…",
            "description": "…"
          }
        ]
      }
    ]
  }
  ```
  `commands` and `cache_ts`: OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/client_app_commands.json:25-39, 209-214, 320-341, 439-441, 448-449. `app_actions` is redacted in the capture; its shape comes from super-platinum's decoder. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/shortcuts.rs:52-115
- **Field notes:** a command's `type` is `core`, `app` or `service`. `app` commands carry `app` (the app id) and `app_name`; `service` commands are legacy integrations such as Giphy and carry `service_name`. A shortcut's `type` is `global_action` (lightning-bolt menu) or `message_action` (a message's "More actions" menu); the decoder skips other types. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-design.md:104-109; 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/shortcuts.rs:87-91
- **Pagination:** none. The web client caches the list in local storage; the capture only saw this call at boot after local storage was cleared. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-design.md:109-111; gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:76
- **Used for:** `/` autocomplete (name, usage, desc, app icon), routing a typed command, and both shortcut menus. Refetch on `commands_changed`. OBSERVED: slackless/.omo/research/findings.md:54; ItsFelix5/slock/packages/plugins/src/composer/client/index.tsx:22

### commands.list

- **Transport:** seen only in emacs-slack: `POST https://slack.com/api/commands.list` with no params and no body; the token goes in `Authorization: Bearer` plus `Cookie: d=`. The common transport is unverified for this method (see LEADS). OBSERVED: emacs-slack/emacs-slack/slack-slash-commands.el:35, 89-108; emacs-slack/emacs-slack/slack-request.el:201-240
- **Visibility:** private; an older method (emacs-slack file dated 2017). The 2026 web-client captures use `client.appCommands` instead. OBSERVED: emacs-slack/emacs-slack/slack-slash-commands.el:3
- **Request params:** none.
- **Response:** `commands[]`, each with `name`, `type` (`core`|`app`|`service`), `usage`, `desc`, `alias_of`, plus `canonical_name` (core), `app` (app) or `service_name` (service). OBSERVED: emacs-slack/emacs-slack/slack-slash-commands.el:37-53, 65-87
- **Pagination:** none.
- **Used for:** a fallback command catalog only.

### chat.command

- **Transport:** common form POST, `_x_reason=executeCommand`. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_channel.json:32-35
- **Visibility:** private.
- **Request params** (in web-client capture order):
  - `command` - string - required - the command with its slash, e.g. `/giphy`. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_channel.json:8-11
  - `disp` - string - required by the web client - the command as displayed; equal to `command` in every capture. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_channel.json:12-15
  - `blocks` - JSON string - optional - the arguments as a `rich_text` array, e.g. `[{"type":"rich_text","elements":[{"type":"rich_text_section","elements":[{"type":"text","text":"gorilla"}]}]}]`; omitted when there are no arguments. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_channel.json:16-19; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-design.md:78
  - `channel` - conversation id - required. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_channel.json:20-23
  - `client_token` - string - required by the web client - `web-<unix ms>`; echoed back in `view_opened` if the command opens a modal. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_channel.json:24-27; gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:175-177
  - `thread_ts` - ts - optional - run the command in a thread. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_thread.json:28-31
  - `team_id` - team id - sent for known commands; absent in the unknown-command capture. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_channel.json:28-31; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_unknown.json:3-44
  - `app_id` - app id - sent for `type: app` commands. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/slash_commands/api.rs:98-113
  - `text` - string - older clients send the arguments as plain text instead of `blocks`. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:464-482; dylex/slack-libpurple/slack-cmd.c:79-80; emacs-slack/emacs-slack/slack-slash-commands.el:129-163
- **Response:** `{"ok":true,"keep_input":false}`; an unknown command gives `{"ok":false,"error":"unknown_command"}`. Older responses could carry `response`, text the client shows as a Slackbot ephemeral. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_channel.json:49-52; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_unknown.json:45-48; emacs-slack/emacs-slack/slack-slash-commands.el:142
- **Pagination:** none.
- **Used for:** running every server-side command. The result arrives on the websocket, not in the response: a posted message, a `me_message`, a Slackbot ephemeral, or a `view_opened`. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-design.md:94-102; gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:86-98

#### Built-in (core) commands

- The core commands in a 2026-10 capture: `/shrug /me /topic /away /msg /open /leave /who /archive /feedback /invite /mute /remind /feed /status /dm /join /close /part /remove /kick /dnd /apps /active`. Aliases: `/dm`→`/msg`, `/join`→`/open`, `/close` and `/part`→`/leave`, `/kick`→`/remove`. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/client_app_commands.json:33-310; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-design.md:107-109
- `/shrug hi`, `/me waves`, `/remind me test in 1 minute`, `/giphy` and app commands: the web client calls `chat.command`. The results are a posted message, a `me_message`, and a Slackbot ephemeral plus `saved_added`. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_shrug.json:8-19; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_me.json:8-19; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_remind.json:8-19; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-design.md:96-98
- `/away`: toggles via `presence.set presence=away|active` (presence area), with no `chat.command`. `/status away` also sends `presence.set away`. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/presence_set_away.json:1-27; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-design.md:99-100
- `/msg @user text`: the client resolves the user, then calls `chat.postMessage` to the DM (messaging area). `/open #channel`: client-side navigation only. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-design.md:101-102
- `/topic /invite /leave /mute /dnd /feed /who /archive /remove /apps /active /feedback`: listed as `core`, but how the web client dispatches them was not captured (see LEADS). OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/client_app_commands.json:57, 105, 117, 129, 141, 153, 165, 189, 262, 286, 298, 310
- `/collapse` and `/expand` are documented in the help center but missing from the 2026-10 core list. OBSERVED: https://slack.com/help/articles/201259356-Use-built-in-slash-commands; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/client_app_commands.json:33-310

### apps.actions.v2.execute

- **Transport:** common form POST, `_x_reason=app-global-actions-api/execute-app-action`.
- **Visibility:** private.
- **Request params:**
  - `action_id` - string - required - `app_actions[].actions[].action_id` from `client.appCommands`.
  - `app_id` - string - required - `app_actions[].app_id`.
  - `context` - JSON string - required - `{"channel_id":"C…","message_ts":"…"}`; a message shortcut sends both keys, a global shortcut never sends `message_ts`.
  - `client_token` - string - required - ties the run to the `view_opened` it causes.
  - `team_id` - team id - global shortcuts only, required when the action lists `teams` (Grid); never sent for message shortcuts.
  - OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/shortcuts.rs:165-172, 192-239
- **Response:** `ok` is checked, and `app_access_restricted` is handled as its own error. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/shortcuts.rs:249-260
- **Pagination:** none.
- **Used for:** the lightning-bolt (global) shortcut menu and a message's "More actions" shortcuts. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/shortcuts.rs:87-91

### apps.actions.list

- **Transport:** seen only in emacs-slack: `POST https://slack.com/api/apps.actions.list` with no params and no body; the token goes in `Authorization: Bearer` plus `Cookie: d=`. The common transport is unverified for this method (see LEADS). OBSERVED: emacs-slack/emacs-slack/slack-action.el:32-33, 81-93; emacs-slack/emacs-slack/slack-request.el:201-240
- **Visibility:** private; an older method (emacs-slack file dated 2018), replaced by `client.appCommands`'s `app_actions` in 2026 captures. OBSERVED: emacs-slack/emacs-slack/slack-action.el:3
- **Request params:** none.
- **Response:** `{"ok":true,"app_actions":[{"app_id","app_name","actions":[{"name","type","action_id"}]}]}`. OBSERVED: emacs-slack/emacs-slack/slack-action.el:81-111; emacs-slack/emacs-slack/slack-room-buffer.el:334-341
- **Pagination:** none.
- **Used for:** the message actions menu in emacs-slack; fallback only.

### apps.actions.run

- **Transport:** seen only in emacs-slack: `POST https://slack.com/api/apps.actions.run` with the params in the URL query string and the token in `Authorization: Bearer` plus `Cookie: d=`. The common transport is unverified for this method (see LEADS). OBSERVED: emacs-slack/emacs-slack/slack-action.el:34-36, 73-79; emacs-slack/emacs-slack/slack-request.el:201-240
- **Visibility:** private; an older method, pairs with `apps.actions.list`.
- **Request params:** `message_ts`, `channel`, `type`, `action_id`, `app_id`, `client_token` - all strings, all sent. OBSERVED: emacs-slack/emacs-slack/slack-action.el:34-36, 57-79
- **Response:** only `ok` and `error` are checked. OBSERVED: emacs-slack/emacs-slack/slack-action.el:66-72
- **Pagination:** none.
- **Used for:** running a message action; fallback for `apps.actions.v2.execute`.

### blocks.actions

- **Transport:** common form POST, `_x_reason=dispatch_action_to_developer`. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/blocks_actions_ephemeral_send.json:36-39
- **Visibility:** private.
- **Request params:**
  - `service_id` - bot id - required for messages - the bot that posted the message, or the view's `bot_id`. App Home sends it only when the view has a `bot_id`. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/blocks_actions_ephemeral_send.json:8-11; 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/home.rs:313-315
  - `app_id` - app id - required. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/blocks_actions_ephemeral_send.json:12-15
  - `service_team_id` - team id - required - the team that installed the app (`app_installed_team_id` for views). OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/blocks_actions_ephemeral_send.json:16-19; 3kh0/super-platinum/crates/super-platinum-core/src/slack/api/block_suggestions.rs:143-157
  - `actions` - JSON array - required - exactly one action: the element as the payload declared it, plus `block_id` and the user's choice. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/blocks_actions_ephemeral_send.json:20-23; gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:270, 278-282; 3kh0/super-platinum/crates/super-platinum-core/src/slack/api/block_actions.rs:524
  - `container` - JSON - required - one of:
    - `{"type":"message_attachment","message_ts":"…","attachment_id":1,"channel_id":"C…","is_ephemeral":true}` for blocks inside a legacy attachment. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/blocks_actions_ephemeral_send.json:24-27
    - `{"type":"view","view_id":"V…"}` for a modal or the App Home. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:271; 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/home.rs:302-305
    - `{"type":"message","message_ts":"…","channel_id":"C…","is_ephemeral":false}` for top-level message blocks. OBSERVED: emacs-slack/emacs-slack/slack-room-buffer.el:613-619
  - `client_token` - string - required - `web-<unix ms>`. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/blocks_actions_ephemeral_send.json:28-31
  - `state` - JSON - optional - `{"values":{…}}`, the current input values of the surface. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/blocks_actions_ephemeral_send.json:32-35; 3kh0/super-platinum/crates/super-platinum-core/src/slack/api/block_actions.rs:531-533
  - `function_execution_id` - string - optional - set when the surface belongs to a workflow function run. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/slack/api/block_actions.rs:528-530
- **Response:** `{"ok":true}`. The outcome arrives on the websocket: a `view_updated`, or a `message_changed`/`message_deleted` for an ephemeral. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/blocks_actions_ephemeral_send.json:53-55; gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:276; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-design.md:84-85
- **Pagination:** none.
- **Used for:** every Block Kit interaction in messages, attachments, modals and the App Home.
- **Special service ids:** when the message was not posted by a bot, emacs-slack sends `service_id=B01` (Slackbot). Actions on AI-agent approval cards (block ids prefixed `ai_agent_action:`) go to `BSLACKBOT`. OBSERVED: emacs-slack/emacs-slack/slack-room-buffer.el:621-642
- **Older JSON-body variant:** emacs-slack posts `{"actions","service_id","container","client_token"}` as `application/json`, without `app_id`. Prefer the web-client form shape above. OBSERVED: emacs-slack/emacs-slack/slack-block-element.el:548-566

#### Interactive elements and what each action sends

Each action echoes the element (`type`, `action_id`, `block_id`, `text`, `value`, …) and adds the user's choice:

- `button`: nothing extra; `value` and `text` are echoed. Buttons with a `url` open the link (emacs-slack opens the URL and skips the API call). `style` is `primary` or `danger`, and an optional `confirm` dialog runs before dispatch. OBSERVED: emacs-slack/emacs-slack/slack-block-element.el:111-133, 170-178; emacs-slack/emacs-slack/slack-room-buffer.el:650-668
- `static_select`, `external_select`, `overflow`: `selected_option: {"text":{…},"value":"…"}`. OBSERVED: emacs-slack/emacs-slack/slack-room-buffer.el:752-821
- `users_select` → `selected_user`; `conversations_select` → `selected_conversation`; `channels_select` (public channels only) → `selected_channel`. OBSERVED: emacs-slack/emacs-slack/slack-room-buffer.el:670-733
- `multi_conversations_select` → `selected_conversations` (array). OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/slack/api/block_actions.rs:441-444
- `datepicker` → `selected_date` (`YYYY-MM-DD`). OBSERVED: emacs-slack/emacs-slack/slack-block-element.el:499-504; emacs-slack/emacs-slack/slack-room-buffer.el:823-838
- `plain_text_input` in a view: the value goes in `state.values[block_id][action_id] = {"type":"plain_text_input","value":"…"}`. `dispatch_action_config.trigger_actions_on: ["on_enter_pressed"]` means Enter dispatches. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:244-250; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/ws_view_opened.json:41-55
- Every element type that can appear, each of which must at least render: `button`, `checkboxes`, `datepicker`, `datetimepicker`, `timepicker`, `plain_text_input`, `email_text_input`, `url_text_input`, `number_input`, `radio_buttons`, `rich_text_input`, `file_input`, `overflow`, `image`, `feedback_buttons`, `icon_button`, `workflow_button`; the select menus `static_select`, `external_select`, `users_select`, `conversations_select`, `channels_select`, and their multi-select forms. OBSERVED: beeper/slackgo/block_element.go:6-22; https://docs.slack.dev/reference/block-kit/block-elements; emacs-slack/emacs-slack/slack-block-element.el:63-79
- Text objects are `plain_text` or `mrkdwn`. OBSERVED: emacs-slack/emacs-slack/slack-block-composition.el:49-50

#### Block types to render

- `section` (text, fields, accessory), `divider`, `image` (`image_url`, `alt_text`, `image_width`, `image_height`, `image_bytes`, `is_animated`, `fallback`), `actions`, `context`, `context_actions`, `file`, `input` (`label`, `optional`, `dispatch_action`, `element`), `header`, `rich_text`, `call`, `video`, `markdown`, `table`, `data_table`, `task_card`, `plan`, `alert`, `card`, `carousel`. OBSERVED: beeper/slackgo/block.go:8-27; https://docs.slack.dev/reference/block-kit/blocks; gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:139-152, 202-212; mautrix/slack/pkg/msgconv/blocks.go:399-466
- Seen in real messages but missing from the public list: `timeline`, and `container`, which wraps AI-agent approval cards; each child block keeps its own `block_id`. OBSERVED: emacs-slack/emacs-slack/slack-block.el:61, 67; emacs-slack/emacs-slack/slack-room-buffer.el:735-750
- `rich_text` children: `rich_text_section`, `rich_text_list` (ordered or bullet), `rich_text_preformatted`, `rich_text_quote`. Inline elements: `text`, `link`, `emoji`, `user`, `usergroup`, `channel`, `team`, `broadcast`, `date`, `color`, `message_mention`, `attachment_mention`, `canvas`, `citation`. OBSERVED: beeper/slackgo/block_rich_text.go:90-94, 310-324; emacs-slack/emacs-slack/slack-block-rich-text-element.el:83-107; mautrix/slack/pkg/msgconv/blocks.go:319-379, 500-506
- `table` cells are either `rich_text` or `raw_text`. OBSERVED: emacs-slack/emacs-slack/slack-block-table.el:58-65
- Blocks also appear inside legacy attachments (`attachments[].blocks`; Giphy's preview is `image` + `actions`). Interactions there use the `message_attachment` container. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/ws_ephemeral_preview.json:8-29; beeper/slackgo/attachments.go:100; mautrix/slack/pkg/msgconv/blocks.go:671, 733-735
- Unknown block types must be kept and rendered with a fallback, not dropped; slackgo preserves them on round-trip. OBSERVED: beeper/slackgo/block_unknown_test.go:19
- Even plain user messages arrive as `rich_text` blocks. OBSERVED: slackless/.omo/research/gist.md:549, 612

### blocks.suggestions

- **Transport:** common form POST.
- **Visibility:** private.
- **Request params:** `service_id` (bot id), `app_id`, `service_team_id`, `container` (same shapes as in `blocks.actions`), `action_id`, `block_id`, `value` (the typed query), and optional `function_execution_id`. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/slack/api/block_suggestions.rs:152-188
- **Response:** `{"ok":true,"options":[{"text":{…},"value":"…"}]}`, or `option_groups` instead of `options`. OBSERVED: emacs-slack/emacs-slack/slack-block-element.el:298-325
- **Pagination:** none; one call per query. Respect the element's `min_query_length`. OBSERVED: emacs-slack/emacs-slack/slack-block-element.el:261, 301-303
- **Used for:** option lists of `external_select` and its multi-select form.

### views.get

- **Transport:** common form POST, `_x_reason=fetchView`. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:112-116
- **Visibility:** private.
- **Request params:** `view_id` - string - required. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:234-237; 3kh0/super-platinum/crates/super-platinum-core/src/slack/api/block_views.rs:12-19
- **Response:** `{"ok":true,"view":{…}}`, the same view object the websocket events carry. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:236-237
- **Pagination:** none.
- **Used for:** the web client refetches after every `view_opened`, `view_updated` and push, even though the event already includes the view. Optional for slackless. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:90-96

### views.submit

- **Transport:** common form POST, `_x_reason=submit-view`. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:112-116
- **Visibility:** private on the client side; the app receives it as a `view_submission` interaction. OBSERVED: beeper/slackgo/interactions.go:30
- **Request params** (capture order):
  - `client_token` - string - required - `web-<unix ms>`.
  - `view_id` - string - required.
  - `state` - JSON - required - `{"values":{"<block_id>":{"<action_id>":{"type":"plain_text_input","value":"dancer"}}}}`; a view without inputs sends `{"values":{}}`.
  - OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:239-250; 3kh0/super-platinum/crates/super-platinum-core/src/slack/api/block_views.rs:21-45
- **Response:**
  - push: `{"ok":true,"view":{…new view…},"response_action":"push"}`. The new view has a new `id`, `previous_view_id` = the submitted view, the same `root_view_id`, and `close` becomes "Back".
  - clear: `{"ok":true,"view":null,"response_action":"clear"}`. Close the whole modal stack.
  - OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:252-258
- **Pagination:** none.
- **Used for:** the modal submit button. The `update` and `errors` responses were not captured. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:260-262

### views.close

- **Transport:** common form POST.
- **Visibility:** private.
- **Request params:** `client_token` - string - required; `view_id` - string - required; `root_view_id` - string - optional. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/slack/api/block_views.rs:47-64
- **Response:** not captured. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:288-292
- **Pagination:** none.
- **Used for:** closing a modal (Cancel, or dismissing it). The method name also appears in ImShyMike/slack-undoc-client and in super-platinum's own transport tests. OBSERVED: ImShyMike/slack-undoc-client/src/client.ts:2086-2087; 3kh0/super-platinum/crates/super-platinum-core/src/slack/transport/block_interaction_tests.rs:35

### chat.attachmentAction

- **Transport:** common form POST.
- **Visibility:** private.
- **Request params:**
  - `payload` - JSON string - required - keys in web-client order: `actions` (one action: `id`, `name`, `text`, `type`, `value`, `style` (sent even when empty), optional `url`, optional `confirm{text,title,ok_text,dismiss_text}`), `attachment_id` (as a string), `callback_id`, `channel_id`, `is_ephemeral`, `message_ts`, `prompt_app_install`. OBSERVED: gammons/slk/internal/slack/attachment_action.go:56-121
  - `service_id` - bot id - required (or `bot_user_id`) - the posting app's bot. emacs-slack falls back to `B01`. OBSERVED: gammons/slk/internal/slack/attachment_action.go:23-25, 39-42; emacs-slack/emacs-slack/slack-room-buffer.el:496-498
  - `client_token` - string - sent by emacs-slack. OBSERVED: emacs-slack/emacs-slack/slack-room-buffer.el:507-509
  - Select menus add `data_source`, `min_query_length` and `selected_options: [{"value":"…"}]` to the action. OBSERVED: emacs-slack/emacs-slack/slack-attachment.el:331-351
- **Response:** `{"ok":true}`. The effect arrives on the websocket; for an ephemeral it is a `message_deleted`. OBSERVED: gammons/slk/internal/slack/attachment_action.go:30-33, 44-52
- **Pagination:** none.
- **Used for:** buttons and menus in legacy `attachments[].actions`.

#### Legacy attachment fields to render

- Fields: `color` (left bar), `fallback`, `pretext`, `author_name`/`author_link`/`author_icon`/`author_subname`, `title`/`title_link`, `text`, `fields[]` (`title`, `value`, `short` = half width), `image_url` (+ `image_width`/`image_height`), `thumb_url`, `footer`/`footer_icon`, `ts`, `mrkdwn_in` (names which fields are mrkdwn), `actions[]`, `callback_id`, `id`, `blocks`, `files`, `is_share`. Shared-message unfurls add `channel_id`, `channel_name`, `from_url`. OBSERVED: beeper/slackgo/attachments.go:10-13, 65-109; emacs-slack/emacs-slack/slack-attachment.el:41-70
- Action objects: `name`, `text`, `type`, `style` (`default`/`primary`/`danger`), `value`, `data_source` (`static`, `users`, `channels`, `conversations` or `external`), option groups, `selected_options`, `min_query_length`, `confirm` (`title`, `text`, `ok_text`, `dismiss_text`), `url`. OBSERVED: beeper/slackgo/attachments.go:19-30, 40-59; emacs-slack/emacs-slack/slack-attachment.el:94-123, 278-329
- emacs-slack paints `color` as `#<color>`, so it expects hex without the `#`. OBSERVED: emacs-slack/emacs-slack/slack-attachment.el:455-458

### chat.attachmentSuggestion

- **Transport:** seen only in emacs-slack: `POST https://slack.com/api/chat.attachmentSuggestion` with `service_id` and `payload` in the URL query string and the token in `Authorization: Bearer` plus `Cookie: d=`. The common transport is unverified for this method (see LEADS). OBSERVED: emacs-slack/emacs-slack/slack-attachment.el:240-250, 264-271; emacs-slack/emacs-slack/slack-request.el:201-240
- **Visibility:** private.
- **Request params:** `service_id` - bot id - required; `payload` - JSON string - required - `{"name":<action name>,"value":<typed query>}` plus the common attachment fields (`attachment_id`, `callback_id`, `is_ephemeral`, `message_ts`, `channel_id`). OBSERVED: emacs-slack/emacs-slack/slack-attachment.el:233-251; emacs-slack/emacs-slack/slack-room-buffer.el:488-495
- **Response:** `options[]` with `text` and `value`. OBSERVED: emacs-slack/emacs-slack/slack-attachment.el:259-263, 283-296
- **Pagination:** none.
- **Used for:** legacy attachment select menus with `data_source: external`.

### dialog.get

- **Transport:** seen only in emacs-slack: `POST https://slack.com/api/dialog.get` with `dialog_id` in the URL query string and the token in `Authorization: Bearer` plus `Cookie: d=`. The common transport is unverified for this method (see LEADS). OBSERVED: emacs-slack/emacs-slack/slack-dialog-buffer.el:381-382, 395-401; emacs-slack/emacs-slack/slack-request.el:201-240
- **Visibility:** private; serves legacy dialogs from before Block Kit modals.
- **Request params:** `dialog_id` - string - required. OBSERVED: emacs-slack/emacs-slack/slack-dialog-buffer.el:380-401
- **Response:** `{"ok":true,"dialog":{…}}`. Elements carry `name`, `data_source` (`external`, `conversations`, `channels`, `users` or `static`) and `min_query_length`; the dialog carries `notify_on_cancel`. OBSERVED: emacs-slack/emacs-slack/slack-dialog-buffer.el:384-394; emacs-slack/emacs-slack/slack-dialog.el:188-195, 214-272, 300-303
- **Pagination:** none.
- **Used for:** fetching a legacy dialog after a `dialog_opened` event.

### dialog.submit

- **Transport:** seen only in emacs-slack: `POST https://slack.com/api/dialog.submit` with `submission` and `dialog_id` in the URL query string and the token in `Authorization: Bearer` plus `Cookie: d=`. The common transport is unverified for this method (see LEADS). OBSERVED: emacs-slack/emacs-slack/slack-dialog.el:277-279, 284-290; emacs-slack/emacs-slack/slack-request.el:201-240
- **Visibility:** private, legacy.
- **Request params:** `dialog_id` - string - required; `submission` - JSON object of element `name` → value - required. OBSERVED: emacs-slack/emacs-slack/slack-dialog.el:274-298
- **Response:** handed to the caller unparsed; emacs-slack keeps a dialog-level `error-message` and per-element `errors` for it. OBSERVED: emacs-slack/emacs-slack/slack-dialog.el:280-283, 316-319
- **Pagination:** none.
- **Used for:** submitting a legacy dialog.

### dialog.selectSuggestion

- **Transport:** seen only in emacs-slack: `POST https://slack.com/api/dialog.selectSuggestion` with `dialog_id`, `name` and `value` in the URL query string and the token in `Authorization: Bearer` plus `Cookie: d=`. The common transport is unverified for this method (see LEADS). OBSERVED: emacs-slack/emacs-slack/slack-dialog.el:190-195, 201-208; emacs-slack/emacs-slack/slack-request.el:201-240
- **Visibility:** private, legacy.
- **Request params:** `dialog_id`, `name` (element name), `value` (typed query) - all required. OBSERVED: emacs-slack/emacs-slack/slack-dialog.el:188-208
- **Response:** `options[]` or `option_groups[]` (each option has `label` and `value`). OBSERVED: emacs-slack/emacs-slack/slack-dialog.el:222-250
- **Pagination:** none.
- **Used for:** external select menus in legacy dialogs.

### dialog.notifyCancel

- **Transport:** seen only in emacs-slack: `POST https://slack.com/api/dialog.notifyCancel` with `dialog_id` in the URL query string and the token in `Authorization: Bearer` plus `Cookie: d=`. The common transport is unverified for this method (see LEADS). OBSERVED: emacs-slack/emacs-slack/slack-dialog.el:301-302, 308-314; emacs-slack/emacs-slack/slack-request.el:201-240
- **Visibility:** private, legacy.
- **Request params:** `dialog_id` - string - required; sent only when the dialog has `notify_on_cancel`. OBSERVED: emacs-slack/emacs-slack/slack-dialog.el:300-314
- **Response:** only `ok` and `error` are checked. OBSERVED: emacs-slack/emacs-slack/slack-dialog.el:305-307
- **Pagination:** none.
- **Used for:** cancelling a legacy dialog.

### apps.home.dispatchOpenEvent

- **Transport:** common form POST, `_x_reason=app-home-logging`.
- **Visibility:** private.
- **Request params:**
  - `id` - conversation id - required - the app's DM channel (`D…`).
  - `type` - `home` - required.
  - `service_team_id` - team id - optional - super-platinum sends the route's team context (`route.team_id()`) and omits the field when that context is absent or empty, never falling back to the active team; its legacy `opened` wrapper sent `app_installed_team_id`. slock always sends the app's `app_profile.team_id` from `apps.profile.get`.
  - OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/home.rs:337-374; 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/home/tests/unified.rs:9-29; 3kh0/super-platinum/src/desktop/app_home.rs:375-380; ItsFelix5/slock/packages/plugins/src/app-home/server/index.ts:40-48
- **Response:** not decoded by any source read.
- **Pagination:** none.
- **Used for:** sent when the user opens an app's Home tab. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/home.rs:337-343; ItsFelix5/slock/packages/plugins/src/app-home/server/index.ts:47

#### App Home flow (as implemented by super-platinum)

1. `conversations.info` with `channel=<app DM>`, `return_app_home=true` and `_x_reason=app-home-open-api` (conversations area) returns `app_home: {home_tab_enabled, messages_tab_enabled}` and `home_view`, a view with `type: "home"`, `id`, `app_id`, `app_installed_team_id`, `blocks` and `state`. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/home.rs:62-111, 319-336
2. `apps.home.dispatchOpenEvent` as above.
3. Interactions inside the Home tab use `blocks.actions` with `container: {"type":"view","view_id":<home view id>}`. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/home.rs:299-316
4. The app publishes Home content with the public `views.publish` (`user_id`, `view`, optional `hash`; Tier 4). A user client never calls it. OBSERVED: https://docs.slack.dev/reference/methods/views.publish

### apps.home.open

- **Transport:** common form POST, `_x_reason=app-home-open-api`.
- **Visibility:** private.
- **Request params:** `app_id` - required; `app_installed_team_id` - required. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/home/navigation.rs:88-106
- **Response:** `app_home.conversation_id`, the app's DM channel to open. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/home/navigation.rs:109
- **Pagination:** none.
- **Used for:** navigating to an app's Home from somewhere else (app profile, link).

### apps.permissions.internal.add

- **Transport:** seen only in emacs-slack: `POST https://slack.com/api/apps.permissions.internal.add` with all five params in the URL query string and the token in `Authorization: Bearer` plus `Cookie: d=`. The common transport is unverified for this method (see LEADS). OBSERVED: emacs-slack/emacs-slack/slack-websocket.el:783-788, 801-807; emacs-slack/emacs-slack/slack-request.el:201-240
- **Visibility:** private.
- **Request params:** `channel`, `app_user`, `invite_message_ts`, `did_confirm=true`, `send_ephemeral_error_message=true`. OBSERVED: emacs-slack/emacs-slack/slack-websocket.el:780-807
- **Response:** only `ok` and `error` are checked. OBSERVED: emacs-slack/emacs-slack/slack-websocket.el:796-800
- **Pagination:** none.
- **Used for:** approving an app's request to join a conversation, after `app_conversation_invite_request`.

### apps.permissions.internal.denyAdd

- **Transport:** seen only in emacs-slack: `POST https://slack.com/api/apps.permissions.internal.denyAdd` with the three params in the URL query string and the token in `Authorization: Bearer` plus `Cookie: d=`. The common transport is unverified for this method (see LEADS). OBSERVED: emacs-slack/emacs-slack/slack-websocket.el:812-815, 828-834; emacs-slack/emacs-slack/slack-request.el:201-240
- **Visibility:** private.
- **Request params:** `channel`, `app_user`, `invite_message_ts`. OBSERVED: emacs-slack/emacs-slack/slack-websocket.el:809-834
- **Response:** only `ok` and `error` are checked. OBSERVED: emacs-slack/emacs-slack/slack-websocket.el:823-827
- **Pagination:** none.
- **Used for:** denying the same request.

### workflows.triggers.trip

- **Transport:** common form POST, `_x_reason=trip-trigger-from-bk-workflow-button`.
- **Visibility:** private.
- **Request params:**
  - `client_token` - string - required.
  - `context` - JSON string - required - for an App Home button: `{"action_id":"…","block_id":"…","location":"button","message_ts":"<home view id>"}`.
  - `url` - string - required - the link-trigger URL from the button's `workflow.trigger.url` (`https://slack.com/shortcuts/Ft…/…`); must be https.
  - `run_precheck` - `true` - optional - sent only when true, never as `false`.
  - OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/home/workflow.rs:95-112, 145-176; https://docs.slack.dev/reference/block-kit/block-elements/workflow-button-element
- **Response:** `{"ok":true,"is_slow_workflow":<bool>,"contains_ai_steps":<bool>}`. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/home/workflow.rs:186-200
- **Pagination:** none.
- **Used for:** running a link trigger from a `workflow_button`. Before tripping, the client can fetch trigger metadata with `workflows.triggers.info`. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/home/workflow/authentication.rs:15-35

### workflows.triggers.info

- **Transport:** common form POST, `_x_reason=hermes_triggers_ufm`.
- **Visibility:** private.
- **Request params:** `trigger_id` - required; `include_workflow_details` - `true`. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/channel_workflows/details.rs:11-20; 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/home/workflow/authentication.rs:23-31
- **Response:** `{"ok":true,"trigger":{"id":"…",…}}`, plus workflow details. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/home/workflow/authentication.rs:33-35
- **Pagination:** none.
- **Used for:** workflow details in the channel's Workflows list, and metadata before a workflow-button trip.

### workflows.triggers.preview

- **Transport:** common form POST, `_x_reason=hermes_trigger_previews_ufm`.
- **Visibility:** private.
- **Request params:** `trigger_ids` - comma-joined ids - required. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/channel_workflows.rs:37-47
- **Response:** not read in detail.
- **Pagination:** batch by ids; no cursor.
- **Used for:** the channel's Workflows list. The trigger ids come from `bookmarks.list` with `include_folders=true`, `include_legacy_workflows=true` and `_x_reason=bookmarks-store/conditional-fetching` (bookmarks area). OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/channel_workflows.rs:27-36

### workflows.triggers.list

- **Transport:** common form POST.
- **Visibility:** private.
- **Request params:** either `app_id` (`_x_reason=load-workflow-editor`) or `app_ids` (`_x_reason=automations_gallery`). OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/workflow_management.rs:128-136; 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/workflow_management/list.rs:86-88
- **Response:** not read in detail.
- **Pagination:** none observed.
- **Used for:** workflow editor and gallery listings; the web client also fires it during boot. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:3769-3772

### workflows.triggers.listRecentlyRunForUser

- **Transport:** common form POST.
- **Visibility:** private.
- **Request params:** not captured (only the call name and timing).
- **Response:** not captured.
- **Pagination:** unknown.
- **Used for:** fired at boot; by its name, the "recently run" workflows list. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:3641-3644

### functions.workflows.list

- **Transport:** common form POST, `_x_reason=automations_gallery`.
- **Visibility:** private.
- **Request params:** `limit` (50), `filter_options`, `sort_options` (`{"column":"date_updated","direction":"asc"|"desc"}`), `workflow_builder_only` (`true`), `cursor` (on later pages). OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/workflow_management/list.rs:31-39
- **Response:** `{"ok":true,"workflows":[{"id":"…","app_id":"…",…}]}`. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/workflow_management/list.rs:57-59
- **Pagination:** cursor. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/workflow_management/list.rs:35
- **Used for:** the Workflow Builder / automations gallery; also fired at boot. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:3649-3652

### functions.workflows.get

- **Transport:** common form POST, `_x_reason=hermes_workflows_store_unknown_fetching`.
- **Visibility:** private.
- **Request params:** `workflow_id` - required. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/channel_workflows/details.rs:21-30
- **Response:** not read in detail.
- **Pagination:** none.
- **Used for:** fetching a workflow the client has not seen yet, e.g. one referenced by a trigger.

### conversations.bulkReacjiTriggers

- **Transport:** common form POST.
- **Visibility:** private.
- **Request params:** not captured.
- **Response:** not captured.
- **Pagination:** n/a.
- **Used for:** fired on every channel open, so the client knows which emoji reactions in that channel start a workflow. OBSERVED: slackless/.omo/research/gist.md:184, 632; slackless/.omo/research/findings.md:61

## Websocket events

The general event reference is api-realtime.md; these are the events this area depends on.

- `view_opened` - fires right after a request that opens a modal (`chat.command`, a shortcut, a button). It carries the whole view; match `client_token` to your request. `channel_id` is `null` even for commands run in a channel, and `timeout_range: 10000` is unexplained. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/ws_view_opened.json:1-90; gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:118-177; 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/modal.rs:423-440
  ```json
  {"type":"view_opened","view_id":"V…","root_view_id":"V…","previous_view_id":null,"timeout_range":10000,
   "client_token":"web-1791802169822","title":{…},"submit":{…},"close":{…},"view_type":"modal",
   "app_id":"A0F827J2C","app_installed_team_id":"T…",
   "view":{"id":"V…","team_id":"T…","type":"modal","blocks":[…],"private_metadata":"","callback_id":"giphy:search",
           "state":{"values":{}},"hash":"1791802169.OiD6M0Kv","clear_on_close":false,"notify_on_close":false,
           "previous_view_id":null,"root_view_id":"V…","external_id":"","app_installed_team_id":"T…","bot_id":"B…"},
   "channel_id":null,"event_ts":"1791802169.005200"}
  ```
- `view_updated` - fires when the app changes a view: after a `views.submit` push, or after a `blocks.actions` in the view. Carries `view_id`, `app_id`, `app_installed_team_id`, the full `view`, and `event_ts`, but no `client_token`. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:179-232; ItsFelix5/slock/packages/plugins/src/view-modals/client/index.tsx:12-17
- `commands_changed` - fires when slash commands are added or removed. Payload `commands_updated[]` and `commands_removed[]` (command objects). OBSERVED: emacs-slack/emacs-slack/slack-websocket.el:521-522, 836-850; beeper/slackgo/websocket_managed_conn.go:653
- `emoji_changed` - subtype `add` (`name`, `value`), `remove` (`names[]`) or `rename` (`old_name`, `new_name`). OBSERVED: wee-slack/wee-slack/slack/slack_workspace.py:741-747; 3kh0/super-platinum/crates/super-platinum-core/src/composer_emoji/changes.rs:7-31, 59; beeper/slackgo/websocket_managed_conn.go:651
- `pref_change` for `emoji_use`, `frecency`, `emoji_mode` or `preferred_skin_tone` - updates one pref; do not treat it as a full reload. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/composer_emoji/preferences.rs:7, 53-62; beeper/slackgo/websocket_managed_conn.go:635
- `dialog_opened` - payload `dialog_id`, `client_token`; fetch with `dialog.get` only if the token is yours. OBSERVED: emacs-slack/emacs-slack/slack-websocket.el:523-524, 852-858
- `app_conversation_invite_request` - payload `app_user`, `channel_id`, `invite_message_ts`, `scope_info[]` (each with `short_description`). Answer with `apps.permissions.internal.add` or `denyAdd`. OBSERVED: emacs-slack/emacs-slack/slack-websocket.el:755-778
- `bot_added`, `bot_changed` - payload `bot` (bot profile). OBSERVED: emacs-slack/emacs-slack/slack-websocket.el:477-479, 695-697; beeper/slackgo/websocket_managed_conn.go:657-658
- `apps_installed`, `apps_changed`, `apps_uninstalled` - the installed-app list changed; refresh app state. slock refetches its installed-apps list on any of the three. OBSERVED: ItsFelix5/slock/packages/types/src/gatewayEvents.ts:137-139; ItsFelix5/slock/packages/plugins/src/installed-apps/client/index.tsx:18; beeper/slackgo/websocket_managed_conn.go:662
- Message-side results of commands and interactions (message events belong to api-messaging.md and api-realtime.md): `subtype: me_message` for `/me`; a Slackbot ephemeral `bot_message` (`user: USLACKBOT`, `bot_id: B01`, `is_ephemeral: true`) for `/remind`; `message_changed` with `hidden` and an inner `is_ephemeral` when an ephemeral is updated; `message_deleted` with `deleted_ts` when it is dismissed. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/ws_me_message.json:1-29; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/ws_remind_ephemeral.json:1-33; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/ws_ephemeral_deleted.json:1-8; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-design.md:84-85

## Gotchas

- **`_x_reason` tags.** Seen so far: `executeCommand` (chat.command), `dispatch-client-extras` and `app-commands-conditional-fetching` (client.appCommands), `dispatch_action_to_developer` (blocks.actions), `fetchView`, `submit-view`, `app-global-actions-api/execute-app-action`, `app-home-logging`, `app-home-open-api`, `trip-trigger-from-bk-workflow-button`, `hermes_triggers_ufm`, `hermes_trigger_previews_ufm`, `fetch-frecency-prefs`, `customize-emoji-new-query`. slk copies web-client request shapes because unfamiliar shapes are what Enterprise Grid anomaly detection flags. OBSERVED: gammons/slk/internal/slack/attachment_action.go:56-61; gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:112-116
- **`client_token` is the only link between a request and the modal it opens.** The web client uses `web-<unix ms>`; emacs-slack uses `EmacsSlack-<team>-<10 digits>`, and the server echoes whatever it was given. Generate a unique token per request and keep a token→origin map; the origin also supplies the channel, because `view_opened.channel_id` is null. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/ws_view_opened.json:7, 88; emacs-slack/emacs-slack/slack-team.el:237-246; emacs-slack/emacs-slack/slack-websocket.el:852-858
- **Slash-command arguments changed encoding.** The web client sends them as a `rich_text` `blocks` array; wee-slack, libpurple and emacs-slack send `text`. Copy the web shape. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/chat_command_channel.json:16-19; wee-slack/wee-slack/slack/slack_api.py:472-476
- **Some core commands never reach `chat.command`.** `/away`, `/status away`, `/msg` and `/open` are implemented in the client with other APIs, so slackless must implement them locally. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-design.md:94-102
- **Giphy is a `service`, not an app command,** and its app id `A0F827J2C` is the same in every workspace. After Send, the posted message has `user` = the current user **and** `bot_id` = Giphy's bot; render it as the user's message with app attribution. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:69-70, 98; gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/client_app_commands.json:439-441
- **Modal stack.** `push` returns a new view id with `previous_view_id` set and the same `root_view_id`; `clear` closes the whole stack. Keep the stack client-side, keyed by `root_view_id`. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-app-modals-research.md:252-258
- **Enterprise Grid.** Commands carry `teams[]` and `is_workspace_agnostic`. Global shortcuts must send `team_id` when the action lists teams. Interactions send `service_team_id` = the installing team (`app_installed_team_id`), not the active team, and App Home sends `service_team_id` only from the view context. OBSERVED: gammons/slk/docs/superpowers/specs/2026-10-12-slash-commands-captures/client_app_commands.json:35-38; 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/shortcuts.rs:209-218; 3kh0/super-platinum/crates/super-platinum-core/src/slack/api/block_suggestions.rs:143-157; 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/home.rs:344-372
- **Emoji aliases.** Resolve `alias:` recursively with a depth limit. A target can be a standard emoji name or nothing at all: emacs-slack falls back to `:slack:` for a dangling alias, and mautrix refuses to follow a second level. OBSERVED: emacs-slack/emacs-slack/slack-emoji.el:123-135; mautrix/slack/pkg/connector/emoji.go:333-338
- **Shortcode boundary.** Slack substitutes `:name:` only when it stands alone, so `:00:` inside `17:00:00` stays text even if a custom emoji `00` exists. OBSERVED: emacs-slack/emacs-slack/slack-emoji.el:57-82
- **`emoji_mode: as_text`** means the user wants emoji shown as text; the boot default is `default`. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/composer_emoji/preferences.rs:64-66; rusq/slackdump/internal/edge/assets/client.userBoot.json:1019
- **`emoji.list` is Tier 2:** fetch it once per session and on `emoji_changed`; do not poll. OBSERVED: https://docs.slack.dev/reference/methods/emoji.list
- **Workflow buttons:** super-platinum refuses `workflow_button`s that have `confirm` or `visible_to_user_ids`, and requires an https trigger URL. Copy that caution until those cases are captured. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/block_actions/home/workflow.rs:95-112
- **No `workflows.listForChannel`.** No source calls it. A channel's workflows come from `bookmarks.list` (legacy workflows included) plus `workflows.triggers.preview`. OBSERVED: 3kh0/super-platinum/crates/super-platinum-core/src/channel_workflows.rs:27-47
- **Older method family** (emacs-slack, 2017-2018): `commands.list`, `apps.actions.list/run`, JSON-body `blocks.actions`, `dialog.*`. None appear in the 2026 web-client captures; use them only as fallbacks. OBSERVED: emacs-slack/emacs-slack/slack-slash-commands.el:3; emacs-slack/emacs-slack/slack-action.el:3
- **Evidence pinned to these commits** (re-check here when Slack changes): mautrix/slack 38d69d5, beeper/slackgo d0ae74c, rusq/slackdump fec2e68, wee-slack/wee-slack aa904c2, emacs-slack/emacs-slack eafa2d4, dylex/slack-libpurple 1115ec1, gammons/slk 78a0177, 3kh0/super-platinum 58e7455, ItsFelix5/slock abb335d, ImShyMike/slack-undoc-client 868fd4d, iamcal/emoji-data e77759c (HEAD of each clone, read on 2026-11-19).

## LEADS

- `views.close` response, and whether it is what makes Slack send `view_closed` to the app for views with `notify_on_close: true`. "Back" on a pushed view is probably a local pop to `previous_view_id` with no API call.
- Client-side shapes of the `views.submit` responses `response_action: "update"` and `"errors"`; the app-side docs describe both.
- `state.values` shapes for selects, checkboxes, radios, date/time pickers and number/email/url inputs. Expect the app-side shapes: `selected_option`, `selected_options`, `selected_date`, `selected_time`, `selected_date_time`, `value`.
- `timeout_range: 10000` on `view_opened`: probably how long the client waits for the app before showing an error.
- Whether the client ever sends the view `hash`, the app-side concurrency token.
- Modals opened by a shortcut or a message button probably arrive as `view_opened` the same way.
- How the web client dispatches `/topic /invite /leave /mute /dnd /feed /who /archive /remove /kick /apps /feedback /active`. Most probably `chat.command`, but `/leave`, `/dnd`, `/mute` and `/active` may be handled in the client like `/away`.
- `/collapse` and `/expand` are probably handled entirely in the client now (toggling inline media).
- `client.appCommands` `cache_ts` is probably a conditional-fetch key: send the last `cache_ts` and get only changes.
- Whether the older `text` form of `chat.command` arguments is still accepted.
- The `emoji_use` format (probably a JSON map of name→count), and how the web client writes `frecency`/`emoji_use` back (probably `users.prefs.set name=frecency value=<json>`).
- Frecency item ids for emoji (tests use `E1`; the real prefix is unknown).
- Request and response bodies of `edgeapi emojis/info`, and params and response of `search.autocomplete.topEmojis`.
- `emoji.getInfo`, `emoji.collections.list` and `blocks.format` exist as method names in a generated client of undocumented methods (ImShyMike/slack-undoc-client/src/client.ts:558, 1078-1083), but what they do is unknown.
- `conversations.bulkReacjiTriggers`: params (probably the channel id) and response (probably an emoji→trigger map).
- `workflows.triggers.listRecentlyRunForUser` params and response; the public app-side `workflows.featured.list` may be what backs "featured workflows" in a channel header (beeper/slackgo/workflows_featured.go).
- `apps.home.dispatchOpenEvent` probably makes Slack send `app_home_opened` to the app.
- `apps.actions.v2.execute` probably opens its modal through `view_opened`, like `chat.command`.
- A pasted link-trigger URL (`https://slack.com/shortcuts/Ft…`) is probably run with `workflows.triggers.trip` too, with a different `context`.
- Legacy attachment `color` may also be one of the names `good`, `warning`, `danger`, or carry a leading `#`.
- Legacy dialogs (`dialog.*`) may be retired for new apps.
- iamcal short names may differ from Slack's alias groups for some emoji; diff the two tables at build time.
- unverified: the common transport (workspace host, token and params in a form body) for `commands.list`, `apps.actions.list`, `apps.actions.run`, `chat.attachmentSuggestion`, `dialog.get`, `dialog.submit`, `dialog.selectSuggestion`, `dialog.notifyCancel`, `apps.permissions.internal.add` and `apps.permissions.internal.denyAdd`. Their only source, emacs-slack, posts to `https://slack.com/api/<method>` with the params in the URL query string and the token in an `Authorization: Bearer` header; test the web-client form before relying on it. OBSERVED: emacs-slack/emacs-slack/slack-request.el:201-240; tkf/emacs-request/request.el:348, 506-508 (c22e3c2)
