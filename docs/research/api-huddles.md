# Huddles and calls - Slack web-client API

## Summary

Covers huddle presence (room state, invites, per-user `huddle_state`), joining/leaving
huddles (`rooms.*`, `screenhero.rooms.*`), the public third-party Calls API (`calls.*`),
the huddle thread/canvas linkage, and the media plane: huddles are **Amazon Chime SDK**
meetings (codename `free_willy`); Slack only authenticates and hands out Chime
`CreateMeeting`/`CreateAttendee` credentials. The `rooms.*`/`huddles.*` family is
private (web-client only, works with `xoxc` + `d` cookie); `calls.*` is public
(bot/user tokens, scopes, tier-3 rate limit). Biggest risks: the entire media session
is a private Chime signaling protocol over a second WebSocket (JOIN/SUBSCRIBE protobuf
frames, SRTP to Chime SFU); `rooms.leave` semantics are partially unknown; everything
here is undocumented and changeable without notice.

## Endpoints

### rooms.join

Joins (or creates) the huddle in a channel/DM/thread and allocates the Chime meeting.
This is the single most important private call in the area.

- Transport: `POST https://<workspace>.slack.com/api/rooms.join`, form-encoded (multipart
  or urlencoded), `token=<xoxc>` in the body, session cookie `d=<xoxd>` required.
- Visibility: private (web-client).
- Request params:
  - `token` - string - required - `xoxc-` client token.
  - `channel_id` - string - required (when joining by channel) - channel/DM to huddle in.
  - `id` - string - optional - call/room id (join an existing room directly).
  - `thread_ts` - string - optional - huddle attached to a thread.
  - `regions` - string - optional - preferred Chime media region, e.g. `us-east-2`,
    `eu-central-1`, `eu-south-1`. Official client sends the result of
    `getNearestChimeMediaRegion(teamId)`; may be sent empty (server resolves).
  - `multidevice` - bool - optional - observed sent as `"true"`/`"false"`; effect unverified.
- Response (key fields, from a real capture):
  ```jsonc
  {
    "ok": true,
    "call": {
      "call_id": "R000000000", // 11 chars, always starts with R
      "survey_percent": 0.05,
      "free_willy": {
        // Chime backend codename
        "attendee": {
          "AttendeeId": "0ea2843f-...",
          "ExternalUserId": "<team>-<call_id>-<user_id>",
          "JoinToken": "<98 chars>", // used as WS subprotocol, not a header
          "Capabilities": {
            "Audio": "SendReceive",
            "Video": "SendReceive",
            "Content": "SendReceive",
          },
        },
        "meeting": {
          "MeetingId": "dedc1557-...",
          "ExternalMeetingId": "<team>-<call_id>-<user_id>",
          "MediaRegion": "eu-central-1",
          "MeetingArn": "arn:aws:chime:us-east-1:...:meeting/<uuid>",
          "MediaPlacement": {
            "SignalingUrl": "wss://signal.z3.ec1.m.chime.aws/control/<uuid>",
            "AudioHostUrl": "<hash>.k.m3.ec1.app.chime.aws:3478",
            "AudioFallbackUrl": "wss://wss.k.m3.ec1.app.chime.aws:443/calls/<uuid>",
            "TurnControlUrl": "https://<n>.cell.us-east-1.meetings.chime.aws/v2/turn_sessions",
            "ScreenSharingUrl": "wss://...chime.aws:443/v2/screen/<uuid>",
            "ScreenViewingUrl": "wss://...chime.aws:443/ws/connect?...",
            "ScreenDataUrl": "wss://...chime.aws:443/v2/screen/<uuid>",
            "EventIngestionUrl": "https://data.svc.ue1.ingest.chime.aws/v1/client-events",
          },
        },
      },
      "huddle": {
        "id": "R000000000",
        "channels": ["C..."],
        "has_ended": false,
        "huddle_link": "https://app.slack.com/huddle/<team>/<channel>",
        "participants": [],
        "thread_root_ts": "...",
      },
      "canvas": { "root_thread_ts": "...", "thread_channel_id": "C..." },
    },
  }
  ```
- Behavior notes (verified in the field): repeated calls return the same `call_id` and
  `MeetingId`; the room is _prewarmed_ (`participants: []`, `is_prewarmed: true`) until
  media actually flows; a USLACKBOT-authored `huddle_thread` message appears in the
  channel already at join time; the room self-expires (~2 min, `has_ended: true`) if
  nobody connects media. Connecting the media session is what sets you as a Slack
  participant - no extra Slack call is needed.
- Pagination: none.
- Used for: ringing/joining every huddle; the `free_willy` block feeds the Chime SDK session.
- OBSERVED: https://jasonaa.notion.site/Join-Huddle-2e150d931336454397cb632d62d27715 (request body + full response capture)
- OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (sections 1, 2, 5; xoxc acceptance; prewarm; expiry)
- OBSERVED: DangerBlack/slaq/src/api/client.rs:1502-1525 (`rooms_join(channel_id, regions, multidevice)` form POST)

### rooms.leave

Leaves a huddle room. **The official client does not use this** - it tears a huddle down
with a Chime `LEAVE` frame on the signaling socket - and the REST call rejects many
argument combinations.

