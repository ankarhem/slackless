# Threads, activity, unreads and notifications - Slack web-client API

## Summary

- Scope: the Threads view (`subscriptions.thread.*`), the Activity view (`activity.*`, `messages.list` previews), unread state and "All unreads" (`client.counts`, `conversations.mark`, `*_marked` frames), badges (`channel_badges` / `activity_v2`, `badge_counts_updated`), keyword highlights and notification prefs (`users.prefs.*`), schedules and DND (`dnd.*`). Evidence and OBSERVED tags are in the sections below.
- Public: only `conversations.mark`, the five `dnd.*` methods and the `channel_marked` / `im_marked` / `group_marked` / `pref_change` / `dnd_updated(_user)` events have docs.slack.dev pages. Every other method and event here returns "Page not found" there, or appears only in the client bundle, so it is private web-client API.
- Model: `client.counts` (on connect, on a poll timer, after reconnect) is the unread and badge snapshot. Realtime frames (`*_marked`, `thread_marked`, `badge_counts_updated`, `activity`, `pref_change`, `dnd_updated`) patch it, and badge numbers are computed client-side under experiment-gated rules.
- Notification prefs are one double-encoded JSON pref, `all_notifications_prefs`, written key by key with `users.prefs.setNotifications`. Schedules ("notification hours"), sounds-mute and badge toggles go through `users.prefs.set`.
- Risks: Activity categories and badge math change with server experiments; `subscriptions.thread.getView` is expensive and its cursor is easy to get wrong; Enterprise Grid delivers the same frame on every workspace socket; and whether to trust `desktop_notification` frames or compute notifications locally is unresolved (see LEADS).

## Endpoints

**Conventions.**

- **Transport.** Unless a subsection says otherwise, every call is `POST https://<ws>.slack.com/api/<method>` with body field `token=xoxc-…` and cookie `d=xoxd-…`. The official web client sends its API requests to the workspace host (OBSERVED: ankarhem/slackless/.omo/research/findings.md:22).
  - **Host.** gammons takes the API base from `auth.test`'s `url`. On Enterprise Grid that is `https://<org>.enterprise.slack.com/api/`, and sending Grid requests to `https://slack.com/api/` instead routes them to a different team or fails. gammons falls back to `https://slack.com/api/`; emacs-slack hard-codes `https://slack.com/api/` and wee-slack `https://api.slack.com/api/`. (OBSERVED: gammons/slk/internal/slack/client.go:78,391-399,436-443; emacs-slack/emacs-slack/slack-all-threads-buffer.el:32; wee-slack/wee-slack/slack/slack_api.py:142)
  - **Token placement.** emacs-slack and ericboehs send the token as `Authorization: Bearer` instead of a body field. gammons keeps it in the form body because a Bearer header next to browser-shaped headers triggered Enterprise Grid anomaly detection. (OBSERVED: gammons/slk/internal/slack/client.go:1836-1847; emacs-slack/emacs-slack/slack-request.el:210-240; ericboehs/slk/lib/slk/models/workspace.rb:27-31)
