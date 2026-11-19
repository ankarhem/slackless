# Verification report: api-huddles.md

Date: 2026-11-19. Every `###` subsection (15 endpoint sections + 6 websocket event
sections) was re-checked against the source named in its `OBSERVED:` tag.

Method note: all GitHub files were fetched raw from their default branches at the cited
line ranges. The two jasonaa.notion.site pages are no longer readable as public HTML or
via Notion's public API (blocks come back `role: "none"`); their content was verified
through the 2021-09-18 Wayback snapshot (timestamp 20210918192833), which contains the
captured WebSocket payloads verbatim, and through Notion's public page-chunk API for the
"Join Huddle" page (whose sample request/response are fully readable there).
`docs.slack.dev/reference/methods/calls.add` was fetched live. Local citations
(`.omo/research/gist.md`) were checked against the file in this repo.

Item count: `grep -c '^### '` on api-huddles.md (before the moves below) = 21, which
already includes all 6 websocket event sections (they are `###` subsections). Total
items checked = 21; no items beyond that count.

| Item                               | Check                           | Result | Reason                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| ---------------------------------- | ------------------------------- | ------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| rooms.join                         | name + transport + params       | PASS   | slaq `src/api/client.rs:1510-1525` posts form `rooms.join` with `channel_id`/`regions`/`multidevice`; protocol doc sec. 1 table (`id?`, `channel_id?`, `thread_ts?`, `regions`, `multidevice`) + sec. 5 (xoxc accepted, response capture, prewarm/expiry); Notion "Join Huddle" page (POST `https://<ws>.slack.com/api/rooms.join`, multipart body `channel_id`/`regions: us-east-2`/`token`, `d`/`xoxd-` cookie callout, response `call.free_willy` + `huddle` + `canvas` blocks match) |
| rooms.leave                        | name + params + behavior        | PASS   | `client.rs:1532-1544` signature `channel_id, call_id, attendee_id, reason`; protocol doc sec. 1 table, sec. 4 benign errors, sec. 5 `invalid_arguments` after join-without-media, sec. 10 "client doesn't use it, teardown is the Chime LEAVE frame"                                                                                                                                                                                                                                     |
| rooms.notifyMember                 | name + params                   | PASS   | protocol doc sec. 1 method table: `channel_id`, `user_id` (exact)                                                                                                                                                                                                                                                                                                                                                                                                                        |
| rooms.getLink                      | name + params                   | PASS   | protocol doc sec. 1 method table: no arguments (`-`)                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| rooms.canJoinHuddle                | source reproducible             | FAIL   | Only cited source (Routes table on notion "figuring out the slack huddles api") not reproducible: Notion public API returns the page blocks as `role:"none"`, the 2021-09-18 Wayback capture has the payloads but not the Routes table, and no other cited source (slaq doc/code, slack-go, wee-slack) mentions this method. Claim moved to LEADS as unverified                                                                                                                          |
| huddles.info (Edge API / Flannel)  | transport + pattern             | PASS   | `.omo/research/gist.md` L109-111 (`POST https://edgeapi.slack.com/cache/<id>/<resource>/<action>`, JSON bodies, Flannel over Loom) + L122 (`/cache/E03C83N442J/huddles/info # huddle/call state` among boot calls) + L143/L174 (conditional `updated_ids` fetching); exact JSON body correctly hedged in LEADS                                                                                                                                                                           |
| screenhero.rooms.info              | name + params                   | PASS   | protocol doc sec. 1 method table: `room`; sec. 4: id prefixes `Vr`/`zt-`                                                                                                                                                                                                                                                                                                                                                                                                                 |
| screenhero.rooms.create            | name + response fields          | PASS   | `client.rs:359-374` `ScreenheroRoomsCreateResponse { call_id, id, channel_id }` + flattened unknowns; protocol doc sec. 1: "Not `screenhero.rooms.create`... the current family is `rooms.*`"                                                                                                                                                                                                                                                                                            |
| screenhero.rooms.* legacy family   | member list                     | PASS   | protocol doc sec. 1: exactly `screenhero.rooms.{invite,refreshToken,rename,userEvent,proxyLogs,proxyAnalytics,proxyMeetingEvents}`                                                                                                                                                                                                                                                                                                                                                       |
| calls.add (public)                 | name + scopes + args + response | PASS   | docs.slack.dev page (fetched live): `calls.add`, scope `calls:write`, Tier 3 rate limit, `external_unique_id` + `join_url` Required, `created_by` optional with user token / required otherwise, `users` JSON array, response example with `"id": "R0E69JAIF"`; slack-go `calls.go` `AddCallContext` -> `postMethod(ctx, "calls.add", values, ...)` with the same params                                                                                                                 |
| calls.info (public)                | name + params + response fields | PASS   | slack-go `calls.go` `GetCallContext` -> `calls.info` with `id`; `Call` struct fields (`id, title, date_start, date_end, external_unique_id, join_url, desktop_app_join_url, external_display_id, users, channels`) match the doc's list exactly                                                                                                                                                                                                                                          |
| calls.update (public)              | name + params                   | PASS   | slack-go `calls.go` `UpdateCallContext` -> `calls.update` (`token`, `id`, `title`, `join_url`, `desktop_app_join_url`)                                                                                                                                                                                                                                                                                                                                                                   |
| calls.end (public)                 | name + params                   | PASS   | slack-go `calls.go` `EndCallContext` -> `calls.end` with optional `duration` (seconds)                                                                                                                                                                                                                                                                                                                                                                                                   |
| calls.participants.add (public)    | name + params                   | PASS   | slack-go `calls.go` `CallAddParticipantsContext` -> `calls.participants.add`, `users` marshalled to JSON via `setCallParticipants`                                                                                                                                                                                                                                                                                                                                                       |
| calls.participants.remove (public) | name + params                   | PASS   | slack-go `calls.go` `CallRemoveParticipantsContext` -> `calls.participants.remove`, same `setCallParticipants` helper                                                                                                                                                                                                                                                                                                                                                                    |
| huddle_invite                      | event name + payload            | PASS   | slaq protocol doc sec. 11 table: fires for direct/two-party only, fields `channel_id`, `sender_user_id`, `call_id` + complete `free_willy` block; `src/app/huddles.rs` fixture adds `type`/`team_id`, handler builds `Event::Invite { channel, caller }`                                                                                                                                                                                                                                 |
| huddle_invite_cancel               | event name + payload            | PASS   | Wayback snapshot 20210918192833 of the notion page contains the payload `{"type":"huddle_invite_cancel","team_id":"T0266FRGM","channel_id":"C0266FRGV","event_ts":"1628135021.355900"}` verbatim; `huddles.rs` -> `Event::Cancel { channel }`                                                                                                                                                                                                                                            |
| sh_room_join                       | event name + payload            | PASS   | Wayback snapshot contains the full captured payload verbatim (matches the doc's example incl. `huddle.channel_id`, `media_backend_type: "free_willy"`); `huddles.rs` handles it with fallback to `room.channels[0]` for legacy calls                                                                                                                                                                                                                                                     |
| sh_room_leave                      | event name + payload            | PASS   | slack-go `websocket_managed_conn.go:652-654` registers `sh_room_join`/`sh_room_leave`/`sh_room_update`; slaq protocol doc sec. 11: all three carry the same complete `room` object; `huddles.rs` single arm                                                                                                                                                                                                                                                                              |
| sh_room_update                     | event name + payload            | PASS   | slack-go `websocket_misc.go:248-256` `SHRoomUpdateEvent { type, room, user, huddle, event_ts, ts }`; wee-slack `typings/slack_rtm/slack_rtm_message.pyi:304-310` `SlackShRoomUpdate` TypedDict                                                                                                                                                                                                                                                                                           |
| user_huddle_changed                | event name + payload            | PASS   | slack-go `websocket_managed_conn.go:618` + `websocket_misc.go:88-94` `{ type, user, cache_ts, event_ts }`; `users.go:50-51` and wee-slack `typings/slack_api/slack_profile.pyi:39-40` confirm `huddle_state` / `huddle_state_expiration_ts` profile fields                                                                                                                                                                                                                               |

TOTAL: 21 items, 20 PASS, 1 FAIL

Supporting checks done alongside the table (media-stack `####` subsections, whose facts
back the endpoint/event items above): signaling URL query params + `["_aws_wt_session",
<JoinToken>]` subprotocols and `[0x05][SdkSignalFrame]` framing (slaq `signaling.rs:1-15`,
protocol doc sec. 3); frame-type constants JOIN=1...REMOTE_VIDEO_UPDATE=24
(`signaling.rs:39-52`, exact match); proto2 schema with 53 messages, 13 enums, 225 fields
(counted in `proto/SignalingProtocol.proto`); JOIN frame `protocol_version=2`,
`HAS_STREAM_UPDATE(2)`, `wants_compressed_sdp` (protocol doc sec. 3 + `signaling.rs:54-59`);
JOIN_ACK TURN credentials with ttl 300 and `video_subscription_limit 25` (protocol doc
sec. 5); audio-only SUBSCRIBE shape incl. `AmazonChimeExpressAudio`, framerate 15,
max_bitrate_kbps 600 (protocol doc sec. 7); SRTP profile-order and TURN-relay findings
(protocol doc SOLVED section, secs. 8/10); crate list incl. `webrtc 0.12`,
`tokio-tungstenite` (rustls), `prost`/`protox`, `sonora 0.2`, `rav1d`,
`shiguredo_libvpx` (`Cargo.toml`); `room` object field list (slack-go
`websocket_misc.go:185-221`, matches field-for-field incl. nullability note for `name`);
`huddle_thread` subtype fields + `?open=start_huddle` join link (wee-slack
`slack_message.py:893-914`, `slack_conversations_history.pyi:407-411`); boot flags
`allow_huddles*` (slackdump `client_boot.go:358-360`) and prefs `huddles_autoinvite` /
`use_voip_huddle_invite` (`client.userBoot.json`); experiments
`huddles_dual_udp_support` / `huddles_agc_browser_default` (`.omo/research/gist.md` L233);
`api.huddles.onPeerMessage` + transcription hooks (taut `HuddleActivity.tsx`); mautrix
"Added notices for huddles ending" + `beeper/slackgo` replace (CHANGELOG.md, go.mod);
browser fallback URL (emacs-slack `slack-buffer.el:1158-1164`). All consistent with the
document.

## Edits made to api-huddles.md

1. `### rooms.canJoinHuddle` section body replaced: claim moved to LEADS as unverified
   (notion Routes table not reproducible via live page, Notion public API, or the
   2021-09-18 Wayback capture; no independent confirmation). The `###` header was kept so
   the section count is unchanged.
2. `## LEADS`: added an explicit `rooms.canJoinHuddle` entry with the "unverified:" note
   and reason.
3. `### huddle_invite_cancel` OBSERVED line: annotated that the live notion page is not
   publicly readable and the payload was verified against Wayback snapshot 20210918192833.
4. `### sh_room_join` OBSERVED line: same Wayback-verification annotation.
5. `### huddles.info` OBSERVED (notion line): annotated that URL, transport and purpose
   are independently verified via the `.omo/research/gist.md` boot trace.

No misspellings, wrong transports, or wrong parameters were found in any other item; no
other content was changed and no endpoints were added or removed.