- Transport: `POST https://<workspace>.slack.com/api/rooms.leave`, form-encoded.
- Visibility: private.
- Request params: `channel_id` - string; `call_id` - string (R-prefixed); `attendee_id` -
  string (Chime attendee); `reason` - string. Also accepts `id`/`room` variants (tried, rejected).
- Response: `{"ok": true}` or `invalid_arguments`.
- Errors: `channel_not_found`, `room_not_found`, `invalid_channel_id`, `attendee_not_found`
  are benign (the official client ignores them). After a `rooms.join` with no media
  connection, `rooms.leave` fails with `invalid_arguments` under every combination tried -
  hypothesis: it wants an attendee session that never existed.
- Pagination: none.
- Used for: cleanup; but prefer Chime `LEAVE` on the signaling socket.
- OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (sections 1, 5; benign errors; invalid_arguments findings)
- OBSERVED: DangerBlack/slaq/src/api/client.rs:1527-1544 (`rooms_leave(channel_id, call_id, attendee_id, reason)`)

### rooms.notifyMember

Notifies (invites) a user to the channel huddle.

- Transport: `POST https://<workspace>.slack.com/api/rooms.notifyMember`, form-encoded.
- Visibility: private.
- Request params: `channel_id` - string - required; `user_id` - string - required.
- Response: not captured (believed `{"ok":true}` - see LEADS).
- Pagination: none.
- Used for: "invite someone to the huddle" from the participant picker.
- OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (section 1 method table, read from webapp bundle)

### rooms.getLink

Returns a shareable huddle link.