- **Body encoding.** The official client sends `multipart/form-data` bodies ending in `_x_reason=<ui action>`, `_x_mode=online`, `_x_sonic=true`, `_x_app_name=client` (OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:1505-1519; emacs-slack/emacs-slack/slack-activity-feed-buffer.el:152-160). `application/x-www-form-urlencoded` is accepted as well (OBSERVED: gammons/slk/internal/slack/client.go:1257-1263; wee-slack/wee-slack/slack/slack_api.py:441-447). emacs-slack is different: `slack-request-create :params` go into the URL query string (request.el), so its `POST`s in this area carry an empty body; only its `activity.feed` call sends one (OBSERVED: emacs-slack/emacs-slack/slack-request.el:201-209; tkf/emacs-request/request.el:348,505-508; emacs-slack/emacs-slack/slack-activity-feed-buffer.el:230-239).
- **`_x_reason`.** It is a body field, never a query param, and appears on 153 of 163 captured official requests (OBSERVED: gammons/slk/internal/slackhttp/reason.go:25-31).
- **Errors.** Failures use the `{"ok": false, "error": "…"}` envelope (OBSERVED: https://docs.slack.dev/reference/methods/conversations.mark).
- **Cross-area methods** (owned by sibling docs):
  - `client.userBoot` also carries `prefs` and a `dnd` block (OBSERVED: wee-slack/wee-slack/typings/slack_api/slack_client_userboot.pyi:66-67).
  - The client re-reads `conversations.history` with `_x_reason=unread-counts/onLastReadUpdated` around the unread marker (OBSERVED: gammons/slk/internal/slackhttp/reason.go:64-66).
  - Thread replies load lazily via `conversations.replies` (OBSERVED: ankarhem/slackless/.omo/research/findings.md:179).
  - `saved.list` (Later) runs at boot (OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:3883).

Source pins. Line numbers in OBSERVED tags refer to these commits:

| Repo                    | Commit                                   | What it is                                                            |
| ----------------------- | ---------------------------------------- | --------------------------------------------------------------------- |
| mautrix/slack           | 38d69d5d40ab05d0d6e944f5569fb45f124d7cb6 | Matrix bridge; `go.mod:48` replaces slack-go with beeper/slackgo      |
| beeper/slackgo          | d0ae74c35f4863bbacaf814dd1fb0c5aacb4e882 | the slack-go fork mautrix depends on                                  |
| rusq/slackdump          | fec2e68b506d08e04d6c5df511dffc2365cf6ec9 | `internal/edge` web-client calls                                      |
| wee-slack/wee-slack     | aa904c24a85a50278e271fa6d4ac29838186c800 | WeeChat client (session-token mode)                                   |
| emacs-slack/emacs-slack | eafa2d4886451e75ef7a50d887fbb48cf70ac354 | Emacs client                                                          |
| dylex/slack-libpurple   | 1115ec117bcddb38ee0ec649f139d04ab3b53942 | Pidgin plugin                                                         |
| gammons/slk             | 78a017777fc5c777828de9f4af3aa08abe74cff1 | Go TUI on the web-client API; ships HAR-derived contracts in testdata |
| ericboehs/slk           | e6c4deffec5177c25b1e410b9d80bae8b4a455b1 | Ruby CLI on the web-client API                                        |
| 3kh0/slack-datamine     | 27df832a636724c75c2dc6f52c22bccc02af75d1 | API method and argument names mined from the client's webpack bundle  |
| 3kh0/super-platinum     | 58e74550f1e0e4c60e32bb318e8add4e527a307b | native Rust client; protocol docs cite official bundle offsets        |
| some-du6e/clack         | 6548bfbe9e14a0722abe93d522e71c42351c891f | client whose docs trace the web-client bundle                         |
| tkf/emacs-request       | c22e3c23a6dd90f64be536e176ea0ed6113a5ba6 | request.el, the HTTP library emacs-slack's `slack-request` calls      |

Visibility "private (docs 404)" means `https://docs.slack.dev/reference/methods/<name>` answered "Page not found" on 2026-11-19. "Private (bundle only)" means the name appears in the datamined client method list and the docs page was not checked.

### subscriptions.thread.getView

- **Transport:** `POST https://<ws>.slack.com/api/subscriptions.thread.getView`, form body. OBSERVED: gammons/slk/internal/slack/client.go:2165-2174; emacs-slack/emacs-slack/slack-all-threads-buffer.el:303-309
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/subscriptions.thread.getView; 3kh0/slack-datamine/build/api-methods.txt:1216
- **Request params:**
  - `current_ts` - ts string - optional - exclusive pagination cursor. gammons omits it on page 1; emacs-slack sends "now" (first 15 chars of the float epoch). OBSERVED: gammons/slk/internal/slack/client.go:2170-2172; emacs-slack/emacs-slack/slack-all-threads-buffer.el:265-269
  - `limit` - int - optional - page size. gammons sends 100, ericboehs 20 (and 1 when it only wants totals). OBSERVED: gammons/slk/internal/slack/client.go:2167; ericboehs/slk/lib/slk/api/threads.rb:15-17,55-60
  - `fetch_threads_state` - bool - optional - sent as `true`. OBSERVED: gammons/slk/internal/slack/client.go:2168
  - `priority_mode` - string - optional - sent as `all`. OBSERVED: gammons/slk/internal/slack/client.go:2169
- **Response** (shape; values illustrative):
  ```json
  {
    "ok": true,
    "total_unread_replies": 3,
    "new_threads_count": 1,
    "has_more": true,
    "max_ts": "<future watermark - NOT a cursor>",
    "threads": [
      {
        "root_msg": {
          "channel": "C…",
          "ts": "…",
          "thread_ts": "…",
          "last_read": "…",
          "subscribed": true,
          "latest_reply": "…",
          "…": "rest of the message object"
        },
        "latest_replies": [{ "…": "message" }],
        "unread_replies": [{ "…": "message" }]
      }
    ]
  }
  ```
  OBSERVED: emacs-slack/emacs-slack/slack-all-threads-buffer.el:95-104,281-288; gammons/slk/internal/slack/client.go:2015-2043; ericboehs/slk/lib/slk/api/threads.rb:55-60
- **Pagination** (per a HAR capture of the web client):
  - Threads come newest `root_msg.latest_reply` first.
  - The next `current_ts` is the `latest_reply` (falling back to `thread_ts`) of the last thread on the page. Items with `subscribed: false` still advance it.
  - Stop when `has_more` is false. Treat a cursor that does not strictly decrease as an error.
  - `max_ts` on page 1 is roughly now + 1h; echoing it back re-serves page 1 forever.

  OBSERVED: gammons/slk/internal/slack/client.go:2050-2066,2112-2160. emacs-slack instead advances with the last root's `last_read`. OBSERVED: emacs-slack/emacs-slack/slack-all-threads-buffer.el:138,187-200

- **Used for:** the Threads view (each root plus latest/unread replies) and the Threads unread count (`total_unread_replies`). gammons also reconciles local subscription rows against the full set and tombstones rows missing from it. OBSERVED: gammons/slk/cmd/slk/thread_subscriptions.go:23-35; gammons/slk/internal/cache/thread_subscriptions.go:19-27; ericboehs/slk/lib/slk/api/threads.rb:55-60

### subscriptions.thread.mark

- **Transport:** `POST https://<ws>.slack.com/api/subscriptions.thread.mark`, form body. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:449-462; beeper/slackgo/conversation.go:1096-1115
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/subscriptions.thread.mark
- **Request params:**
  - `channel` - string - required - conversation id. OBSERVED: gammons/slk/internal/slack/client.go:1615-1620
  - `thread_ts` - ts - required - thread root. OBSERVED: gammons/slk/internal/slack/client.go:1615-1620
  - `ts` - ts - required - reply to record as last read; gammons falls back to `thread_ts` when the thread has no replies. OBSERVED: gammons/slk/internal/slack/client.go:1600-1610
  - `read` - `1` or `0` - sent by gammons, wee-slack and slackgo - `1` marks read, `0` marks unread. ericboehs omits it. OBSERVED: gammons/slk/internal/slack/client.go:1611-1619; wee-slack/wee-slack/slack/slack_api.py:453-458; beeper/slackgo/conversation.go:1100-1106; ericboehs/slk/lib/slk/api/threads.rb:23-29
- **Response:** `{"ok": true}`; failures come back as `ok:false`. OBSERVED: gammons/slk/internal/slack/client.go:1621-1624
- **Pagination:** none.
- **Used for:** reading a thread, and "mark unread" inside a thread. mautrix maps threaded Matrix read receipts to it. OBSERVED: mautrix/slack/pkg/connector/handlematrix.go:392-419
- **Constraints observed in clients:**
  - wee-slack calls it only with session tokens, and only for subscribed threads ("last_read can only be set on subscribed threads"). OBSERVED: wee-slack/wee-slack/slack/slack_thread.py:126-139
  - emacs-slack calls `subscriptions.thread.get` first and marks only when a returned subscription value is `<= ts`. OBSERVED: emacs-slack/emacs-slack/slack-thread.el:156-183
  - The user's own mark echoes back as `thread_marked`. OBSERVED: gammons/slk/internal/cache/thread_subscriptions.go:88-90

### subscriptions.thread.add

- **Transport:** `POST https://<ws>.slack.com/api/subscriptions.thread.add`, form body. OBSERVED: emacs-slack/emacs-slack/slack-thread.el:105-120; ericboehs/slk/lib/slk/api/threads.rb:31-38
- **Visibility:** private (bundle only). ericboehs notes it needs a session (xoxc) token. OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:1212; ericboehs/slk/lib/slk/api/threads.rb:32
- **Request params:**
  - `channel` - string - required. OBSERVED: emacs-slack/emacs-slack/slack-thread.el:117-119
  - `thread_ts` - ts - required - thread root. OBSERVED: emacs-slack/emacs-slack/slack-thread.el:117-119
  - `last_read` - ts - sent by both clients - read cursor to start from. emacs-slack sends the root ts; ericboehs sends the latest reply ts to treat as already read. OBSERVED: emacs-slack/emacs-slack/slack-thread.el:117-119; ericboehs/slk/lib/slk/api/threads.rb:35,70-72
- **Response:** ok envelope; emacs-slack only checks for an error. OBSERVED: emacs-slack/emacs-slack/slack-thread.el:107-111
- **Pagination:** none.
- **Used for:** follow a thread ("get notified about new replies"). Subscription state changes arrive as `thread_subscribed` frames (see Websocket events). OBSERVED: ericboehs/slk/lib/slk/api/threads.rb:31; gammons/slk/internal/slack/events.go:791-810

### subscriptions.thread.remove

- **Transport:** `POST https://<ws>.slack.com/api/subscriptions.thread.remove`, form body. OBSERVED: emacs-slack/emacs-slack/slack-thread.el:122-137; ericboehs/slk/lib/slk/api/threads.rb:40-46
- **Visibility:** private (bundle only). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:1218
- **Request params:** same as `subscriptions.thread.add` (`channel`, `thread_ts`, `last_read`). OBSERVED: emacs-slack/emacs-slack/slack-thread.el:134-136; ericboehs/slk/lib/slk/api/threads.rb:44-46,70-72
- **Response:** ok envelope. OBSERVED: emacs-slack/emacs-slack/slack-thread.el:124-128
- **Pagination:** none.
- **Used for:** unfollow a thread. The state change arrives as `thread_unsubscribed`. OBSERVED: wee-slack/wee-slack/slack/slack_workspace.py:855-864

### subscriptions.thread.get

- **Transport:** `POST https://<ws>.slack.com/api/subscriptions.thread.get`, form body. OBSERVED: emacs-slack/emacs-slack/slack-thread.el:139-154; ericboehs/slk/lib/slk/api/threads.rb:48-53
- **Visibility:** private (bundle only). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:1214
- **Request params:**
  - `channel` - string - required. OBSERVED: emacs-slack/emacs-slack/slack-thread.el:152-153
  - `thread_ts` - ts - required. OBSERVED: emacs-slack/emacs-slack/slack-thread.el:152-153
- **Response:** `{"ok": true, "subscriptions": […]}`. emacs-slack compares each element with a message ts string. OBSERVED: emacs-slack/emacs-slack/slack-thread.el:145-146,168-172
- **Pagination:** none.
- **Used for:** looking up one thread's follow state ("check whether you are subscribed"), and gating `subscriptions.thread.mark`. OBSERVED: ericboehs/slk/lib/slk/api/threads.rb:48-53; emacs-slack/emacs-slack/slack-thread.el:164-172

### subscriptions.thread.clearAll

- **Transport:** `POST https://<ws>.slack.com/api/subscriptions.thread.clearAll`. emacs-slack, the only client source, sends `current_ts` as a URL query parameter with an empty body (see Conventions); a form body is unverified (see LEADS). OBSERVED: emacs-slack/emacs-slack/slack-all-threads-buffer.el:33,257-263; emacs-slack/emacs-slack/slack-request.el:201-209; tkf/emacs-request/request.el:348
- **Visibility:** private (bundle only). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:1213
- **Request params:** `current_ts` - ts - sent - "now" (first 15 chars of the float epoch). OBSERVED: emacs-slack/emacs-slack/slack-all-threads-buffer.el:249-252,262
- **Response:** ok envelope. OBSERVED: emacs-slack/emacs-slack/slack-all-threads-buffer.el:254-256
- **Pagination:** none.
- **Used for:** emacs-slack calls it every time it builds the All Threads buffer. OBSERVED: emacs-slack/emacs-slack/slack-all-threads-buffer.el:115-134. Its server-side effect is unverified (see LEADS).

### activity.feed

- **Transport:** `POST https://<ws>.slack.com/api/activity.feed`. emacs-slack hand-builds a WebKit-style multipart body; gammons sends a url-encoded form. OBSERVED: emacs-slack/emacs-slack/slack-activity-feed-buffer.el:138-160,230-239; gammons/slk/internal/slack/activity.go:127-140
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/activity.feed; 3kh0/slack-datamine/build/api-args.txt:4
- **Request params:**
  - `types` - comma list - optional - item categories. emacs-slack and gammons always send it; ericboehs sends it only when the caller passes types. OBSERVED: ericboehs/slk/lib/slk/api/activity.rb:12-16
    - gammons's list, which it describes as the one the official web client sends when bootstrapping Activity: `at_user,at_user_group,at_channel,at_everyone,keyword,thread_v2,message_reaction,bot_dm_bundle,dm,unjoined_channel_mention,channel,saved_reminder,list_user_mentioned`. OBSERVED: gammons/slk/internal/slack/activity.go:17-20
    - emacs-slack's "all" string, verbatim (duplicates included): `at_user,at_user_group,at_channel,at_everyone,keyword,list_record_assigned,list_user_mentioned,list_todo_notification,list_approval_request,list_approval_reviewed,unjoined_channel_mention,at_user,unjoined_channel_mention,at_channel,at_everyone,at_user_group,keyword,thread_v2,message_reaction,bot_dm_bundle,dm,prejoin_dm_welcome_party_alert,internal_channel_invite,external_channel_invite,external_dm_invite,quietly_added_to_channel,channel,saved_reminder,list_record_edited`.
    - Threads view: `thread_v2`. Mentions view: `at_user,unjoined_channel_mention,at_channel,at_everyone,at_user_group,keyword`.

    OBSERVED: emacs-slack/emacs-slack/slack-activity-feed-buffer.el:55-64

  - `mode` - string - sent by every client - `chrono_v1` (emacs-slack, gammons, clack) or `priority_reads_and_unreads_v1` (ericboehs default). OBSERVED: emacs-slack/emacs-slack/slack-activity-feed-buffer.el:127; gammons/slk/internal/slack/activity.go:131; ericboehs/slk/lib/slk/api/activity.rb:12; some-du6e/clack/docs/ACTIVITY.md:28-29
  - `limit` - int - optional - 20 (emacs-slack, clack) or 50 (ericboehs). OBSERVED: emacs-slack/emacs-slack/slack-activity-feed-buffer.el:125; ericboehs/slk/lib/slk/api/activity.rb:12
  - `cursor` - string - optional - previous `response_metadata.next_cursor`. OBSERVED: gammons/slk/internal/slack/activity.go:136-138
  - `unread_only` / `priority_only` / `archive_only` - bool - optional - Unreads toggle / VIP filter / archived ("cleared") items. OBSERVED: emacs-slack/emacs-slack/slack-activity-feed-buffer.el:128,131-132; some-du6e/clack/docs/ACTIVITY.md:40,507
  - `channel_ids` - list - optional. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:4
  - `channel_section_ids` - comma list - optional - section-scoped (e.g. starred) view. OBSERVED: emacs-slack/emacs-slack/slack-activity-feed-buffer.el:66-71,129-130; some-du6e/clack/docs/ACTIVITY.md:478
  - `is_activity_inbox` - bool - optional - emacs-slack sends `true`, ericboehs `false`. OBSERVED: emacs-slack/emacs-slack/slack-activity-feed-buffer.el:136; ericboehs/slk/lib/slk/api/activity.rb:22-23
  - `exclude_automations` / `automations_only` / `only_salesforce_channels` - bool - optional. OBSERVED: emacs-slack/emacs-slack/slack-activity-feed-buffer.el:133-135; some-du6e/clack/docs/ACTIVITY.md:28-31
  - `snooze_only` - bool - optional - ericboehs sends `false`. OBSERVED: ericboehs/slk/lib/slk/api/activity.rb:22
  - `_x_reason` - `fetchActivityFeed`. OBSERVED: emacs-slack/emacs-slack/slack-activity-feed-buffer.el:156
- **Response** (one item per `item.type` shape):
  ```json
  {
    "ok": true,
    "items": [
      {
        "is_unread": false,
        "is_bot": false,
        "is_archived": false,
        "version": "…",
        "feed_ts": "1784892914.789649",
        "key": "thread_v2-C0005TEST-1784891984.803759",
        "item": {
          "type": "thread_v2",
          "bundle_info": {
            "unread_count": 0,
            "payload": {
              "thread_entry": {
                "channel_id": "C…",
                "thread_ts": "…",
                "latest_ts": "…",
                "unread_msg_count": 0
              }
            }
          }
        }
      },
      {
        "item": {
          "type": "at_user",
          "message": {
            "ts": "…",
            "channel": "C…",
            "author_user_id": "U…",
            "thread_ts": "…",
            "is_broadcast": false
          }
        }
      },
      {
        "item": {
          "type": "message_reaction",
          "message": { "…": "…" },
          "reaction": { "user": "U…", "name": "eyes" }
        }
      },
      {
        "item": {
          "type": "dm",
          "bundle_info": {
            "payload": {
              "dm_entry": { "latest_message": { "ts": "…", "channel": "D…" } }
            }
          }
        }
      },
      {
        "item": {
          "type": "channel",
          "bundle_info": {
            "payload": { "channel_entry": { "latest_message": { "…": "…" } } }
          }
        }
      },
      {
        "item": {
          "type": "bot_dm_bundle",
          "bundle_info": {
            "payload": { "message": { "ts": "…", "channel": "D…" } }
          }
        }
      },
      {
        "item": {
          "type": "saved_reminder",
          "saved_reminder_payload": { "item_type": "…" }
        }
      },
      {
        "item": {
          "type": "generic_system_alert",
          "generic_system_alert_payload": { "click_target_id": "C…" }
        }
      }
    ],
    "response_metadata": { "next_cursor": "…" }
  }
  ```
  OBSERVED: gammons/slk/internal/slack/testdata/activity_feed.json:4-23; gammons/slk/internal/slack/activity.go:35-105,196-255; emacs-slack/emacs-slack/slack-activity-feed-buffer.el:170-214; some-du6e/clack/docs/ACTIVITY.md:54
- **Pagination:** cursor in `response_metadata.next_cursor`; empty on the last page. OBSERVED: gammons/slk/internal/slack/activity.go:42-44,177-180
- **Used for:** the Activity view. Tabs map to `types`, Unreads to `unread_only`, VIP to `priority_only`. Row message bodies are hydrated separately with `messages.list`. OBSERVED: gammons/slk/internal/slack/activity.go:107-125,315-326; emacs-slack/emacs-slack/slack-activity-feed-buffer.el:77-94

### activity.markRead

- **Transport:** `POST https://<ws>.slack.com/api/activity.markRead`, form body. OBSERVED: gammons/slk/internal/slack/activity.go:452-461
- **Visibility:** private (bundle only). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:8
- **Request params:** `entries` - JSON array - required - `[{"type":"thread_v2","key":"thread_v2-C…-…","feed_ts":"…"}]`. gammons notes all three fields are required, and every feed item carries them. OBSERVED: gammons/slk/internal/slack/activity.go:439-461
- **Response:** ok envelope. OBSERVED: gammons/slk/internal/slack/activity.go:461-465
- **Pagination:** none.
- **Used for:** "Mark as read" on an Activity row, and bulk select. OBSERVED: gammons/slk/internal/slack/activity.go:447-451; some-du6e/clack/docs/ACTIVITY.md:39,460

### activity.markUnread

- **Transport:** `POST https://<ws>.slack.com/api/activity.markUnread`, form body. OBSERVED: gammons/slk/internal/slack/activity.go:453-461
- **Visibility:** private (bundle only). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:9
- **Request params:** `entries` - JSON array - required - same shape as `activity.markRead`. OBSERVED: gammons/slk/internal/slack/activity.go:439-461; some-du6e/clack/docs/ACTIVITY.md:39
- **Response:** ok envelope. OBSERVED: gammons/slk/internal/slack/activity.go:461-465
- **Pagination:** none.
- **Used for:** "Mark as unread" on an Activity row. OBSERVED: gammons/slk/internal/slack/activity.go:447-451

### activity.views

- **Transport:** not observed. The sources show only that the official client calls it during boot (a call name and time in a boot sequence, with no verb, encoding or params) and that the client's `fetchActivityViews` reads it; the standard form POST (see Conventions) is presumed (see LEADS). OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:3701-3704; some-du6e/clack/docs/ACTIVITY.md:215-218
- **Visibility:** private (bundle only). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:12
- **Request params:** none observed.
- **Response:** the saved views (each carrying `density`) plus global `prefs {sort, density}`. Density is `dense` for Dense; anything else maps to Detailed (`compact`). OBSERVED: some-du6e/clack/docs/ACTIVITY.md:217-221
- **Pagination:** none observed.
- **Used for:** Activity inbox tabs / saved views and the Dense/Detailed layout. Writes go through `activity.views.update/create` and `activity.prefs.update` (see the names-only table below); realtime updates arrive as `activity_views_updated`. OBSERVED: some-du6e/clack/docs/ACTIVITY.md:222-228,503-505

### messages.list

- **Transport:** `POST https://<ws>.slack.com/api/messages.list`, form body. OBSERVED: gammons/slk/internal/slack/activity.go:382-392
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/messages.list
- **Request params:**
  - `message_ids` - JSON - required - `[{"channel":"C…","timestamps":["…","…"]}]`. OBSERVED: gammons/slk/internal/slack/activity.go:301-304,383-388
  - `org_wide_aware` - bool - sent `true`. OBSERVED: gammons/slk/internal/slack/activity.go:389
  - `cached_latest_updates` - JSON - sent `{}`. OBSERVED: gammons/slk/internal/slack/activity.go:390
- **Response:** `{"ok": true, "messages_data": {"C…": {"messages": [{"ts": "…", "user": "U…", "text": "…"}]}}}`; error `too_many_channels`. OBSERVED: gammons/slk/internal/slack/activity.go:291-299,407-437
- **Pagination:** none. Batch at most about 8 channels per call: one workspace accepted 9 and rejected 39. gammons splits a batch in half on `too_many_channels`. OBSERVED: gammons/slk/internal/slack/activity.go:306-313,371-380
- **Used for:** Activity row previews ("the same batch call the web client uses to fill the Activity feed's previews"). General message fetching belongs to the messages area. OBSERVED: gammons/slk/internal/slack/activity.go:315-321

### client.counts

- **Transport:** `POST https://<ws>.slack.com/api/client.counts`, multipart in the official client. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:1501-1519
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/client.counts
- **Request params:**
  - `thread_counts_by_channel` - bool - optional - adds per-channel thread counts (`threads.*_by_channel`). OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:1510,1526-1531; emacs-slack/emacs-slack/slack-counts.el:106; beeper/slackgo/users.go:757-759
  - `org_wide_aware` - bool - optional - sent `true` by the official client, slackdump and mautrix. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:1511; rusq/slackdump/internal/edge/client.go:56-62; mautrix/slack/pkg/connector/client.go:572-576
  - `include_file_channels` - bool - optional - adds the `file_channels` block. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:1512,1569-1576
  - `include_all_unreads` - bool - optional - sent `true` by the official client. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:1513,1648
  - `channel_ids` - optional - name only. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:286
  - `dry_run_last_fetched` - epoch seconds - optional - seen on the poll request only. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:1514
  - `_x_reason` - `fetchClientCountsOnConnect` (connect/boot) or `pollClientCountsAfterTimeout` (poll); slackdump sends `client-counts-api/fetchClientCounts`. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:1515,1649; rusq/slackdump/internal/edge/client.go:61
- **Response** (shape of a captured official call; ints shown as `0`):
  ```json
  {
    "ok": true,
    "threads": {
      "has_unreads": true,
      "mention_count": 0,
      "vip_count": 0,
      "mention_count_by_channel": {},
      "unread_count_by_channel": { "C47A0C3TK": 0 }
    },
    "channels": [
      {
        "id": "C…",
        "last_read": "<ts>",
        "latest": "<ts>",
        "updated": "<ts>",
        "history_invalid": "<ts>",
        "mention_count": 0,
        "has_unreads": true
      }
    ],
    "mpims": ["same item shape"],
    "ims": ["same item shape"],
    "file_channels": {
      "quip": { "mention_count_by_channel": {} },
      "list": { "mention_count_by_channel": {} }
    },
    "activity_v2": {
      "at_user": 0,
      "at_user_group": 0,
      "at_channel": 0,
      "at_everyone": 0,
      "channel": 0,
      "dm": 0,
      "keyword": 0,
      "message_reaction": 0,
      "internal_channel_invite": 0,
      "external_channel_invite": 0,
      "external_dm_invite": 0,
      "shared_workspace_invite": 0,
      "team_joiner_invite": 0,
      "unjoined_channel_mention": 0,
      "list_record_assigned": 0,
      "list_record_edited": 0,
      "list_user_mentioned": 0,
      "list_todo_notification": 0,
      "list_approval_request": 0,
      "list_approval_reviewed": 0,
      "bot_dm_bundle": 0,
      "thread_v2": 0,
      "saved_reminder": 0,
      "prejoin_dm_welcome_party_alert": 0,
      "quietly_added_to_channel": 0
    },
    "channel_badges": {
      "channels": 0,
      "dms": 0,
      "app_dms": 0,
      "thread_mentions": 0,
      "thread_unreads": 0
    },
    "alerts": {
      "external_channel_invite": 0,
      "external_dm_invite": 0,
      "shared_workspace_invite": 0,
      "sales_home_notification": 0,
      "lists_user_mentioned": 0,
      "lists_record_assigned": 0,
      "lists_record_edited": 0,
      "lists_todo_notification": 0,
      "lists_approval_request": 0,
      "lists_approval_reviewed": 0,
      "huddles_missed_huddle": 0,
      "calendar_upcoming_event": 0,
      "unjoined_channel_mention": 0,
      "generic_system_alert": 0,
      "prejoin_dm_welcome_party_alert": 0,
      "quietly_added_to_channel": 0
    },
    "saved": {
      "uncompleted_count": 0,
      "uncompleted_overdue_count": 0,
      "archived_count": 0,
      "completed_count": 0,
      "total_count": 0
    },
    "counts_last_fetched": 0
  }
  ```
  OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:1522-1636; wee-slack/wee-slack/typings/slack_api/slack_client_counts.pyi:8-49; rusq/slackdump/internal/edge/client.go:36-50. Conversation items in local captures carried `id`, `latest`, `last_read`, `mention_count`, `has_unreads`, `history_invalid` and `updated` across 264 channels and 38 ims. OBSERVED: ankarhem/slackless/.omo/research/findings.md:94-100
- **Pagination:** none; one call returns every conversation with read state.
- **Used for:**
  - The "All unreads" list, sidebar bold state and mention badges (`has_unreads`, `mention_count`), and last-read cursors (`last_read`, `latest`).
  - The Threads unread dot (`threads.has_unreads`), the Activity badge (`activity_v2`), the legacy badge math (`channel_badges`) and the overdue-Later badge (`saved.uncompleted_overdue_count`).
  - Reconnect catch-up. mautrix seeds its last-read cache from it.

  OBSERVED: emacs-slack/emacs-slack/slack-counts.el:49-69; mautrix/slack/pkg/connector/client.go:567-599; gammons/slk/internal/slack/client.go:1249-1255; gammons/slk/cmd/slk/reconnect_sync.go:35-37,62; ericboehs/slk/lib/slk/api/client.rb:24-29; 3kh0/super-platinum/docs/notification-presentation-protocol.md:500-506

- **Aggregation:**
  - emacs-slack's summary: per kind (channels/mpims/ims), total = sum of `mention_count` and unread = any `has_unreads`; threads come from the `threads` block. OBSERVED: emacs-slack/emacs-slack/slack-counts.el:49-69
  - A conversation with `has_unreads: true` and `mention_count: 0` must render a dot, not "1". OBSERVED: gammons/slk/internal/slack/client.go:1216-1220

### conversations.mark

- **Transport:** `POST https://<ws>.slack.com/api/conversations.mark`, form body. OBSERVED: https://docs.slack.dev/reference/methods/conversations.mark; wee-slack/wee-slack/slack/slack_api.py:441-447; dylex/slack-libpurple/slack-conversation.c:177
- **Visibility:** public; rate limit Tier 3. OBSERVED: https://docs.slack.dev/reference/methods/conversations.mark
- **Request params:**
  - `channel` - string - required - conversation to move the read cursor in. OBSERVED: https://docs.slack.dev/reference/methods/conversations.mark
  - `ts` - ts - required - message to record as most recently seen. OBSERVED: https://docs.slack.dev/reference/methods/conversations.mark
  - `reason` - string - optional - name only, from the bundle. The official client sends `_x_reason=viewed`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:336; gammons/slk/internal/slackhttp/reason.go:74
- **Response:** `{"ok": true}`. "The mark is saved to the database and broadcast via the message server to all open connections for the calling user", which produces the `*_marked` frames. OBSERVED: https://docs.slack.dev/reference/methods/conversations.mark
- **Pagination:** none.
- **Used for:**
  - Reading a channel, DM or MPDM. mautrix maps Matrix read receipts to it. OBSERVED: mautrix/slack/pkg/connector/handlematrix.go:400,418
  - "Mark unread": post an older `ts` to roll the watermark backwards. OBSERVED: gammons/slk/internal/slack/client.go:1584-1598
  - Catch-up / mark-all-read flows: `client.counts`, then `conversations.mark` per unread conversation. OBSERVED: ericboehs/slk/lib/slk/services/unread_marker.rb:19,68; ericboehs/slk/test/commands/catchup_test.rb:14-22
- **Legacy names:** emacs-slack still defines `channels.mark` and `im.mark` URLs. OBSERVED: emacs-slack/emacs-slack/slack-channel.el:39; emacs-slack/emacs-slack/slack-im.el:39

### users.prefs.get

- **Transport:** `POST` (official, multipart) or `GET https://<ws>.slack.com/api/users.prefs.get`. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2188-2202; beeper/slackgo/info.go:327-338; emacs-slack/emacs-slack/slack-user.el:540-545
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/users.prefs.get
- **Request params:**
  - `prefs` - comma list - optional - restricts the response to the named prefs (e.g. `all_notifications_prefs`, `emoji_use,frecency`). gammons omits it and reads `all_notifications_prefs` and `muted_channels` from the full response. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:196-202; 3kh0/super-platinum/docs/global-notifications-protocol.md:46-51; gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2197; gammons/slk/internal/slack/client.go:1677-1691
  - `_x_reason` - e.g. `fetch-frecency-prefs`, `fetch-user-expectation-last-triggered-pref`. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:2198,2220
- **Response** (`all_notifications_prefs` is a JSON-encoded string inside the JSON):
  ```json
  {
    "ok": true,
    "prefs": {
      "all_notifications_prefs": "{\"channels\":{\"C…\":{\"desktop\":\"mentions_dms\",\"mobile\":\"everything\",\"muted\":false,\"suppress_at_channel\":false,\"follow_all_threads\":false}},\"global\":{\"global_desktop\":\"everything\",\"global_mobile\":\"mentions_dms\",\"global_keywords\":\"deploy,outage\",\"threads_everything\":false}}",
      "muted_channels": "C1,C2",
      "highlight_words": "<string>",
      "dnd_enabled": true,
      "dnd_days": "weekdays",
      "dnd_start_hour": "22:00",
      "dnd_end_hour": "08:00",
      "mute_sounds": false,
      "activity_include_threads_in_badge_count": false,
      "mac_ssb_bullet": true
    }
  }
  ```
  OBSERVED: wee-slack/wee-slack/typings/slack_api/slack_users_prefs.pyi:6-39; gammons/slk/internal/slack/client.go:1683-1691,1719-1744; beeper/slackgo/info.go:258,278; 3kh0/super-platinum/docs/notification-schedule-protocol.md:59-70; 3kh0/super-platinum/docs/notification-presentation-protocol.md:34-36
- **Decoded `all_notifications_prefs` keys:**

  | Scope          | Key                                                               | Values / meaning                                                        | OBSERVED                                                                                                                                |
  | -------------- | ----------------------------------------------------------------- | ----------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
  | `channels[id]` | `desktop`                                                         | `everything`, `mentions_dms`, legacy `nothing` (read as `mentions_dms`) | wee-slack/wee-slack/typings/slack_api/slack_users_prefs.pyi:6-9; 3kh0/super-platinum/docs/channel-notifications-protocol.md:25,111-117  |
  | `channels[id]` | `mobile`                                                          | same enum; `nothing` turns mobile off                                   | 3kh0/super-platinum/docs/channel-notifications-protocol.md:70-71                                                                        |
  | `channels[id]` | `muted`                                                           | bool                                                                    | wee-slack/wee-slack/typings/slack_api/slack_users_prefs.pyi:12; gammons/slk/internal/slack/client.go:1719-1744                          |
  | `channels[id]` | `suppress_at_channel`                                             | bool; the UI "broadcast mentions" checkbox is its inverse               | 3kh0/super-platinum/docs/channel-notifications-protocol.md:73                                                                           |
  | `channels[id]` | `follow_all_threads`                                              | bool ("Follow every thread")                                            | wee-slack/wee-slack/typings/slack_api/slack_users_prefs.pyi:11; 3kh0/super-platinum/docs/channel-notifications-protocol.md:72           |
  | `channels[id]` | `desktop_push_enabled`                                            | bool device switch, independent of the message filter                   | 3kh0/super-platinum/docs/channel-notifications-protocol.md:69                                                                           |
  | `channels[id]` | `badge_all_unreads`                                               | bool ("Badge every new message")                                        | 3kh0/super-platinum/docs/channel-notifications-protocol.md:74                                                                           |
  | `global`       | `global_desktop`                                                  | `everything` / `mentions_dms` (legacy `nothing` read as Mentions)       | 3kh0/super-platinum/docs/global-notifications-protocol.md:71                                                                            |
  | `global`       | `global_mobile`                                                   | `everything` / `mentions_dms` / `nothing`                               | 3kh0/super-platinum/docs/global-notifications-protocol.md:72                                                                            |
  | `global`       | `global_desktop_push_enabled`                                     | bool                                                                    | 3kh0/super-platinum/docs/global-notifications-protocol.md:73                                                                            |
  | `global`       | `global_keywords`                                                 | comma-separated keywords (highlight words); empty clears                | 3kh0/super-platinum/docs/global-notifications-protocol.md:74; wee-slack/wee-slack/typings/slack_api/slack_users_prefs.pyi:18            |
  | `global`       | `threads_everything`                                              | bool ("Thread replies")                                                 | 3kh0/super-platinum/docs/global-notifications-protocol.md:75                                                                            |
  | `global`       | `global_mpdm_desktop`, `global_mpdm_mobile`                       | DM / group-DM defaults                                                  | 3kh0/super-platinum/docs/global-notifications-protocol.md:76; wee-slack/wee-slack/typings/slack_api/slack_users_prefs.pyi:20-21         |
  | `global`       | `desktop_sound`, `priority_desktop_sound`, `mobile_sound`         | sound filename or `none`; VIP `none` means "same as messages"           | 3kh0/super-platinum/docs/notification-presentation-protocol.md:31-32; wee-slack/wee-slack/typings/slack_api/slack_users_prefs.pyi:16,22 |
  | `global`       | `no_text_in_notifications`, `push_show_preview`, `push_idle_wait` | preview hiding, mobile preview, mobile idle delay                       | 3kh0/super-platinum/docs/notification-presentation-protocol.md:33; wee-slack/wee-slack/typings/slack_api/slack_users_prefs.pyi:23-25    |

- **Other flat prefs in this area:**
  - Legacy `muted_channels` and `highlight_words`. OBSERVED: beeper/slackgo/info.go:258,278
  - The schedule keys `dnd_*` (see `users.prefs.set`). OBSERVED: 3kh0/super-platinum/docs/notification-schedule-protocol.md:59-70
  - `mute_sounds`, `activity_include_threads_in_badge_count`, `mac_ssb_bullet`, `mac_ssb_bounce`. OBSERVED: 3kh0/super-platinum/docs/notification-presentation-protocol.md:34-37
  - `activity_dms_opt_out`, `global_reminders_in_activity`, `tab_rail_customization`. OBSERVED: 3kh0/super-platinum/docs/notification-presentation-protocol.md:483,500-510
  - slackgo also models the older flat push prefs (`push_dm_alert`, `push_mention_alert`, `push_everything`, `loud_channels`, `never_channels`, `at_channel_suppressed_channels`, `all_channels_loud`). OBSERVED: beeper/slackgo/info.go:259-278
- **Pagination:** none.
- **Used for:** the notification settings UI, mute state, keyword highlighting and schedule display. wee-slack also reads `all_notifications_prefs` from `client.userBoot`'s `prefs` (cross-area). OBSERVED: wee-slack/wee-slack/slack/slack_workspace.py:405-447,450-452,498-499

### users.prefs.set

- **Transport:** `POST https://<ws>.slack.com/api/users.prefs.set`, form body. OBSERVED: wee-slack/wee-slack/slack/slack_api.py:569-575; 3kh0/super-platinum/docs/notification-presentation-protocol.md:75-80
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/users.prefs.set; 3kh0/slack-datamine/build/api-args.txt:889
- **Request params:**
  - `name` + `value` - scalar form. Examples: `name=mute_sounds value=true _x_reason=prefs-api/setUserPrefByApi`; wee-slack writes `name=muted_channels value=C1,C2`. OBSERVED: 3kh0/super-platinum/docs/notification-presentation-protocol.md:72-86; wee-slack/wee-slack/slack/slack_api.py:569-575
  - `prefs` - JSON object - multi form - a patch such as `{"dnd_days":"weekdays","dnd_enabled":true}`, sent with `_x_reason=prefs-api/setMultipleUserPrefsByApi`. It is an object, not the `[{name,value}]` array that `users.prefs.setNotifications` uses. OBSERVED: 3kh0/super-platinum/docs/notification-schedule-protocol.md:86-112
  - Legacy slackgo form: `muted_channels=<list>&reason=update-muted-channels`. OBSERVED: beeper/slackgo/info.go:352-356
- **Schedule ("notification hours") keys** (wall-clock strings, never timezone-converted):
  - `dnd_enabled` bool; every captured schedule write includes `true`.
  - `dnd_days` = `every_day` | `weekdays` | `custom`.
  - Global quiet hours: `dnd_start_hour` / `dnd_end_hour` as `"HH:MM"`.
  - Custom days: `dnd_enabled_<day>` = `all_day` | `disabled` | `partial`, plus `dnd_before_<day>` / `dnd_after_<day>`.
  - The UI's allowed-from "Start" writes `dnd_end_hour` / `dnd_before_<day>`, and "End" writes `dnd_start_hour` / `dnd_after_<day>`.

  OBSERVED: 3kh0/super-platinum/docs/notification-schedule-protocol.md:42-46,59-70,142-147

- **Response:** slackgo decodes `{ok, prefs}`; super-platinum reconciles with a fresh `users.prefs.get` instead of trusting the write response. OBSERVED: beeper/slackgo/info.go:354-361; 3kh0/super-platinum/docs/notification-presentation-protocol.md:84-88
- **Pagination:** none.
- **Used for:** notification schedule, "Mute all messaging sounds", "Include thread replies in Activity badge", and macOS bullet/bounce. OBSERVED: 3kh0/super-platinum/docs/notification-presentation-protocol.md:34-37,82; 3kh0/super-platinum/docs/notification-schedule-protocol.md:52

### users.prefs.setNotifications

- **Transport:** `POST https://<ws>.slack.com/api/users.prefs.setNotifications`, form body. OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:42-51,59-67
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/methods/users.prefs.setNotifications; 3kh0/slack-datamine/build/api-args.txt:890
- **Request params:**
  - `global` - bool - sent on every capture - `true` targets `all_notifications_prefs.global` (user-wide defaults), `false` a channel override. OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:44,63; 3kh0/super-platinum/docs/global-notifications-protocol.md:53-56
  - `name`, `value` - single-pref form; booleans travel as strings. OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:59-67; 3kh0/super-platinum/docs/global-notifications-protocol.md:155-170
  - `prefs` - JSON array `[{"name":…,"value":…,"sync":bool}]` - multi-pref form. OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:47-49; 3kh0/super-platinum/docs/global-notifications-protocol.md:145-146
  - `channel_id` (single form) / `channel_ids` (multi form; a single raw id is accepted). OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:45,53,64
  - `sync` - bool - top-level; with `true`, the desktop change mirrors to mobile. Multi-global calls omit it. OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:37-38,46; 3kh0/super-platinum/docs/global-notifications-protocol.md:150-153
- **Examples (verbatim from the captured client):**
  ```text
  # channel mode "All new posts"
  global=false  channel_ids=C_SYNTHETIC  sync=true
  prefs=[{"name":"desktop","value":"everything","sync":true},
         {"name":"follow_all_threads","value":"false","sync":false},
         {"name":"badge_all_unreads","value":"false","sync":false}]
  _x_reason=prefs-store/setMultiChannelNotificationOverride
  # one channel toggle
  name=desktop_push_enabled  value=false  channel_id=C_SYNTHETIC  global=false  sync=false
  _x_reason=prefs-store/setChannelNotificationOverride
  # reset a channel to defaults (removes the whole override object)
  channel_id=C_SYNTHETIC  name=reset  value=default  global=false  sync=false
  _x_reason=prefs-store/resetChannelNotificationOverride
  # global default with linked mobile
  global=true  prefs=[{"name":"global_desktop","value":"everything"},{"name":"global_mobile","value":"everything"}]
  _x_reason=NotificationsChannelDefaults
  # highlight keywords
  name=global_keywords  value=<comma list, may be empty>  global=true  sync=false
  _x_reason=NotificationsKeywordsInput
  # sound (the captured client sends _x_reason empty)
  name=desktop_sound  value=b2.mp3  global=true  sync=false
  _x_reason=
  ```
  OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:42-51,59-67,153-170; 3kh0/super-platinum/docs/global-notifications-protocol.md:140-181; 3kh0/super-platinum/docs/notification-presentation-protocol.md:55-63
- **Presets:**
  - "Just mentions" = `desktop=mentions_dms`, `follow_all_threads=false`, `suppress_at_channel=false`.
  - Choosing a mode on a muted channel also appends `muted=false`, which is the per-channel mute key on this same method.

  OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:34-36,53

- **Response:** not captured in the sources read. Confirm with `users.prefs.get(prefs=all_notifications_prefs)` or the `pref_change` frame. OBSERVED: 3kh0/super-platinum/docs/global-notifications-protocol.md:46-51; gammons/slk/internal/service/mutestore.go:106-117
- **Pagination:** none.
- **Used for:** per-channel notification settings (mute, mode, mobile override, thread follow, @channel suppression, badge), global defaults, keywords, sounds and preview. OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:12-23

### dnd.info

- **Transport:** `POST https://<ws>.slack.com/api/dnd.info` (the docs list GET; POST works for ericboehs and slackgo). OBSERVED: https://docs.slack.dev/reference/methods/dnd.info; ericboehs/slk/lib/slk/api/dnd.rb:12-14; beeper/slackgo/dnd.go:111-126
- **Visibility:** public; Tier 3. OBSERVED: https://docs.slack.dev/reference/methods/dnd.info
- **Request params:**
  - `user` - string - optional - defaults to self. OBSERVED: https://docs.slack.dev/reference/methods/dnd.info
  - `team_id` - string - optional - needed with org tokens; slackgo adds it after a workspace migration returns `missing_argument: team_id`. OBSERVED: https://docs.slack.dev/reference/methods/dnd.info; beeper/slackgo/dnd.go:10-16
  - `_x_reason` - `fetchAndUpsertDndForUsers-getDndTimesFor:self`. OBSERVED: gammons/slk/internal/slackhttp/reason.go:78
- **Response:**
  ```json
  {
    "ok": true,
    "dnd_enabled": true,
    "next_dnd_start_ts": 1450416600,
    "next_dnd_end_ts": 1450452600,
    "snooze_enabled": true,
    "snooze_endtime": 1450416600,
    "snooze_remaining": 1196,
    "snooze_is_indefinite": false
  }
  ```
  `snooze_*` fields appear only for the current user; `snooze_endtime` and `snooze_remaining` appear only while `snooze_enabled`. OBSERVED: https://docs.slack.dev/reference/methods/dnd.info
- **Pagination:** none.
- **Used for:** own DND/snooze state at boot (the official boot sequence calls it), and refetching after `dnd_invalidated`. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:3713-3716; gammons/slk/internal/slack/events.go:747-757

### dnd.setSnooze

- **Transport:** `POST https://<ws>.slack.com/api/dnd.setSnooze`, form body. The docs accept url-encoded or JSON; slackgo sends a url-encoded form, while ericboehs sends a JSON body with `Authorization: Bearer`. OBSERVED: https://docs.slack.dev/reference/methods/dnd.setSnooze; beeper/slackgo/dnd.go:46-48,174-184; beeper/slackgo/slack.go:271-272; ericboehs/slk/lib/slk/api/dnd.rb:16-19; ericboehs/slk/lib/slk/services/api_client.rb:62-71
- **Visibility:** public; Tier 2. OBSERVED: https://docs.slack.dev/reference/methods/dnd.setSnooze
- **Request params:** `num_minutes` - string/int - required - minutes from now. OBSERVED: https://docs.slack.dev/reference/methods/dnd.setSnooze
- **Response:** `{"ok": true, "snooze_enabled": true, "snooze_endtime": 1450373897, "snooze_remaining": 60, "snooze_is_indefinite": false}`; `snooze_remaining` is in seconds. OBSERVED: https://docs.slack.dev/reference/methods/dnd.setSnooze
- **Pagination:** none.
- **Used for:** "Pause notifications" for a duration; it also extends an active snooze. OBSERVED: https://docs.slack.dev/reference/methods/dnd.setSnooze; 3kh0/super-platinum/docs/notification-schedule-protocol.md:15-16

### dnd.endSnooze

- **Transport:** `POST https://<ws>.slack.com/api/dnd.endSnooze`. OBSERVED: https://docs.slack.dev/reference/methods/dnd.endSnooze; ericboehs/slk/lib/slk/api/dnd.rb:21-23
- **Visibility:** public; Tier 2. OBSERVED: https://docs.slack.dev/reference/methods/dnd.endSnooze
- **Request params:** none beyond the token. OBSERVED: https://docs.slack.dev/reference/methods/dnd.endSnooze
- **Response:** `{"ok": true, "dnd_enabled": true, "next_dnd_start_ts": 1450418400, "next_dnd_end_ts": 1450454400, "snooze_enabled": false}`. OBSERVED: https://docs.slack.dev/reference/methods/dnd.endSnooze
- **Pagination:** none.
- **Used for:** "Resume notifications". OBSERVED: 3kh0/super-platinum/docs/notification-schedule-protocol.md:15-16

### dnd.endDnd

- **Transport:** `POST https://<ws>.slack.com/api/dnd.endDnd`. OBSERVED: https://docs.slack.dev/reference/methods/dnd.endDnd; beeper/slackgo/dnd.go:63-74
- **Visibility:** public; Tier 2. OBSERVED: https://docs.slack.dev/reference/methods/dnd.endDnd
- **Request params:** none beyond the token. OBSERVED: https://docs.slack.dev/reference/methods/dnd.endDnd
- **Response:** `{"ok": true}`. OBSERVED: https://docs.slack.dev/reference/methods/dnd.endDnd
- **Pagination:** none.
- **Used for:** ending the currently scheduled DND session immediately. OBSERVED: https://docs.slack.dev/reference/methods/dnd.endDnd

### dnd.teamInfo

- **Transport:** `GET https://<ws>.slack.com/api/dnd.teamInfo` per the docs and emacs-slack, whose request sets no `:type` and so takes `slack-request`'s default `GET` (with `users` in the query string). slackgo sends `POST` with a url-encoded form. OBSERVED: https://docs.slack.dev/reference/methods/dnd.teamInfo; emacs-slack/emacs-slack/slack-dnd-status.el:33,73-77; emacs-slack/emacs-slack/slack-request.el:69; beeper/slackgo/dnd.go:140-154
- **Visibility:** public; Tier 2. OBSERVED: https://docs.slack.dev/reference/methods/dnd.teamInfo
- **Request params:**
  - `users` - comma list - required - up to 50 users. OBSERVED: https://docs.slack.dev/reference/methods/dnd.teamInfo; emacs-slack/emacs-slack/slack-dnd-status.el:77
  - `team_id` - string - optional - needed with org tokens. OBSERVED: https://docs.slack.dev/reference/methods/dnd.teamInfo
- **Response:** `{"ok": true, "users": {"U023BECGF": {"dnd_enabled": true, "next_dnd_start_ts": 1450387800, "next_dnd_end_ts": 1450423800}}}`. OBSERVED: https://docs.slack.dev/reference/methods/dnd.teamInfo; emacs-slack/emacs-slack/slack-dnd-status.el:35-44
- **Pagination:** none; batch at most 50 users.
- **Used for:** other members' DND indicators. The official boot sequence calls it. OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:3835

### Names-only methods (client bundle and bundle traces; wire behaviour unverified)

- **Transport:** presumably the standard form POST (see Conventions); not verified for these.
- **Visibility:** private (bundle only).
- **Response and pagination:** not observed.
- **Caveat:** the datamine's argument lists are what its extractor found and can be incomplete. For example, its `activity.feed` entry lacks `is_activity_inbox`, which clients send. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:4; emacs-slack/emacs-slack/slack-activity-feed-buffer.el:121-136

| Method                                 | Args observed                                                                             | Observed context                                           | OBSERVED                                                                               |
| -------------------------------------- | ----------------------------------------------------------------------------------------- | ---------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| `activity.archive`                     | `entries, key, ts, type`                                                                  | Activity "Clear"                                           | 3kh0/slack-datamine/build/api-args.txt:1; some-du6e/clack/docs/ACTIVITY.md:40          |
| `activity.unarchive`                   | `key, ts, type`                                                                           | Activity "Restore"                                         | 3kh0/slack-datamine/build/api-args.txt:6; some-du6e/clack/docs/ACTIVITY.md:40          |
| `activity.markAllRead`                 | `filters, types`                                                                          | mark everything read                                       | 3kh0/slack-datamine/build/api-args.txt:5; some-du6e/clack/docs/ACTIVITY.md:41          |
| `activity.clearBefore`                 | `before_ts` (+ `filters`: `entry_types`, `channel_ids`, `unread_only`, `archive_only`, …) | clear older items                                          | 3kh0/slack-datamine/build/api-args.txt:3; some-du6e/clack/docs/ACTIVITY.md:41-44       |
| `activity.clearAll`                    | `filters, undo_key`                                                                       | Undo on the "Notifications cleared" toast sends `undo_key` | 3kh0/slack-datamine/build/api-args.txt:2; some-du6e/clack/docs/ACTIVITY.md:400-403     |
| `activity.prefs.update`                | `density` (`dense` / `compact`)                                                           | global Activity layout                                     | some-du6e/clack/docs/ACTIVITY.md:222-225; 3kh0/slack-datamine/build/api-methods.txt:10 |
| `activity.views.create`                | `name, sort, density, filters`                                                            | new / suggested view                                       | some-du6e/clack/docs/ACTIVITY.md:491; 3kh0/slack-datamine/build/api-methods.txt:13     |
| `activity.views.update`                | `view_id, sort, density, next_view_id`                                                    | save / reorder a view                                      | some-du6e/clack/docs/ACTIVITY.md:411,650; 3kh0/slack-datamine/build/api-methods.txt:17 |
| `activity.views.delete`                | `view_id`                                                                                 | delete a view                                              | 3kh0/slack-datamine/build/api-args.txt:7                                               |
| `activity.views.hide` / `.unhide`      | `view_type`                                                                               | hide / unhide a default tab                                | 3kh0/slack-datamine/build/api-args.txt:8-9; some-du6e/clack/docs/ACTIVITY.md:485       |
| `activity.mark`, `activity.getEntries` | none extracted                                                                            | -                                                          | 3kh0/slack-datamine/build/api-methods.txt:5-6                                          |
| `subscriptions.thread.getTimestamps`   | `cursor, fetch_threads_state, limit, mode, priority_mode`                                 | -                                                          | 3kh0/slack-datamine/build/api-args.txt:822                                             |
| `conversations.badgeCount`             | `channel`                                                                                 | -                                                          | 3kh0/slack-datamine/build/api-args.txt:302                                             |
| `unread.collapse` / `unread.expand`    | `channel`                                                                                 | -                                                          | 3kh0/slack-datamine/build/api-args.txt:851-852                                         |
| `unread.markRead`                      | `markers`                                                                                 | -                                                          | 3kh0/slack-datamine/build/api-args.txt:853                                             |

## Websocket events

The web client receives these frames on `wss://wss-primary.slack.com/...`, opened with URL flags such as `lazy_channels=1` and `no_query_on_subscribe=1` (OBSERVED: ankarhem/slackless/.omo/research/findings.md:21,108-109). Each frame is a JSON object with a top-level `type`; slackgo's event map lists every type below except `update_thread_state`, `mpim_marked` and `dnd_invalidated` (OBSERVED: beeper/slackgo/websocket_managed_conn.go:581-686). The socket does not replay missed frames, so reconcile with `client.counts` after every reconnect or wake (OBSERVED: gammons/slk/cmd/slk/thread_subscriptions.go:131-138; gammons/slk/cmd/slk/reconnect_sync.go:62).

### thread_marked / thread_subscribed / thread_unsubscribed

- **Payload:**
  ```json
  {
    "type": "thread_marked",
    "subscription": {
      "type": "thread",
      "channel": "C…",
      "thread_ts": "…",
      "date_create": 1700000000,
      "active": true,
      "last_read": "…"
    },
    "event_ts": "…"
  }
  ```
  `thread_subscribed` and `thread_unsubscribed` carry the same `subscription` block. OBSERVED: wee-slack/wee-slack/typings/slack_rtm/slack_rtm_message.pyi:261-289; beeper/slackgo/websocket_channels.go:89-106; beeper/slackgo/websocket_managed_conn.go:682-684
- **When:** `thread_marked` fires when a thread read cursor moves, including the echo of the user's own `subscriptions.thread.mark`. The other two fire when follow state changes. OBSERVED: gammons/slk/internal/cache/thread_subscriptions.go:72-94
- **Handling:**
  - `subscription.active` means "subscribed for unread updates" and is not a read/unread signal.
  - For `thread_unsubscribed`, treat `active` as false whatever the inner flag says.

  OBSERVED: gammons/slk/internal/slack/events.go:480-507,777-810

- **Non-thread subscriptions:** `subscription.type` can be something other than `thread`; wee-slack ignores those frames. OBSERVED: wee-slack/wee-slack/typings/slack_rtm/slack_rtm_message.pyi:269-271; wee-slack/wee-slack/slack/slack_workspace.py:757-762,846-864
- **Field-name conflict:** wee-slack types the timestamp as `date_create`, slackgo as `date_created`. OBSERVED: wee-slack/wee-slack/typings/slack_rtm/slack_rtm_message.pyi:265; beeper/slackgo/websocket_channels.go:93
- **Client handling:**
  - mautrix turns `thread_marked` into a threaded read receipt and drops it when `last_read` is empty. OBSERVED: mautrix/slack/pkg/connector/handleslack.go:334-341
  - emacs-slack refetches `client.counts` on every `thread_marked`. OBSERVED: emacs-slack/emacs-slack/slack-websocket.el:714-719
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/events/thread_marked; https://docs.slack.dev/reference/events/thread_unsubscribed

### update_thread_state

- **Payload:** `{"type": "update_thread_state", "has_unreads": true, "mention_count": 2}`. emacs-slack writes these two fields into its `client.counts` threads rollup. OBSERVED: emacs-slack/emacs-slack/slack-websocket.el:533-541; emacs-slack/emacs-slack/slack-counts.el:221-225
- **When:** not documented in the sources. clack's own event API lists it among the frame types it passes through raw; whether current servers still send it is unverified (see LEADS). OBSERVED: some-du6e/clack/docs/NATIVE.md:204-214
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/events/update_thread_state

### channel_marked / group_marked / mpim_marked / im_marked

- **Payload:**
  ```json
  {"type": "channel_marked", "channel": "C…", "ts": "1401383885.000061",
   "unread_count": 0, "unread_count_display": 0, "num_mentions": 0, "num_mentions_display": 0,
   "mention_count": 0, "mention_count_display": 0, "event_ts": "…"}
  {"type": "im_marked", "channel": "D…", "ts": "…", "dm_count": 0, "unread_count_display": 0,
   "num_mentions_display": 0, "mention_count_display": 0, "event_ts": "…"}
  ```
  `group_marked` adds `is_mpim: false` and `mpim_marked` adds `is_mpim: true`. OBSERVED: wee-slack/wee-slack/typings/slack_rtm/slack_rtm_message.pyi:225-259; https://docs.slack.dev/reference/events/channel_marked
- **When:** after `conversations.mark` from any of the user's clients (broadcast to all open connections). OBSERVED: https://docs.slack.dev/reference/methods/conversations.mark
- **Client handling:**
  - Set `last_read = ts`. OBSERVED: wee-slack/wee-slack/slack/slack_workspace.py:839-845; mautrix/slack/pkg/connector/handleslack.go:330-349
  - gammons also updates the badge from `unread_count_display` and `mention_count`; an absent `mention_count` decodes to 0 and clears the badge. OBSERVED: gammons/slk/internal/slack/events.go:420-432,759-766
- **Visibility:** `channel_marked`, `im_marked` and `group_marked` are public (RTM); `mpim_marked` has no docs page. OBSERVED: https://docs.slack.dev/reference/events/im_marked; https://docs.slack.dev/reference/events/group_marked; https://docs.slack.dev/reference/events/mpim_marked

### badge_counts_updated

- **Payload:** `{"type": "badge_counts_updated", "activity_v2": {"at_user": 1, "thread_v2": 2, "…": "same keys as client.counts.activity_v2"}}`. slackgo's struct models only `type`. OBSERVED: gammons/slk/internal/slack/events.go:892-904; beeper/slackgo/websocket_misc.go:294-298
- **When:** the server recalculates the Activity badge, e.g. a new mention arrives or an item is read elsewhere (gammons' description). OBSERVED: gammons/slk/internal/slack/events.go:893-897
- **Official handling** (bundle trace):
  - Replace the stored `activity_v2` map; never merge it.
  - If the raw values sum to 0, mark loaded unread root-feed items read locally, with no API write.
  - The legacy (non-Inbox) branch updates `counts` and `alerts`.

  OBSERVED: 3kh0/super-platinum/docs/notification-presentation-protocol.md:442-459; some-du6e/clack/docs/ACTIVITY.md:1162-1165

- **Ordering:** the frame can supersede an in-flight `client.counts`; gammons timestamps arrivals and drops older HTTP snapshots. OBSERVED: gammons/slk/cmd/slk/workspace.go:96-100
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/events/badge_counts_updated

### activity

- **Payload:** `{"type": "activity", "subtype": "activity_updated", "key": "thread_v2-C…-…", "entry": {"…": "feed item"}, "event_ts": "…"}`. OBSERVED: beeper/slackgo/websocket_misc.go:283-292
- **Subtypes:**
  - `activity_updated` - patch the entry.
  - `activity_deleted` - drop it.
  - `activity_invalidated` - refetch.

  OBSERVED: some-du6e/clack/docs/ACTIVITY.md:45-47

- **Also handled by the same handler** (whether as `activity` subtypes or as their own frame types is unverified; see LEADS):
  - `activity_clear_all_completed` - carries `undo_key` and shows an undo toast; undo calls `activity.clearAll{undo_key}`.
  - `activity_views_updated` - `{views}` replaces the saved views.

  OBSERVED: some-du6e/clack/docs/ACTIVITY.md:45-49,400-403,503-505

### desktop_notification

- **Payload:**
  ```json
  {
    "type": "desktop_notification",
    "title": "…",
    "subtitle": "…",
    "msg": "…",
    "content": "…",
    "channel": "C…",
    "ts": "…",
    "launchUri": "…",
    "avatarImage": "…",
    "ssbFilename": "…",
    "imageUri": "…",
    "is_shared": false,
    "is_channel_invite": false,
    "event_ts": "…"
  }
  ```
  OBSERVED: beeper/slackgo/websocket_desktop_notification.go:3-19; beeper/slackgo/rtm.go:23
- **When:** not documented. mautrix, running on a session token, lists it among frames it receives and ignores as "duplicates or do not contain useful information". OBSERVED: mautrix/slack/pkg/connector/handleslack.go:156-160
- **Visibility:** private (docs 404). OBSERVED: https://docs.slack.dev/reference/events/desktop_notification
- **Client-side alternative**, as implemented elsewhere:
  - gammons evaluates in order:
    1. Never notify for own messages.
    2. Suppress during DND or snooze.
    3. Suppress muted conversations.
    4. Suppress the channel being viewed on the active workspace.
    5. Notify for DMs and group DMs.
    6. Notify for an @mention of self.
    7. Notify on a case-insensitive keyword substring match.

    OBSERVED: gammons/slk/internal/notify/notifier.go:56-114

  - wee-slack compiles `global_keywords` into a case-insensitive `\b(?:kw1|kw2)\b` regex. OBSERVED: wee-slack/wee-slack/slack/slack_workspace.py:434-445
  - The official settings UI computes delivery as `!macOSDisabled && browserPermissionAllowed && effectiveDesktopPush`, where a channel override beats the global value. OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:22-24,176-184
  - The effective mode normalises legacy `nothing` to `mentions_dms`. DM/MPDM defaults come from `global_mpdm_*`, and an absent value means Everything. OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:25; 3kh0/super-platinum/docs/global-notifications-protocol.md:97-100

### mobile_in_app_notification

- **Payload:**
  ```json
  {
    "type": "mobile_in_app_notification",
    "title": "…",
    "subtitle": "…",
    "ts": "…",
    "channel": "C…",
    "avatarImage": "…",
    "is_shared": false,
    "channel_name": "…",
    "author_id": "U…",
    "author_display_name": "…",
    "msg_text": "…",
    "push_id": "…",
    "notif_id": "…",
    "mobileLaunchUri": "…",
    "event_ts": "…"
  }
  ```
  OBSERVED: beeper/slackgo/websocket_mobile_in_app_notification.go:3-20; beeper/slackgo/websocket_managed_conn.go:678
- **When:** not documented in the sources read.

### pref_change

- **Payload:** `{"type": "pref_change", "name": "all_notifications_prefs", "value": "{\"channels\":{…},\"global\":{…}}"}`; the public example is `{"name": "messages_theme", "value": "dense"}`. OBSERVED: https://docs.slack.dev/reference/events/pref_change; wee-slack/wee-slack/typings/slack_rtm/slack_rtm_message.pyi:320-324
- **Value type:** `value` is polymorphic:
  - string for scalar prefs (`muted_channels` is a comma list);
  - array for list prefs (`highlight_words`);
  - object for nested prefs.

  OBSERVED: gammons/slk/internal/slack/events.go:442-478

- **When:** after any user-pref mutation. `all_notifications_prefs` and `muted_channels` carry the full new value, so replace rather than merge. OBSERVED: gammons/slk/internal/service/mutestore.go:106-117; wee-slack/wee-slack/slack/slack_workspace.py:695-700
- **Visibility:** public (RTM). OBSERVED: https://docs.slack.dev/reference/events/pref_change

### dnd_updated / dnd_updated_user / dnd_invalidated

- **Payloads:**
  ```json
  {"type": "dnd_updated", "user": "U1234", "dnd_status": {"dnd_enabled": true, "next_dnd_start_ts": 1450387800,
    "next_dnd_end_ts": 1450423800, "snooze_enabled": true, "snooze_endtime": 1450373897}}
  {"type": "dnd_updated_user", "user": "U123ABC456", "dnd_status": {"dnd_enabled": true,
    "next_dnd_start_ts": 1450387800, "next_dnd_end_ts": 1450423800}}
  {"type": "dnd_invalidated", "event_ts": "…", "user": {"id": "U…"}}
  ```
  OBSERVED: https://docs.slack.dev/reference/events/dnd_updated; https://docs.slack.dev/reference/events/dnd_updated_user; gammons/slk/internal/slack/events.go:410-418
- **When:**
  - `dnd_updated` - the current user's DND or snooze changed.
  - `dnd_updated_user` - another member's DND changed; it carries no snooze fields.
  - `dnd_invalidated` - refetch that user's DND.

  OBSERVED: https://docs.slack.dev/reference/events/dnd_updated_user; gammons/slk/internal/slack/events.go:728-757

- **Effective DND** (gammons): snoozed if `snooze_enabled && snooze_endtime > now`; otherwise scheduled DND if `dnd_enabled && next_dnd_start_ts > 0 && next_dnd_start_ts <= now < next_dnd_end_ts`. OBSERVED: gammons/slk/internal/slack/events.go:370-376,954-962
- **emacs-slack:** ignores `dnd_updated` (commented out) and stores `dnd_updated_user` per user. OBSERVED: emacs-slack/emacs-slack/slack-websocket.el:510-512,741-745
- **Visibility:** `dnd_updated` and `dnd_updated_user` are public (Events + RTM). OBSERVED: https://docs.slack.dev/reference/events/dnd_updated

## Gotchas

- **Session tokens only.** `subscriptions.thread.mark` requires a session token (OBSERVED: wee-slack/wee-slack/slack/slack_thread.py:127-129), and so does `subscriptions.thread.add` (OBSERVED: ericboehs/slk/lib/slk/api/threads.rb:32).
- **`getView` is expensive.**
  - A full sweep is about 62 requests per workspace up to a 1000-item hard cap, measured at 132 s on a 105-channel workspace. (OBSERVED: gammons/slk/cmd/slk/thread_subscriptions.go:28-35,131-139; gammons/slk/internal/slack/client.go:2045-2048)
  - gammons limits it to one sweep per workspace per 30 min and staggers Grid workspaces 15 s apart. (OBSERVED: gammons/slk/cmd/slk/thread_subscriptions.go:139-150)
  - Never echo `max_ts` as the cursor. (OBSERVED: gammons/slk/internal/slack/client.go:2059-2062)
- **Debounce `conversations.mark`.** The docs say not to call it often and to set a timer before marking; it is Tier 3. Expect your own `*_marked` echo. (OBSERVED: https://docs.slack.dev/reference/methods/conversations.mark)
- **Own-action echoes.** Your own thread marks echo as `thread_marked`; an echo does not prove a subscription. (OBSERVED: gammons/slk/internal/cache/thread_subscriptions.go:88-94)
- **Grid duplicate frames.** The same frame arrives on every workspace socket. gammons dedupes by frame within a TTL, except per-workspace-state frames:
  - `pref_change`, `dnd_updated(_user)`, `thread_subscribed/unsubscribed`, `thread_marked`, `badge_counts_updated`, `dnd_invalidated`, …
  - These must run on every socket, otherwise a stale DND flag "silently unsilences notifications".

  (OBSERVED: gammons/slk/cmd/slk/grid_dedupe.go:70-133)

- **Grid scoping.**
  - Whether a global notification-pref write propagates to every workspace in a Grid org session is not established. (OBSERVED: 3kh0/super-platinum/docs/global-notifications-protocol.md:58-61)
  - `dnd.*` may require `team_id` after a workspace migration. (OBSERVED: beeper/slackgo/dnd.go:10-16)
  - `client.counts` and `messages.list` are sent with `org_wide_aware=true`. (OBSERVED: gammons/slk/internal/slack/testdata/phase2-api-contracts.json:1511; gammons/slk/internal/slack/activity.go:389)
- **Double-encoded prefs.** `all_notifications_prefs` is JSON inside a JSON string, so decode twice. super-platinum accepts an object as well. (OBSERVED: gammons/slk/internal/slack/client.go:1719-1724; 3kh0/super-platinum/docs/global-notifications-protocol.md:47-49)
- **Mute moved.**
  - Mute state lives in `all_notifications_prefs.channels[id].muted`. The legacy `muted_channels` may be absent, so read the union of both. (OBSERVED: gammons/slk/internal/slack/client.go:1683-1716)
  - Do not port slackgo's `MuteChat`/`UnMuteChat`: they write the legacy pref, and `UnMuteChat` returns early as a no-op when it finds the channel. (OBSERVED: beeper/slackgo/info.go:341-386)
- **Legacy mode values.** Legacy `nothing` desktop mode reads as `mentions_dms`. Mobile `nothing` means mobile is off. (OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:25,70; 3kh0/super-platinum/docs/global-notifications-protocol.md:71-72)
- **`client.userBoot` latest.** Its `latest` is wrong (set to "now" for every conversation); take `latest` from `client.counts`. (OBSERVED: wee-slack/wee-slack/typings/slack_api/slack_client_userboot.pyi:40)
- **Badge math is experiment-gated.** Do not replace it with a sum of sidebar mention counts. (OBSERVED: 3kh0/super-platinum/docs/notification-presentation-protocol.md:280-290,467-496,517-527)
  - With `activity_inbox`=on, the badge sums a fixed `activity_v2` category list. `message_reaction` is excluded. `dm` counts only when `activity_dms_opt_out` is falsy, and several categories sit behind other experiments.
  - Without Inbox, it is `channel_badges.thread_mentions + app_dms + channels`, plus `thread_unreads` when `thread_badging_home_sidebar` or `activity_include_threads_in_badge_count` is on, plus selected alerts.
  - The Dock adds overdue Later, and DMs when they are opted out of Activity.
- **Badges are client-side.** Threads "unread" flags derived from a local cache produce false positives; trust `client.counts.threads.has_unreads`. (OBSERVED: gammons/slk/cmd/slk/workspace.go:88-95)
- **Decode Activity items leniently.** Decode each item separately: Slack sometimes sends `false` where an object is expected, and unknown types appear. (OBSERVED: gammons/slk/internal/slack/activity.go:155-176)
- **Pref writes don't prove delivery.** OS/browser notification permission is separate from the stored pref. (OBSERVED: 3kh0/super-platinum/docs/channel-notifications-protocol.md:174-184)
- **Keywords.**
  - The UI shows them as `", "`-separated but stores them `","`-joined.
  - At most 100 entries, counted by splitting on `/\s*,\s*/`. There is no per-keyword length cap, no trimming beyond `", "`, and no dedup.

  (OBSERVED: 3kh0/super-platinum/docs/global-notifications-protocol.md:186-198)

- **Schedule times are local wall-clock strings.** Changing the Slack timezone keeps them as is; don't convert. (OBSERVED: 3kh0/super-platinum/docs/notification-schedule-protocol.md:142-147)
- **Request fingerprint.**
  - A body that carries `_x_mode` but no `_x_reason` matches about 6% of official traffic and about 100% of a naive client. (OBSERVED: gammons/slk/internal/slackhttp/reason.go:49-62)
  - Observed values in this area: `fetchClientCountsOnConnect`, `pollClientCountsAfterTimeout`, `viewed`, `fetch-frecency-prefs`, `fetchAndUpsertDndForUsers-getDndTimesFor:self`/`:others`, `unread-counts/onLastReadUpdated`, `record-notification-event`, `prefs-api/setUserPrefByApi`, `saved-api/savedList`. (OBSERVED: gammons/slk/internal/slackhttp/testdata/official-request-shape.json:128-177)
- **Sources disagree.** On whether `read` is required for `subscriptions.thread.mark` (ericboehs omits it), and on the subscription timestamp key (`date_create` vs `date_created`). (OBSERVED: ericboehs/slk/lib/slk/api/threads.rb:23-29; wee-slack/wee-slack/typings/slack_rtm/slack_rtm_message.pyi:265; beeper/slackgo/websocket_channels.go:93)
- **Deprecated method names.** `channels.mark` and `im.mark` (still referenced by emacs-slack) are superseded by `conversations.mark`. (OBSERVED: emacs-slack/emacs-slack/slack-channel.el:39; emacs-slack/emacs-slack/slack-im.el:39; https://docs.slack.dev/reference/methods/conversations.mark)

## LEADS

Beliefs and guesses, unverified. Test each against a live capture before relying on it.

**Notifications and realtime frames**

- When the server emits `desktop_notification`, and whether the `wss-primary` web-client socket (with `lazy_channels` / `no_query_on_subscribe`) carries it at all. Also whether the server pre-filters it by prefs and DND, and whether the official desktop client shows it or computes banners locally from `message` frames. Capture frames while being @mentioned in an open web client.
- The firing conditions and full payload of `update_thread_state`, and whether it duplicates `client.counts.threads`.
- Whether `threads_everything` or `follow_all_threads` cause server-side auto-subscription (`thread_subscribed` frames) for replies.
- `mpim_marked` vs `group_marked` for MPDMs on current servers (wee-slack types both).
- The current web client still receives `update_thread_state` (moved from `update_thread_state`). unverified: clack NATIVE.md:204-214 lists it among the raw passthrough types of clack's own `Subscribe()` API, which is not a web-client capture, and emacs-slack's handler (slack-websocket.el:533-541) says nothing about current servers.
- `activity_clear_all_completed` and `activity_views_updated` are `subtype` values of the `activity` frame (moved from `activity`). unverified: clack ACTIVITY.md:45-49 lists them after the three `activity` subtypes with "also", and 400-403 and 503-505 name them without a frame type.

**Threads and unreads**

- `subscriptions.thread.clearAll` probably clears the "new threads" indicator (`new_threads_count`) up to `current_ts`.
- `subscriptions.thread.getTimestamps` (same args as `getView` plus `mode`, `cursor`) may be a cheaper way to sync thread read state than full `getView` sweeps.
- `unread.collapse`, `unread.expand` and `unread.markRead {markers}` likely back the "All unreads" view (collapse a channel group; mark-all-read with per-channel markers). The `markers` shape is unknown.
- `conversations.badgeCount` likely returns one conversation's badge counts.
- `subscriptions.thread.clearAll` takes `current_ts` in a form body (moved from `subscriptions.thread.clearAll`). unverified: the only client source, emacs-slack, sends it as a URL query parameter with an empty body (slack-all-threads-buffer.el:257-263; slack-request.el:201-209).
- `client.counts.dry_run_last_fetched` probably echoes the previous response's `counts_last_fetched` for an incremental poll. The poll cadence behind `pollClientCountsAfterTimeout` is unknown.
- For `ims`/`mpims`, `mention_count` may count every unread message rather than @-mentions. gammons labels this hypothesis unverified (gammons/slk/internal/slack/client.go:1222-1229).

**Activity**

- `activity.views` request params and the full field names of `views[]` items.
- The Activity `key` format `<type>-<channel>-<ts>` may not hold beyond `thread_v2`.
- `activity.views` is a `POST` with no params beyond the token (moved from `activity.views`). unverified: phase2-api-contracts.json:3701-3704 records only the call name and time in the official boot sequence, and clack ACTIVITY.md:215-218 describes only the response; neither shows a verb, encoding or params.

**Prefs and DND**

- `_x_reason=fetchAndUpsertDndForUsers-getDndTimesFor:others` is probably the `dnd.teamInfo` call. `record-notification-event` belongs to an unidentified method, perhaps notification analytics or acknowledgement.
- Huddle notification prefs (key names and setter) and VIP breakthrough during DND were not captured; `client.counts.alerts.huddles_missed_huddle` exists.
- `highlight_words` is probably legacy, with `global_keywords` the live source. Keyword matching semantics in the official client (word boundary vs substring, case folding, emoji/punctuation) are unverified.
- The `users.prefs.setNotifications` response shape, and whether it echoes the updated prefs.
- `users.prefs.set name=muted_channels` may now be ignored by the server, since mute lives in `all_notifications_prefs`.
- Whether a Grid org-level session's `users.prefs.setNotifications global=true` applies to every workspace, and how the Availability tab's "Working hours" relates to the `dnd_*` schedule in Grid.

**Implementation choices**

- emacs-slack's `last_read`-based `getView` cursor may no longer page correctly; prefer gammons's `latest_reply` cursor.