- Transport: `POST https://<workspace>.slack.com/api/rooms.getLink`, form-encoded.
- Visibility: private.
- Request params: none observed.
- Response: not captured (see LEADS); presumably carries an `https://app.slack.com/huddle/<team>/<channel>` link (that link shape is observed inside `rooms.join`'s `huddle.huddle_link`).
- Pagination: none.
- Used for: "copy huddle link" action.
- OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (section 1 method table)

### rooms.canJoinHuddle

Moved to LEADS - **unverified**: the only cited source (the Routes table on the notion
page "figuring out the slack huddles api") cannot be reproduced. The live page's blocks
are not served by Notion's public API (`role: "none"`), and the 2021-09-18 Wayback
capture of the page contains the captured WebSocket payloads but **not** the Routes
table. No other cited source (slaq teardown, slack-go, wee-slack) mentions this method.
Method name, transport, params and response are all unconfirmed - do not implement
from this entry; re-verify from a live webapp bundle or boot trace before use.

### huddles.info (Edge API / Flannel)

Batch huddle state for channels: who is in which huddle right now. Called during boot
alongside `users/info`, `emojis/info`, etc.

- Transport: `POST https://edgeapi.slack.com/cache/<team_id>/huddles/info`, **JSON body**
  (not form-encoded), same auth (token + `d` cookie).
- Visibility: private (Flannel edge cache over the Loom index).
- Request params (shape): conditional-fetch envelope - the client sends ids with
  last-known update timestamps (`updated_ids`) and the server returns only what changed;
  channel selection for which huddles to report. Exact field names not captured - see LEADS.
- Response: huddle state per channel (participants etc.); not captured in detail.
- Pagination: none.
- Used for: sidebar/channel huddle chips at boot; then kept live by `sh_room_*` events.
- OBSERVED: https://jasonaa.notion.site/figuring-out-the-slack-huddles-api-1220507c034046669887287112f54049 ("Huddles Info - checks what users are in huddles for selected channels"; live page not publicly readable - URL, transport and purpose independently verified via the gist.md trace below)
- OBSERVED: .omo/research/gist.md (live trace: `/cache/E03C83N442J/huddles/info # huddle/call state` among boot Edge-API calls; conditional `updated_ids` fetching)

### screenhero.rooms.info

Legacy call-room info (Screenhero era, still served).

- Transport: `POST https://<workspace>.slack.com/api/screenhero.rooms.info`, form-encoded.
- Visibility: private.
- Request params: `room` - string - room id (prefixes `Vr` or `zt-` observed).
- Response: not captured.
- Pagination: none.
- Used for: legacy calls; the current huddle family is `rooms.*` - the slaq teardown notes
  "Not `screenhero.rooms.create`... The current family is `rooms.*`".
- OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (section 1 method table; call/room id prefixes in section 4)

### screenhero.rooms.create

Legacy "start a call" - returns a join URL. Still present in the webapp bundle.

- Transport: `POST https://<workspace>.slack.com/api/screenhero.rooms.create`, form-encoded.
- Visibility: private (legacy).
- Request params: channel/room selector (exact set not captured).
- Response (partial, as parsed by a third-party client):
  ```jsonc
  {
    "call_id": "R...",
    "id": "...",
    "channel_id": "C...",
    "join_url": "https://...",
    "...": "many unnamed fields",
  }
  ```
- Pagination: none.
- Used for: the old 1:1/DM calls flow; for huddles use `rooms.join` instead.
- OBSERVED: DangerBlack/slaq/src/api/client.rs:359-374 (`ScreenheroRoomsCreateResponse` with `call_id`, `id`, `channel_id` + flattened unknowns)
- OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (section 1: "Not screenhero.rooms.create")

### screenhero.rooms.* legacy ancillary family

`screenhero.rooms.{invite, refreshToken, rename, userEvent, proxyLogs, proxyAnalytics,
proxyMeetingEvents}` - still alive in the bundle, ancillary.

- Transport: `POST https://<workspace>.slack.com/api/<method>`, form-encoded.
- Visibility: private (legacy).
- Request/Response: not captured per method.
- Pagination: none.
- Used for: legacy call lifecycle (invites, token refresh, rename, client event proxying);
  a huddle-parity client should not need them; listed so unexpected traffic can be recognized.
- OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (section 1: "Still around are screenhero.rooms.{...}")

### calls.add (public)

Registers a third-party call and returns a Call object the client renders via a
`{"type":"call","call_id":"R..."}` block in `chat.postMessage` (cross-area: messaging).

- Transport: `POST https://slack.com/api/calls.add`, form-encoded or JSON.
- Visibility: **public**. Scopes: `calls:write` (bot or user token). Rate limit: Tier 3.
- Request params: `token` - required; `external_unique_id` - string - required; `join_url` -
  string - required; `external_display_id`, `desktop_app_join_url`, `date_start` (unix ts),
  `title`, `created_by` (user id; required only for bot tokens), `users` (JSON array of
  `{slack_id}` or `{external_id, display_name, avatar_url}`) - all optional.
- Response: `{"ok":true,"call":{"id":"R0E69JAIF","date_start":...,"join_url":...,
"users":[{"slack_id":"U..."}],...}}`.
- Pagination: none.
- Used for: third-party call providers; NOT how the Slack client starts huddles (it uses `rooms.join`).
- OBSERVED: https://docs.slack.dev/reference/methods/calls.add (full page: scopes, args, response, `call` block)
- OBSERVED: slack-go/slack calls.go (`AddCallContext` -> `api.postMethod(ctx, "calls.add", values, ...)`; same params)

### calls.info (public)

Fetches a Call object by id.

- Transport: `POST https://slack.com/api/calls.info`, form-encoded or JSON.
- Visibility: public (scope per docs family; see calls.add page for family pattern).
- Request params: `token` - required; `id` - string - required - Call id.
- Response: `{"ok":true,"call":{ ... Call object ... }}` with fields `id`, `title`,
  `date_start`, `date_end`, `external_unique_id`, `join_url`, `desktop_app_join_url`,
  `external_display_id`, `users[]`, `channels[]`.
- Pagination: none.
- Used for: hydrating a `call` message block.
- OBSERVED: slack-go/slack calls.go (`GetCallContext` -> `calls.info` with `id`; `Call` struct fields)

### calls.update (public)

Updates title/URLs of a registered call.

- Transport: `POST https://slack.com/api/calls.update`, form-encoded or JSON.
- Visibility: public.
- Request params: `token` - required; `id` - required; `title`, `join_url`,
  `desktop_app_join_url` - optional.
- Response: `{"ok":true,"call":{...}}`.
- Pagination: none.
- Used for: third-party providers editing a live call card.
- OBSERVED: slack-go/slack calls.go (`UpdateCallContext` -> `calls.update`)

### calls.end (public)

Ends a registered call.

- Transport: `POST https://slack.com/api/calls.end`, form-encoded or JSON.
- Visibility: public.
- Request params: `token` - required; `id` - required; `duration` - integer seconds - optional.
- Response: `{"ok":true}`.
- Pagination: none.
- Used for: third-party providers ending a call card.
- OBSERVED: slack-go/slack calls.go (`EndCallContext` -> `calls.end` with optional `duration`)

### calls.participants.add (public)

Adds participants to a call.

- Transport: `POST https://slack.com/api/calls.participants.add`, form-encoded or JSON.
- Visibility: public.
- Request params: `token` - required; `id` - Call id - required; `users` - JSON array -
  required - same participant shape as `calls.add`.
- Response: `{"ok":true}`.
- Pagination: none.
- Used for: third-party providers syncing participants.
- OBSERVED: slack-go/slack calls.go (`CallAddParticipantsContext` -> `calls.participants.add`, marshals `users` to JSON)

### calls.participants.remove (public)

Removes participants from a call.

- Transport: `POST https://slack.com/api/calls.participants.remove`, form-encoded or JSON.
- Visibility: public.
- Request params: identical to `calls.participants.add`.
- Response: `{"ok":true}`.
- Pagination: none.
- Used for: third-party providers syncing participants.
- OBSERVED: slack-go/slack calls.go (`CallRemoveParticipantsContext` -> `calls.participants.remove`, same `setCallParticipants` helper)

## Media stack: Amazon Chime SDK signaling

Huddles are not a Slack protocol. The webapp bundle `gantry-v2-async-amazon-chime-sdk-js-experimental`
is `amazon-chime-sdk-js` 15.1.0 (Apache-2.0, public sources at github.com/aws/amazon-chime-sdk-js).
Slack only authenticates (`rooms.join`) and hands out Chime credentials; the media plane is
standard Chime SDK. `media_backend_type: "free_willy"` on the room confirms the Chime backend;
the legacy Screenhero branch (direct TURN, no SFU) must be rejected with a clear error.
OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md ("The result in one line", section 2 `handleCallAPIResponse`)
OBSERVED: https://jasonaa.notion.site/Join-Huddle-2e150d931336454397cb632d62d27715 (hexdump of the first signaling frame containing "amazon-chime-sdk-js"; `media_backend_type` in sh_room_join payload)
OBSERVED: https://github.com/aws/amazon-chime-sdk-js (public SDK repo)

#### Second WebSocket (signaling)

```
URL           <MediaPlacement.SignalingUrl>?X-Chime-Control-Protocol-Version=3&X-Amzn-Chime-Send-Close-On-Error=1
Subprotocols  ["_aws_wt_session", "<attendee.JoinToken>"]   <- auth rides as a WS subprotocol
Framing       every message = [0x05 type byte][SdkSignalFrame protobuf]  (0x05 = FRAME_TYPE_RTC;
              on receive 0x02 is also accepted; other values only warn)
```

Frame types (SdkSignalFrame.type): JOIN=1, JOIN_ACK=2, SUBSCRIBE=3, SUBSCRIBE_ACK=4, INDEX=5,
PAUSE=7, RESUME=8, LEAVE=9, LEAVE_ACK=10, PING_PONG=19, CLIENT_METRIC=21, REMOTE_VIDEO_UPDATE=24.
The proto schema is proto2; the upstream repo versions only generated JS, but a reconstructed
`SignalingProtocol.proto` (53 messages, 13 enums, 225 fields) exists. Server sends PING_PONG
every 10 s and expects the pong.
OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (section 3)
OBSERVED: DangerBlack/slaq/src/calls/signaling.rs:11-16, 33-62 (URL params, subprotocol auth, frame framing, frame-type constants)
OBSERVED: DangerBlack/slaq/proto/SignalingProtocol.proto (reconstructed schema)

#### JOIN / JOIN_ACK

JOIN frame: `protocol_version=2`, `flags=SdkJoinFlags.HAS_STREAM_UPDATE(2)`, `client_details`
(platform_name, chime_sdk_version, utc offset...), random `audio_session_id`,
`wants_compressed_sdp=true` (official client sends the SDP later raw-deflate compressed in
SUBSCRIBE; an uncompressed `sdpOffer` is also accepted). JOIN_ACK carries the **TURN
credentials** - not the `TurnControlUrl`:

```
turn_credentials.uris     ["turn:ice.z2.ec1.m.chime.aws:3478?transport=udp",
                           "turns:ice.z2.ec1.m.chime.aws:443?transport=tcp"]
turn_credentials.username "<unix_expiry_ts>:<uuid>"
turn_credentials.ttl      300                      <- long sessions must re-acquire
video_subscription_limit  25
```

OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (sections 3, 5 JOIN_ACK contents)
OBSERVED: DangerBlack/slaq/src/calls/signaling.rs:64-66 (JOIN flag, PING/PONG types)
OBSERVED: DangerBlack/slaq/src/calls/session.rs:390-395, :736 (ICE servers built from JOIN_ACK)

#### SUBSCRIBE / SUBSCRIBE_ACK (becoming a participant)

Audio-only subscribe as the official client sends it: `duplex=RX` (RX describes the video
direction even when sending audio), full non-trickle `sdp_offer`, `audio_host=AudioHostUrl`,
`audio_checkin=false`, `audio_muted=<bool>`, `send_streams=[{media_type=AUDIO,
track_label="AmazonChimeExpressAudio", attendee_id, stream_id=1, group_id=1, framerate=15,
max_bitrate_kbps=600, avg_bitrate_bps=400000}]` (framerate is set on audio too; the server
expects it). Publisher sessions declare `duplex=DUPLEX` instead of RX. Chain verified end to
end: `rooms.join -> signaling -> JOIN/JOIN_ACK -> SDP offer -> SUBSCRIBE -> SUBSCRIBE_ACK ->
DTLS-SRTP -> connected`, and **connecting media registers you as a Slack participant**
(`participants`/`participant_history` fill in with no further Slack call).
OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (sections 5, 7, 8, 10)
OBSERVED: DangerBlack/slaq/src/calls/signaling.rs:67-75 (RX/DUPLEX/`#content` constants)
OBSERVED: DangerBlack/slaq/src/calls/session.rs:526 (subscribe call with audio_host/attendee)

#### Video and screenshare

Room state tracks `participants_camera_on/off` and `participants_screenshare_on/off`.
Video subscribe limits: `video_subscription_limit=25` (JOIN_ACK); bitrate changes are
requested with `REMOTE_VIDEO_UPDATE`/`watch_video` (per-stream kbps). A shared screen is not
a separate protocol: the sharer joins the meeting a **second time** as an attendee whose id
ends in `#content` and publishes a video stream on the same connection; a viewer reassembles
the RTP and decodes. Incoming screenshare video is AV1 (slaq decodes with pure-Rust `rav1d`);
slaq's own outgoing share encodes VP8 via libvpx. Camera/video SDP from the official client
is BUNDLEd (`m=audio` + `m=video a=inactive` in audio-only sessions).
OBSERVED: DangerBlack/slaq/src/calls/screen.rs:1-10 ("whoever shares joins the meeting a second time as an attendee whose id ends in `#content`")
OBSERVED: DangerBlack/slaq/src/calls/signaling.rs:67-75 (`MEDIA_TYPE_VIDEO=2`, `#content` note)
OBSERVED: DangerBlack/slaq/Cargo.toml (`rav1d` "AV1, which is what Slack shares a screen in"; `shiguredo_libvpx` "VP8... our screen-share send uses")
OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (section 8 SDP BUNDLE; screen URLs in invite)
OBSERVED: DangerBlack/slaq/src/calls/live.rs:335-420 (INDEX groups/streams, watch_video bitrate, screen-share attendee/stream detection)

#### Audio levels and speaking indicators

The SFU's `AUDIO_STREAM_ID_INFO` maps stream id -> attendee (turn into Slack user ids via
`ExternalUserId`), and `AUDIO_METADATA` carries per-stream volumes; the loudest unmuted
stream is whoever is talking. The SFU never reports your own track - render your own mic
level locally.
OBSERVED: DangerBlack/slaq/src/calls/live.rs:253-256, :416-448 (stream_users map, audio_stream_id_info, audio metadata "loudest unmuted stream")

#### Transport / SRTP constraints (learned the hard way)

- Offer `AES128_CM_SHA1_80` **first**. The SFU happily picks `AEAD_AES_128_GCM`, but packets
  under that profile were silently discarded (558 pkts out, 18 in) and signaling tore down
  seconds later. Official client offers AES128_CM_SHA1_80 first.
- `webrtc-srtp` 0.14.0 panics on `SRTP_AEAD_AES_256_GCM` (key_len 32 vs fixed Aes128Gcm) -
  exclude AEAD profiles via `SettingEngine::set_srtp_protection_profiles`.
- Set `RTCIceTransportPolicy::Relay`: the official client offers exactly one `typ relay`
  candidate via Chime TURN; webrtc-rs with policy `all` offers 18 host candidates the SFU
  cannot reach. webrtc-rs does not handle TURN-over-TLS/TCP - pass only `turn:...?transport=udp` URIs.
- Official-client frame cadence after subscribe: CLIENT_METRIC every 1.0 s, PING_PONG every
  10 s, plus DATA_MESSAGE / MEETING_SESSION_TIMING early. Going quiet for 10 s got the
  signaling socket reset; empty CLIENT_METRIC frames got closed _earlier_ - the server reads
  the content (PeerConnection stats).
  OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (SOLVED section; sections 7, 8, 10)

## Websocket events

All on the main client WebSocket (`wss://wss-primary.slack.com/...`, same socket as
messages). `sh_room_*` events carry the **complete room object** - replace the previous
snapshot, do not merge.

### huddle_invite

Fires when someone calls you **directly** - two-party conversations only. This is the only
event that should ring; in channels you are never invited, the huddle is just visible.
Payload (key fields): `type`, `call_id` (R...), `channel_id`, `sender_user_id`, `team_id`,
plus a complete `free_willy` block (meeting+attendee) - so you _could_ join without
`rooms.join`, but a single join path (rooms.join) is more robust for channel huddles.
OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (section 11 table + notes)
OBSERVED: DangerBlack/slaq/src/app/huddles.rs (`"huddle_invite" => ... Event::Invite { channel, caller }` + shape test)

### huddle_invite_cancel

The caller hung up before an answer. Payload: `{"type":"huddle_invite_cancel",
"team_id":"T0266FRGM","channel_id":"C0266FRGV","event_ts":"1628135021.355900"}`.
OBSERVED: https://jasonaa.notion.site/figuring-out-the-slack-huddles-api-1220507c034046669887287112f54049 (captured payload; live page not publicly readable - payload verified against Wayback snapshot 20210918192833)
OBSERVED: DangerBlack/slaq/src/app/huddles.rs (`"huddle_invite_cancel" => Event::Cancel { channel }`)

### sh_room_join

A user entered the room. Carries `user`, `huddle: {channel_id}` and the full `room` object.
Real payload (captured, abridged):

```jsonc
{
  "type": "sh_room_join",
  "user": "UN6C43287",
  "room": {
    "id": "R02AYBUK84Q",
    "name": null,
    "media_server": "",
    "created_by": "UN6C43287",
    "date_start": 1628135005,
    "date_end": 0,
    "participants": ["UN6C43287"],
    "participant_history": ["UN6C43287"],
    "participants_camera_on": [],
    "participants_camera_off": [],
    "participants_screenshare_on": [],
    "participants_screenshare_off": [],
    "channels": [],
    "is_dm_call": true,
    "was_rejected": false,
    "was_missed": false,
    "was_accepted": false,
    "has_ended": false,
    "media_backend_type": "free_willy",
    "external_unique_id": "<CHIME_CALL_ID>",
    "app_id": "A00",
  },
  "huddle": { "channel_id": "C0266FRGV" },
  "event_ts": "1628135021.142400",
  "ts": "1628135021.142400",
}
```

For non-huddle calls (legacy) there is no `huddle` block - fall back to `room.channels[0]`.
OBSERVED: https://jasonaa.notion.site/figuring-out-the-slack-huddles-api-1220507c034046669887287112f54049 (full captured payload; live page not publicly readable - payload verified against Wayback snapshot 20210918192833)
OBSERVED: DangerBlack/slaq/src/app/huddles.rs (`"sh_room_update" | "sh_room_join" | "sh_room_leave"` arm; channel fallback to `room.channels`; test fixtures with `huddle.channel_id`)

### sh_room_leave

A user left the room. Same shape as `sh_room_join` (`room`, `user`, `huddle`, `ts`).
A room whose `participants` became empty is effectively ended (no call to join), same as
`has_ended: true`.
OBSERVED: slack-go/slack websocket_managed_conn.go:652-654 (event map entries `sh_room_join`/`sh_room_leave`/`sh_room_update`)
OBSERVED: wee-slack slack/slack_workspace.py:763-764 (`sh_room_join`/`sh_room_update` handler reads `data["huddle"]["channel_id"]`)
OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (section 11: all three carry the same room object)

### sh_room_update

The room changed state (created, ended, renamed, camera/screenshare flags...). Same payload
shape as `sh_room_join`/`sh_room_leave`; handle identically by snapshot-replace.
OBSERVED: slack-go/slack websocket_misc.go:248-256 (`SHRoomUpdateEvent {type, room, user, huddle, event_ts, ts}`)
OBSERVED: wee-slack typings/slack_rtm/slack_rtm_message.pyi:304-310 (`SlackShRoomUpdate` TypedDict)
OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (section 11)

#### The `room` object (shared by all sh_room_* events)

Full field list (nullable/optional noted): `id`, `name` (nullable), `media_server`,
`created_by`, `date_start`, `date_end` (unix), `participants`, `participant_history`,
`participants_events` (map user -> {joined,...}), `participants_camera_on/off`,
`participants_screenshare_on/off`, `canvas_thread_ts`, `thread_root_ts`, `channels`,
`is_dm_call`, `was_rejected`, `was_missed`, `was_accepted`, `has_ended`, `background_id`,
`canvas_background`, `is_prewarmed`, `is_scheduled`, `recording: {can_record_summary}`,
`locale`, `attached_file_ids`, `media_backend_type` (`free_willy` for Chime), `display_id`,
`external_unique_id` (= Chime meeting id), `app_id`, `call_family` (`huddle`),
`huddle_link`.
OBSERVED: slack-go/slack websocket_misc.go:185-221 (`SHRoom` struct, per-field JSON tags)
OBSERVED: wee-slack typings/slack_api/slack_conversations_history.pyi:374-410 (`SlackMessageSubtypeHuddleThreadRoom`)

### user_huddle_changed

Fires when a user's huddle presence (`profile.huddle_state`,
`huddle_state_expiration_ts`) changes - this is the "in a huddle" indicator on avatars.
Payload: `{type, user (with profile), cache_ts, event_ts}`.
OBSERVED: slack-go/slack websocket_managed_conn.go:618 (`"user_huddle_changed": UserHuddleChangedEvent{}`)
OBSERVED: slack-go/slack websocket_misc.go:88-94 (`UserHuddleChangedEvent {type, user, cache_ts, event_ts}`)
OBSERVED: slack-go/slack users.go:50-51 (`huddle_state`, `huddle_state_expiration_ts` profile fields)
OBSERVED: wee-slack typings/slack_api/slack_profile.pyi:39-40 (`huddle_state`, `huddle_state_expiration_ts`)

#### Huddle-related message subtypes (channel trail)

`huddle_thread` message (author `USLACKBOT`): `{subtype:"huddle_thread", channel, permalink,
no_notifications, room:<room object>}` - appears at `rooms.join` time already and flips
`room.has_ended` at close; renderers show "Huddle started/ended" plus the join link
`https://app.slack.com/client/<team>/<channel_id>?open=start_huddle`. Also seen in the
bundle: `sh_room_created`, `sh_room_shared` subtypes. Huddle notes/canvas are reachable via
`room.canvas_thread_ts` / `rooms.join`'s `canvas: {root_thread_ts, thread_channel_id}`
(canvas area owned by a sibling node).
OBSERVED: wee-slack typings/slack_api/slack_conversations_history.pyi:407-411 (`SlackMessageSubtypeHuddleThread`)
OBSERVED: wee-slack slack/slack_message.py:893-914 (huddle_thread rendering + `?open=start_huddle` URL)
OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (section 4 subtypes; section 5 USLACKBOT huddle_thread; response `canvas` block)
OBSERVED: slack-go/slack websocket_misc.go:200-201 (`canvas_thread_ts`, `thread_root_ts` fields)

## Feasibility assessment (Rust client)

Existence proof: **DangerBlack/slaq** is a native Rust Slack client with working huddle
audio (two-party call verified against the official client, in daily use since 2026-09-30),
video and screenshare, built exactly on this document's protocol. This removes most
research risk; what remains is engineering.
OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md ("WORKS (2026-09-30): two-party call verified", "In use since 2026-09-30")
OBSERVED: DangerBlack/slaq/src/calls/{session.rs,signaling.rs,audio.rs,video.rs,screen.rs,live.rs} (full media implementation tree)

#### Candidate crates

- `webrtc` (webrtc-rs) 0.12 - proven against Chime SFU by slaq; caveats below (TURN/TCP
  gap, SRTP AEAD 256 panic in webrtc-srtp 0.14.0, no `inactive` transceiver - use `recvonly`).
- `prost` + the reconstructed `SignalingProtocol.proto` for Chime signaling frames
  (`protox` builds it without protoc).
- `tokio-tungstenite` (rustls) for the signaling WS incl. subprotocol auth.
- `opus` + `cpal` (capture/playback) + `rodio`; `sonora` 0.2 (pure-Rust port of WebRTC's
  audio module) for echo cancellation/noise suppression/AGC - keeps the binary C++-free.
- `rav1d` (pure-Rust AV1 decode) for incoming screenshare; `shiguredo_libvpx` (VP8 send) or
  keep AV1 for send; `screenshots`/SHM capture for screen content.
- Alternative stack: LiveKit's libwebrtc bindings (prebuilt libwebrtc) would trade binary
  portability for a battle-tested WebRTC stack; not exercised against Chime by any observed
  project - see LEADS. The "Chime SDK protocol reverse-engineering projects" are: slaq
  (Rust, full), jasonaa's notes (signaling discovery), amazon-chime-sdk-js itself (the
  reference implementation, Apache-2.0).
  OBSERVED: DangerBlack/slaq/Cargo.toml (`webrtc = "0.12"`, `tokio-tungstenite`, `opus`, `cpal`, `rodio`, `sonora`, `rav1d`, `shiguredo_libvpx`, `screenshots`)
  OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (section 6 "Implementation in Rust": prost/tokio-tungstenite/webrtc/opus/cpal)
  OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (sections 8/10: webrtc-rs TURN-over-TLS failure log, SRTP panic trace)
  OBSERVED: https://github.com/aws/amazon-chime-sdk-js (reference SDK)

#### Effort estimate (staged, aligned with a parity roadmap)

1. **Stage 0 - presence + events (1-2 days).** Parse `huddle_invite`, `huddle_invite_cancel`,
   `sh_room_join/leave/update` (`replace snapshot`), `user_huddle_changed`; render huddle
   chips, "X is in a huddle" states, ringing on DM invites only. Pure JSON, no media.
2. **Stage 1 - join-in-browser fallback (hours).** Open
   `https://app.slack.com/huddle/<team_id>/<room_id>` (or
   `https://app.slack.com/client/<team>/<channel>?open=start_huddle`) in the browser.
3. **Stage 2 - audio (3-6 weeks).** `rooms.join` -> Chime signaling -> JOIN/SUBSCRIBE ->
   DTLS-SRTP audio with the crate stack above. Known sharp edges, all observed: SRTP
   profile order (AES128_CM_SHA1_80 first), relay-only ICE policy, TURN-udp-only for
   webrtc-rs, honest `audio_muted` flag (send encoded silence when muted), PING_PONG 10 s +
   CLIENT_METRIC 1 s cadence, TURN credentials from JOIN_ACK with 300 s TTL (implement
   refresh), don't offer RED (RFC 2198) until packetization is implemented.
4. **Stage 3 - video receive, then send (2-4 weeks each).** INDEX/REMOTE_VIDEO_UPDATE
   handling, per-stream `watch_video` bitrate control, `video_subscription_limit=25`;
   sending = DUPLEX subscribe with a video `SdkStreamDescriptor`.
5. **Stage 4 - screenshare (2-4 weeks).** Receive: detect `#content` attendee, RTP
   reassembly, AV1 decode. Send: second `rooms.join`-style attendee join ending `#content`
   publishing video (VP8/AV1).
6. **Ongoing.** Device management, AEC tuning (`set_stream_delay_ms`, suppression
   aggressiveness - two parameters slaq never tuned), reconnection, huddle notes/canvas
   integration via the canvas APIs (sibling area).
   OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md ("In use since 2026-09-30"; remaining-work list: screen sharing protocol, incoming events, AEC params)
   OBSERVED: https://github.com/emacs-slack/emacs-slack/blob/master/slack-buffer.el:1158-1164 (`slack-join-huddle` opens `https://app.slack.com/huddle/%s/%s` - the browser fallback predates us)

## Gotchas

- **`rooms.leave` is a trap.** The official client never calls it; huddle teardown is the
  Chime `LEAVE` frame + socket close. REST `rooms.leave` rejects every `reason` tried and
  fails `invalid_arguments` without a prior media session. Room IDs: calls start with `R`
  (11 chars), legacy rooms `Vr`/`zt-`; Chime meeting ids are GUIDs.
  OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (sections 4, 5, 10)
- **Prewarmed rooms look ended.** The first `sh_room_update` of a fresh huddle has
  `participants: []` (`is_prewarmed: true`) - same shape as a room that emptied out. Treat
  empty participants OR `has_ended` as "nothing to join". OBSERVED:
  https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (sections 5, 11)
- **Legacy backend still reachable.** `handleCallAPIResponse` chooses Chime (`free_willy`)
  or legacy Screenhero (token/hostname/port/turn_auth). Implement Chime only; reject the
  legacy branch loudly. Check `room.media_backend_type == "free_willy"`.
  OBSERVED: https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (section 2)
- **Region handling.** Official client sends `regions=<chime media region>` (e.g.
  `eu-south-1`, `us-east-2`); empty is accepted and server-resolves, but signaling may land
  in varying regions. OBSERVED:
  https://github.com/DangerBlack/slaq/blob/main/docs/huddle-protocol.md (sections 5, 10)
  OBSERVED: https://jasonaa.notion.site/Join-Huddle-2e150d931336454397cb632d62d27715 (`regions: us-east-2`)
- **Workspace huddle policy.** `client.boot` carries `allow_huddles`,
  `allow_huddles_transcriptions`, `allow_huddles_video`; user prefs carry
  `huddles_autoinvite`, `use_voip_huddle_invite`. Gate the UI on them.
  OBSERVED: rusq/slackdump internal/edge/client_boot.go:358-360
  OBSERVED: rusq/slackdump internal/edge/assets/client.userBoot.json (`huddles_autoinvite`, `use_voip_huddle_invite`)
- **Server-side A/B experiments** seen in boot: `huddles_dual_udp_support`,
  `huddles_agc_browser_default` - behavior (dual UDP media, browser-default AGC) may differ
  per user cohort. OBSERVED: .omo/research/gist.md (experiments list)
- **Public vs private auth differ.** `calls.*` wants a public token (bot/user, scopes
  `calls:write` etc., tier-3 limits); `rooms.*` wants the web-client pair (`xoxc` in body +
  `d` cookie) and has no documented limits - behave like the web client (backoff on 429/5s),
  not like a bot. OBSERVED: https://docs.slack.dev/reference/methods/calls.add (scopes, rate limit)
  OBSERVED: https://jasonaa.notion.site/Join-Huddle-2e150d931336454397cb632d62d27715 (xoxd cookie required)
- **Transcription/recording exist but are opaque.** `allow_huddles_transcriptions` boot
  flag, `room.recording.can_record_summary`, and huddle-local coordination messages
  (taut hooks `api.huddles.onPeerMessage` inside the official client) hint at Chime
  DataMessage-mediated features; no start/stop method observed yet (see LEADS).
  OBSERVED: rusq/slackdump internal/edge/client_boot.go:359
  OBSERVED: slack-go/slack websocket_misc.go:180-183 (`SHRoomRecording`)
  OBSERVED: https://github.com/jeremy46231/taut/blob/master/plugins/HuddleActivity.tsx (`api.huddles.onPeerMessage`, transcription status)
- **mautrix-slack bridges huddle lifecycle minimally** - it posts notices when huddles end
  (via the `huddle_thread` trail) and does not join media; a parity client must do more.
  OBSERVED: https://github.com/mautrix/slack/blob/master/CHANGELOG.md ("Added notices for huddles ending")
  OBSERVED: mautrix/slack go.mod (`replace github.com/slack-go/slack => github.com/beeper/slackgo`)

## LEADS

- `edgeapi.slack.com/cache/<team>/huddles/info` exact JSON body (channel selection +
  `updated_ids` conditional fetch envelope) was never captured; only the URL, purpose
  ("who is in huddles for selected channels") and the Edge-API conditional-fetch pattern are
  observed. Capture once on a live boot.
- `rooms.canJoinHuddle` exact request param name (channel selector) and response shape.
- `rooms.getLink` response (presumably an `app.slack.com/huddle/...` link).
- `rooms.canJoinHuddle` - unverified: the cited notion Routes table is no longer
  reproducible (Notion public API returns the page blocks as `role: "none"`; the
  2021-09-18 Wayback capture lacks the Routes table; no other observed source names
  this method). Pre-flight join check per channel; transport/params/response unknown.
- `rooms.notifyMember` response shape and whether it triggers `huddle_invite` on the
  invitee's socket.
- Whether `multidevice=true` changes the `rooms.join` response (slaq left it unverified).
- `huddle_invite`'s embedded `free_willy` block could enable joining without `rooms.join`;
  unverified whether attendee credentials from an invite are scoped differently.
- Transcription + huddle summary/recording: start/stop endpoints unknown; likely Chime
  DataMessage + a Slack REST call pair. `SHRoomRecording.can_record_summary` and taut's
  `onPeerMessage`/transcription-status hooks are the starting points.
- Scheduled huddles (`room.is_scheduled`): creation/management methods unknown.
- Screen-share URLs (`ScreenSharingUrl`/`ScreenViewingUrl`/`ScreenDataUrl`) suggest a
  separate Chime screen service, but slaq's implementation shows share media flows as a
  normal `#content` video attendee on the main session - reconcile before using those URLs.
- LiveKit libwebrtc Rust bindings as an alternative to webrtc-rs: untested against Chime;
  would sidestep webrtc-rs's TURN-over-TCP gap and SRTP quirks at the cost of a large
  C++ dependency.
- TURN credential renewal at 300 s TTL: refresh flow (re-JOIN vs `TurnControlUrl`
  `/v2/turn_sessions` POST) not yet pinned down; long huddles (>5 min) depend on it.
- Dual-UDP media (`huddles_dual_udp_support` experiment): may move RTP to a second UDP
  path; watch for changed ICE behavior per cohort.
- `screenhero.rooms.refreshToken` semantics (legacy token refresh for long calls) - only
  relevant if the legacy backend must be supported.
- Huddle survey prefs (`huddle_survey_last_seen`, `survey_percent` in `rooms.join`
  response) - low priority UI plumbing.
