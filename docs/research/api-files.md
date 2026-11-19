# Files, images, media and clips - Slack web-client API

Source snapshots read 2026-11-19: mautrix/slack 38d69d5, beeper/slackgo d0ae74c (the fork mautrix pins), slack-go/slack bb921e9, rusq/slackdump fec2e68, rusq/slack 8f5f9f4, emacs-slack/emacs-slack eafa2d4, wee-slack/wee-slack master aa904c2 and 2.x d4b6887, dylex/slack-libpurple 1115ec1, some-du6e/clack 6548bfb (a Go desktop client that copies the 2026 Slack client; its `docs/FILES-AUDIT.md` is derived from Slack's public CDN JS bundle). Paths below are `owner/repo/path:line` at those commits.

## Summary

- Scope: uploading (attach, process, share), reading/listing/deleting files, public links, the Files view (stars, sections, trash), downloading file bytes and thumbnails, snippets, voice/video clips, and Slack's image proxy and CDNs.
- The web client does not use the public "upload v2" pair. It uses private `files.getUploadURL` -> multipart POST -> `files.completeUpload` (no channel) when a file is attached, then private `files.share` on send. mautrix-slack uses this path for every xoxc/xoxs token. The public `files.getUploadURLExternal` + `files.completeUploadExternal` pair also works with xoxc and is the fallback.
- Every method here is a plain `POST https://<ws>.slack.com/api/<method>` call. No edgeapi (Flannel) file endpoints were observed.
- File bytes and thumbnails on `files.slack.com` load with the `d` cookie. The observed clients send either the cookie alone or `Authorization: Bearer <xoxc>` plus the cookie. Never send either credential to a non-Slack host (remote files).
- Biggest risks: (1) the upload/share/Files-view methods are all private and undocumented; (2) uploads finish asynchronously, so message events often carry stub file objects; (3) loading non-file Slack pages with the session cookie can end the session; (4) clip encoding, `media_display_type` and transcription fields are only partly observed.

## Endpoints

Shared conventions, unless a subsection says otherwise:

- Transport: `POST https://<ws>.slack.com/api/<method>`, `application/x-www-form-urlencoded` with `token=<xoxc>` in the body and `Cookie: d=<xoxd>`. OBSERVED: beeper/slackgo/misc.go:460-468, beeper/slackgo/slack.go:107-118. The live web client posts multipart form-data instead. OBSERVED: .omo/research/gist.md:79. JSON bodies are accepted when the token goes in `Authorization: Bearer`. OBSERVED: emacs-slack/emacs-slack/slack-request.el:211-241 (JSON plus Bearer plus `Cookie: d=` for xoxc tokens, slack-request.el:45-46).
- Optional web-client attribution fields: `_x_reason`, `_x_mode=online`, `_x_sonic=true`, `_x_app_name=client`. OBSERVED: beeper/slackgo/users.go:699-722, rusq/slackdump/internal/edge/edge.go:433-447.
- Envelope: `{"ok":true,...}` or `{"ok":false,"error":"<code>"}`. OBSERVED: https://docs.slack.dev/reference/methods/files.info

### files.getUploadURL

- Transport: POST form, see conventions. emacs-slack sends the params as a query string on a POST. OBSERVED: emacs-slack/emacs-slack/slack-message-attachment-preview-buffer.el:92-101
- Visibility: private. OBSERVED: https://docs.slack.dev/reference/methods/files.getUploadURL (HTTP 404, 2026-11-19)
- Request params:
  - `filename` - string - required - file name. Voice clips get `.m4a` appended. OBSERVED: mautrix/slack/pkg/msgconv/from-matrix.go:155-164
  - `length` - int - required - size in bytes. OBSERVED: some-du6e/clack/internal/slack/more.go:716, beeper/slackgo/files.go:413-424
  - `subtype` - string - optional - `slack_audio` for a voice clip. OBSERVED: mautrix/slack/pkg/msgconv/from-matrix.go:159-161,181-185, beeper/slackgo/files.go:394-398
  - `snippet_type` - string - optional - snippet language ("auto", "text" or a type key) when creating a snippet. OBSERVED: some-du6e/clack/internal/core/writes.go:319-334, some-du6e/clack/internal/core/types.go:455-458
  - `alt_txt` - string - optional - image alt text. OBSERVED: some-du6e/clack/internal/core/writes.go:335-337
  - `_x_*` - optional. OBSERVED: beeper/slackgo/files.go:425
- Response:
  ```json
  { "ok": true, "file": "F0123ABCDEF", "upload_url": "https://..." }
  ```
  OBSERVED: beeper/slackgo/files.go:401-405, emacs-slack/emacs-slack/slack-message-attachment-preview-buffer.el:84-85. clack also accepts `file_id` there (the public method's name for it). OBSERVED: some-du6e/clack/internal/slack/more.go:711-719
- Pagination: none.
- Used for: step 1 of every attachment. The web client uploads eagerly as soon as a file is attached (paste, + menu or drop), and the same path serves snippets, voice clips and Block Kit `file_input`. OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:30,33,86

### upload_url POST (file bytes)

- Transport: `POST <upload_url>` exactly as returned. The body is multipart/form-data with one part named `file` (filename set, part Content-Type = MIME type or octet-stream). OBSERVED: beeper/slackgo/files.go:470-505 (part header at :477), some-du6e/clack/internal/slack/client.go:854-902, emacs-slack/emacs-slack/slack-message-attachment-preview-buffer.el:58-68. For the public (External) slot a raw body also works: `Content-Type: application/octet-stream` plus `--data-binary`. OBSERVED: wee-slack/wee-slack@2.x/wee_slack.py:6377-6384, https://docs.slack.dev/reference/methods/files.getUploadURLExternal ("raw bytes or multipart")
- Visibility: private slot for files.getUploadURL. The External slot is documented as `https://files.slack.com/upload/v1/...`. OBSERVED: https://docs.slack.dev/reference/methods/files.getUploadURLExternal
- Request params / auth: none needed. The web client's XHR has no `withCredentials`, so the browser sends no cookie. OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:78. slackgo sends no token or cookie but sets `Origin: https://app.slack.com` and a browser User-Agent. OBSERVED: beeper/slackgo/files.go:495-497. clack (cookie) and emacs-slack (Bearer plus cookie) send credentials anyway and it still works. OBSERVED: some-du6e/clack/internal/slack/client.go:887, emacs-slack/emacs-slack/slack-request.el:211-241
- Response: HTTP 200 means success and the body is ignored. Anything else is a failure. OBSERVED: some-du6e/clack/internal/slack/client.go:893-897, https://docs.slack.dev/reference/methods/files.getUploadURLExternal ("a non-200 response indicates a failure"), https://docs.slack.dev/messaging/working-with-files (example body `OK - 12`). slackgo is looser and fails only on status >= 400. OBSERVED: beeper/slackgo/files.go:503-505
- Pagination: none.
- Used for: step 2 of both upload flows. Report progress from bytes written; Slack has no progress bar here, only a spinner. OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:42

### files.completeUpload

- Transport: POST form with `files` as a JSON string. emacs-slack sends a JSON body instead. OBSERVED: beeper/slackgo/files.go:446-465, emacs-slack/emacs-slack/slack-message-sender.el:738-763
- Visibility: private. OBSERVED: https://docs.slack.dev/reference/methods/files.completeUpload (HTTP 404, 2026-11-19)
- Request params:
  - `files` - JSON array - required - `[{"id":"F..","title":"name.png"}]`. Optional per-file `"subtype":"slack_audio"`. OBSERVED: beeper/slackgo/files.go:435-439,450-457, some-du6e/clack/internal/slack/more.go:733-745
  - Web client eager upload: no channel at all, so the file is processed but not shared. OBSERVED: some-du6e/clack/internal/slack/more.go:739-741, some-du6e/clack/docs/FILES-AUDIT.md:30
  - `channel`, `blocks`, `thread_ts`, `broadcast` - optional - emacs-slack folds the message into this call to upload and post in one request (`broadcast`, not `reply_broadcast`, when files are attached). OBSERVED: emacs-slack/emacs-slack/slack-message-sender.el:90-104, emacs-slack/emacs-slack/slack-thread-message-buffer.el:275-286
  - `_x_*` - optional. OBSERVED: beeper/slackgo/files.go:459
- Response:
  ```json
  { "ok": true, "files": [{ "id": "F0123ABCDEF", "title": "name.png" }] }
  ```
  OBSERVED: beeper/slackgo/files.go:441-444
- Pagination: none.
- Used for: step 3. The file then sits in PROCESSING until `file_created` arrives, or until polling finds it. See Gotchas. OBSERVED: some-du6e/clack/internal/core/upload_send.go:28-35

### files.share

- Transport: POST form, see conventions.
- Visibility: private. OBSERVED: https://docs.slack.dev/reference/methods/files.share (HTTP 404, 2026-11-19)
- Request params (what the web client's `shareFile` sends):
  - `files` - string - required - comma-joined IDs in message order, e.g. `F1,F2`. OBSERVED: some-du6e/clack/internal/slack/more.go:757,772, beeper/slackgo/files.go:538, some-du6e/clack/docs/FILES-AUDIT.md:31
  - `channel` - string - required. OBSERVED: some-du6e/clack/internal/slack/more.go:772, beeper/slackgo/files.go:539
  - `thread_ts` - string - optional. OBSERVED: some-du6e/clack/internal/slack/more.go:774-777, beeper/slackgo/files.go:551-553
  - `broadcast` - bool - optional - also post the thread reply to the channel. OBSERVED: some-du6e/clack/internal/slack/more.go:776, mautrix/slack/pkg/msgconv/from-matrix.go:206-211
  - `blocks` - JSON string - optional - the composer's `rich_text` blocks. A text-less send has neither `blocks` nor `comment`, and the share dialog sends `[]` when there is no note. OBSERVED: some-du6e/clack/internal/slack/more.go:781-782, some-du6e/clack/docs/FILES-AUDIT.md:44, some-du6e/clack/internal/core/channel_files_more.go:94-101
  - `comment` - string - optional - plain text, only when there are no blocks. OBSERVED: some-du6e/clack/internal/slack/more.go:783-784. slackgo sends `text` here instead. OBSERVED: beeper/slackgo/files.go:548-550
  - `client_msg_id` - UUID - optional - the echo with the same id replaces the optimistic placeholder. OBSERVED: some-du6e/clack/internal/slack/more.go:778-780, some-du6e/clack/docs/FILES-AUDIT.md:45-46
  - `resharing_aware=true`, `skip_dlp_user_warning=false`, `from_share_modal=false` - the web client always sends these. OBSERVED: some-du6e/clack/internal/slack/more.go:772-773
  - `unfurl`, `client_context_team_id` - field names only, commented out in the fork. OBSERVED: beeper/slackgo/files.go:519-523
- Response:
  ```json
  { "ok": true, "file_msg_ts": "1763550000.000100" }
  ```
  OBSERVED: beeper/slackgo/files.go:527-530, mautrix/slack/pkg/connector/handlematrix.go:136-141. clack reads `ts` first, then `file_msg_ts`. OBSERVED: some-du6e/clack/internal/core/channel_files_more.go:103-112
- Pagination: none.
- Used for: composer send with attachments, including multi-file drag-drop (one call carrying every ID), and the "Share file..." dialog. It goes out only after every upload has resolved. OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:44, some-du6e/clack/docs/FILES-AUDIT.md:387

### files.edit

- Transport: POST form, see conventions.
- Visibility: private. OBSERVED: https://docs.slack.dev/reference/methods/files.edit (HTTP 404, 2026-11-19)
- Request params:
  - `file` - string - required. OBSERVED: some-du6e/clack/internal/core/files_view.go:130-145
  - `title` - string - optional - at most 255 characters in the UI. OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:117
  - `alt_txt` - string - optional - `""` clears it. OBSERVED: some-du6e/clack/internal/core/files_view.go:131-134
  - `is_canvas_template=false` - the web client always sends it. OBSERVED: some-du6e/clack/internal/core/files_view.go:141-144
  - clack says these argument names are partly inferred (see LEADS). OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:79
- Response: `{"ok":true}`. Only `ok` is read. OBSERVED: some-du6e/clack/internal/slack/more.go:750-751
- Pagination: none.
- Used for: renaming a pending upload before the share, and "Edit file details" (name, alt text). OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:32,117

### files.getUploadURLExternal

- Transport: POST form. emacs-slack and legacy wee-slack send it as GET with a query string. OBSERVED: https://docs.slack.dev/reference/methods/files.getUploadURLExternal, emacs-slack/emacs-slack/slack-buffer.el:1076-1083, wee-slack/wee-slack@2.x/wee_slack.py:6340-6347,1505-1507
- Visibility: public, Tier 4 ("100+ per minute"). OBSERVED: https://docs.slack.dev/reference/methods/files.getUploadURLExternal
- Request params: `filename` (string, required), `length` (int, required, 0 gives `missing_argument`), `snippet_type` (string, optional), `alt_txt` (string, optional, max 1000 characters, else `alt_txt_too_large`). OBSERVED: https://docs.slack.dev/reference/methods/files.getUploadURLExternal, beeper/slackgo/files.go:674-683
- Response:
  ```json
  {
    "ok": true,
    "upload_url": "https://files.slack.com/upload/v1/ABC123...",
    "file_id": "F123ABC456"
  }
  ```
  OBSERVED: https://docs.slack.dev/reference/methods/files.getUploadURLExternal
- Pagination: none.
- Used for: the fallback upload path. emacs-slack and legacy wee-slack use it with xoxc tokens, and mautrix uses it for non-xoxc tokens. "upload v2" is the SDKs' name for this pair. OBSERVED: emacs-slack/emacs-slack/slack-buffer.el:1048-1083, wee-slack/wee-slack@2.x/wee_slack.py:6329-6347, mautrix/slack/pkg/msgconv/from-matrix.go:167-179, mautrix/slack/pkg/connector/client.go:102, https://docs.slack.dev/messaging/working-with-files

### files.completeUploadExternal

- Transport: POST with a form or JSON body (both documented); emacs-slack POSTs JSON. OBSERVED: https://docs.slack.dev/reference/methods/files.completeUploadExternal, emacs-slack/emacs-slack/slack-buffer.el:1091-1104. Legacy wee-slack builds the same params but sends every Web API call as a GET to `https://api.slack.com/api/<method>?<params>` (query string, no body). OBSERVED: wee-slack/wee-slack@2.x/wee_slack.py:6421-6434 (params), :1498-1507 (URL), :967-983 (request)
- Visibility: public, Tier 4. OBSERVED: https://docs.slack.dev/reference/methods/files.completeUploadExternal
- Request params:
  - `files` - JSON array - required - `[{"id":"F..","title":".."}]`
  - `channel_id` - string - optional - without it the file stays private
  - `channels` - string - optional - comma list
  - `thread_ts` - string - optional - parent ts, one channel only
  - `initial_comment` - string - optional - message text
  - `blocks` - JSON string - optional - ignored when `initial_comment` is set
  - `username`, `icon_url`, `icon_emoji` - bot-only customisation
    OBSERVED: https://docs.slack.dev/reference/methods/files.completeUploadExternal, beeper/slackgo/files.go:723-744 (blocks are sent only when there is no initial_comment, :733)
- Response: the documented example is `{"ok":true,"files":[{"id":"F123ABC456","title":"slack-test"}]}`. Real responses carry full file objects:
  ```json
  {
    "ok": true,
    "files": [
      {
        "id": "F0BJ8HNADEZ",
        "mode": "hosted",
        "mimetype": "",
        "size": 24069,
        "url_private": "https://files.slack.com/files-pri/T06UD92BS3C-F0BJ8HNADEZ/megadoc.docx",
        "media_display_type": "unknown",
        "shares": {},
        "file_access": "visible"
      }
    ]
  }
  ```
  OBSERVED: Bayselonarrend/OpenIntegrations/docs/en/results/Slack/UploadFile.log:1-41. To find the message ts, mautrix reads `files[0].shares.private|public[<channel>][0].ts`. OBSERVED: mautrix/slack/pkg/connector/handlematrix.go:117-125
- Pagination: none.
- Used for: finishing the fallback flow and sharing with `initial_comment` into a channel or thread. It can only be called once, and without it the upload is discarded. Slack Connect errors include `slack_connect_blocked_file_type`, `slack_connect_clip_sharing_blocked` and `slack_connect_file_upload_sharing_blocked`. OBSERVED: https://docs.slack.dev/reference/methods/files.completeUploadExternal

### files.upload

- Transport: POST multipart (`file`) or form (`content`). OBSERVED: https://docs.slack.dev/reference/methods/files.upload
- Visibility: public but deprecated, "officially sunset on November 12, 2025". Tier 2. OBSERVED: https://docs.slack.dev/reference/methods/files.upload
- Request params: `channels`, `content` (creates an editable snippet, 1 MB cap), `file`, `filename`, `filetype`, `initial_comment`, `thread_ts`, `title`. OBSERVED: https://docs.slack.dev/reference/methods/files.upload
- Response: `{"ok":true,"file":{...}}`. OBSERVED: https://docs.slack.dev/reference/methods/files.upload
- Pagination: none.
- Used for: nothing in slackless. emacs-slack keeps only the URL constant and uploads snippets through the v2 path (temp file plus filetype extension). OBSERVED: emacs-slack/emacs-slack/slack-file.el:63, emacs-slack/emacs-slack/slack-file.el:418-433

### files.info

- Transport: POST form, see conventions.
- Visibility: public, Tier 4. The web client also sends undocumented params. OBSERVED: https://docs.slack.dev/reference/methods/files.info
- Request params:
  - `file` - string - required. OBSERVED: https://docs.slack.dev/reference/methods/files.info
  - `count` (default 100), `page` (default 1), `cursor`, `limit` - optional - comment paging. The web client sends `count=0` to skip comments. OBSERVED: https://docs.slack.dev/reference/methods/files.info, some-du6e/clack/internal/core/files_view.go:124
  - `truncate=true` with `page=1&count=500` - private - snippet "Show more". OBSERVED: some-du6e/clack/internal/core/snippets.go:75, some-du6e/clack/docs/FILES-AUDIT.md:100
  - `reason` - string - private - e.g. `share-multiple-files-as-folder-items`. OBSERVED: some-du6e/clack/internal/core/bookmark_folders_more.go:238
- Response:
  ```json
  {
    "ok": true,
    "file": {
      "id": "F0S43PZDF",
      "mode": "hosted",
      "filetype": "gif",
      "...": "..."
    },
    "content": "...",
    "content_html": "...",
    "content_highlight_html": "<div class=...>",
    "content_highlight_html_truncated": false,
    "comments": [],
    "response_metadata": { "next_cursor": "dGVhbTpDMUg5UkVTR0w=" }
  }
  ```
  OBSERVED: https://docs.slack.dev/reference/methods/files.info, some-du6e/clack/internal/core/snippets.go:69-74, wee-slack/wee-slack/typings/slack_api/slack_files_info.pyi:50-55. `shares`, `channels`, `groups`, `ims`, `has_more_shares` and `comments_count` come only from files.info, not from history. OBSERVED: wee-slack/wee-slack/typings/slack_api/slack_files_info.pyi:40-47
- Pagination: comments via `cursor`/`limit` (`next_cursor`), or `count`/`page` (max count 1000, max page 100). OBSERVED: https://docs.slack.dev/reference/methods/files.info
- Errors to handle: `file_not_found`, `file_deleted`, `hidden_by_limit`, `not_visible`, plus the `canvas_*` codes. OBSERVED: mautrix/slack/pkg/connector/directmedia.go:107-110, some-du6e/clack/internal/core/browse.go:1033-1034, https://docs.slack.dev/reference/methods/files.info
- Used for: resolving uploads after `file_created`; refetching stub files after `file_change`; Slack Connect `check_file_info` stubs; snippet expansion; file details; picking a download URL. OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:41,57-62, mautrix/slack/pkg/msgconv/from-slack.go:335-341, mautrix/slack/pkg/connector/directmedia.go:62-84

#### File object reference (files.info, files.list, message `files[]`)

| Field(s)                                                                                                                                                                                                                                                                                               | Meaning / values                                                                                                                                                     | OBSERVED                                                                                                                                                                                                |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `id`, `created`, `timestamp`, `name`, `title`, `mimetype`, `filetype`, `pretty_type`, `user`, `user_team`, `size`                                                                                                                                                                                      | identity and type; `pretty_type` is the label shown on generic file cards                                                                                            | beeper/slackgo/files.go:31-48, slack-go/slack/files.go:38, some-du6e/clack/docs/FILES-AUDIT.md:24                                                                                                       |
| `mode`                                                                                                                                                                                                                                                                                                 | documented: `hosted`, `external`, `snippet`, `post`; also seen `quip` (canvas), `list`, `tombstone` (deleted), `hidden_by_limit`, `file_access` (Slack Connect stub) | https://docs.slack.dev/reference/objects/file-object, some-du6e/clack/internal/richtext/types.go:356, mautrix/slack/pkg/msgconv/from-slack.go:71-72, wee-slack/wee-slack/slack/slack_message.py:320-329 |
| `is_external`, `external_type`, `external_id`, `external_url`                                                                                                                                                                                                                                          | remote file; `external_type` one of `""`, `gdrive`, `dropbox`, `box`, `onedrive`, `app`, `undefined`, `salesforce`                                                   | slack-go/slack/files.go:43-46, https://docs.slack.dev/reference/objects/file-object                                                                                                                     |
| `url_private`, `url_private_download`                                                                                                                                                                                                                                                                  | `https://files.slack.com/files-pri/<T>-<F>/<name>`, download variant `/files-pri/<T>-<F>/download/<name>`                                                            | Bayselonarrend/OpenIntegrations/docs/en/results/Slack/UploadFile.log:25-26, https://docs.slack.dev/messaging/working-with-files                                                                         |
| `permalink`, `permalink_public`                                                                                                                                                                                                                                                                        | `https://<ws>.slack.com/files/<U>/<F>/<name>`; public `https://slack-files.com/<T>-<F>-<secret>`                                                                     | Bayselonarrend/OpenIntegrations/docs/en/results/Slack/UploadFile.log:28-29                                                                                                                              |
| `thumb_64`, `thumb_80`, `thumb_160`, `thumb_360`(+`_w`,`_h`,`_gif`), `thumb_480`(+`_w`,`_h`,`_gif`), `thumb_720`, `thumb_800`, `thumb_960`, `thumb_1024` (+`_w`,`_h`), `thumb_tiny`, `thumb_pdf`(+`_w`,`_h`), `deanimate`, `deanimate_gif`, `pjpeg`, `original_w`, `original_h`, `image_exif_rotation` | image derivatives; `thumb_80` is always 80x80 (padded); `thumb_360` fits in 360                                                                                      | slack-go/slack/files.go:58-87, https://docs.slack.dev/reference/objects/file-object                                                                                                                     |
| `thumb_video`, `thumb_video_w`, `thumb_video_h`, `thumb_video_ts`                                                                                                                                                                                                                                      | video poster frame                                                                                                                                                   | slack-go/slack/files.go:89-92                                                                                                                                                                           |
| `duration_ms`, `mp4`, `mp4_low`, `hls`, `hls_embed`, `hls_preview`, `vtt`, `media_display_type`                                                                                                                                                                                                        | video/audio renditions; `vtt` = captions                                                                                                                             | slack-go/slack/files.go:94-101, https://docs.slack.dev/reference/objects/file-object                                                                                                                    |
| `aac`, `audio_wave_samples`                                                                                                                                                                                                                                                                            | voice clip AAC URL; waveform as ints 0..100                                                                                                                          | slack-go/slack/files.go:103-104, mautrix/slack/pkg/msgconv/from-slack.go:455-458                                                                                                                        |
| `is_transcription_region_supported`, `transcription` `{status, locale, preview{content, has_more}}`                                                                                                                                                                                                    | clip transcript state                                                                                                                                                | slack-go/slack/files.go:105-106,177-188, slackapi/node-slack-sdk/packages/web-api/src/types/response/FilesInfoResponse.ts:612-621                                                                       |
| `subtype`                                                                                                                                                                                                                                                                                              | `slack_audio` (voice clip), `slack_video` (video clip)                                                                                                               | some-du6e/clack/internal/richtext/types.go:355, gastownhall/gascity-packs/slack-full/adapter/attachment_mime.go:13-14, urmzd/saige/flow/connectors/slack/files.go:28-30                                 |
| `preview`, `preview_highlight`, `lines`, `lines_more`, `preview_is_truncated`, `preview_plain_text`, `edit_link`                                                                                                                                                                                       | snippet/post previews (`preview_highlight` = server CodeMirror HTML)                                                                                                 | beeper/slackgo/files.go:80-84, slack-go/slack/files.go:145-147, https://docs.slack.dev/reference/objects/file-object                                                                                    |
| `is_public`, `public_url_shared`, `channels`, `groups`, `ims`, `shares.{public,private}.<channel>[{ts, thread_ts, reply_count, ...}]`, `has_more_shares`                                                                                                                                               | where the file is shared                                                                                                                                             | beeper/slackgo/files.go:86-95,125-138, slack-go/slack/files.go:127                                                                                                                                      |
| `file_access`                                                                                                                                                                                                                                                                                          | `visible`, `check_file_info`, `file_not_found`                                                                                                                       | wee-slack/wee-slack/typings/slack_api/slack_files_info.pyi:38                                                                                                                                           |
| `alt_txt`                                                                                                                                                                                                                                                                                              | image description                                                                                                                                                    | slack-go/slack/files.go:39                                                                                                                                                                              |

### files.list

- Transport: POST form, see conventions.
- Visibility: public, Tier 3. OBSERVED: https://docs.slack.dev/reference/methods/files.list
- Request params: `channel`, `user`, `ts_from`, `ts_to`, `types` (`all`, `spaces`, `snippets`, `images`, `gdocs`, `zips`, `pdfs`; comma-joined), `count` (100), `page` (1), `show_files_hidden_by_limit` (bool), `team_id` (org tokens). OBSERVED: https://docs.slack.dev/reference/methods/files.list. Undocumented `limit` + `cursor` are also sent. OBSERVED: beeper/slackgo/files.go:358-392. The web client's channel About modal sends `channel`, `count`, `page` and `_x_reason=about-modal/sharedFiles`. OBSERVED: rusq/slackdump/internal/edge/files.go:26-46
- Response:
  ```json
  {
    "ok": true,
    "files": [{ "id": "F0S43P1CZ", "mode": "hosted", "...": "..." }],
    "paging": { "count": 100, "total": 2, "page": 1, "pages": 1 },
    "response_metadata": { "next_cursor": "..." }
  }
  ```
  OBSERVED: https://docs.slack.dev/reference/methods/files.list, beeper/slackgo/files.go:218-226
- Pagination: `page` until the last page (slackdump loops on the page count), or `cursor`. OBSERVED: rusq/slackdump/internal/edge/files.go:47-66, beeper/slackgo/files.go:389
- Used for: the channel About modal's shared files. The channel Files tab and the Files browser search with `search.modules.files` instead (search area). OBSERVED: rusq/slackdump/internal/edge/files.go:45, some-du6e/clack/docs/FILES-AUDIT.md:311

### url_private GET (file and thumbnail download)

- Transport: `GET https://files.slack.com/files-pri/<T>-<F>/<name>`. `/download/<name>` forces a download; thumbnails are under `/files-tmb/`. clack also accepts hosts `files-*.slack.com` and paths `/files-pri-safe/` and `/files-origin/`. OBSERVED: some-du6e/clack/internal/slack/client.go:797-817, .omo/research/gist.md:634, beeper/slackgo/files_test.go:65
- Visibility: public with `Authorization: Bearer` (`files:read`). The cookie-only variant is the browser path. OBSERVED: https://docs.slack.dev/messaging/working-with-files
- Request params / auth, as observed with xoxc:
  - `Cookie: d=<xoxd>` only, no Authorization header. OBSERVED: some-du6e/clack/internal/slack/client.go:759-789
  - `Authorization: Bearer <xoxc>` plus `Cookie: d=<xoxd>`. slackgo sends them only to `https` URLs on exactly `files.slack.com`. OBSERVED: beeper/slackgo/misc.go:284-311 (check at :293). emacs-slack sends them to any `https` host whose name contains `slack` (the cookie only for xoxc tokens). OBSERVED: emacs-slack/emacs-slack/slack-request.el:402-418,445-464. Legacy wee-slack's file download sends them with no host check. OBSERVED: wee-slack/wee-slack@2.x/wee_slack.py:4265-4273,1528-1539
  - Bearer plus cookie jar, with redirects outside Slack hosts refused. OBSERVED: rusq/slackdump/internal/client/client.go:124-143, rusq/slack/misc.go:284-312
  - Official-export URLs carry `?t=<token>` and load with a plain GET. OBSERVED: rusq/slackdump/cmd/slackdump/internal/diag/hydrate.go:253-283
- Response: the raw bytes. An HTTP 200 with `text/html` (usually a sign-in redirect) means the auth was not accepted, so do not decode it as an image. mautrix retries once after 5 s. OBSERVED: some-du6e/clack/internal/slack/client.go:781-787, mautrix/slack/pkg/msgconv/from-slack.go:318-330,376-385. Map 404/410 to not found, 401/403 to forbidden and 429 to retry. OBSERVED: mautrix/slack/pkg/connector/directmedia.go:89-100
- Pagination: none.
- Used for: inline images, video posters, the lightbox, downloads, audio playback. Prefer `url_private_download`, then `url_private`. When a file has neither but has `permalink_public`, mautrix fetches that without Slack credentials. OBSERVED: mautrix/slack/pkg/msgconv/from-slack.go:347-353,386-399

### files.delete

- Transport: POST form, see conventions.
- Visibility: public, Tier 3. OBSERVED: https://docs.slack.dev/reference/methods/files.delete
- Request params: `file` - string - required. OBSERVED: https://docs.slack.dev/reference/methods/files.delete, some-du6e/clack/internal/core/manage.go:363-367
- Response: `{"ok":true}`. Errors include `cant_delete_file`, `delete_not_allowed` and `file_deleting_disabled`. It works on tombstoned files. OBSERVED: https://docs.slack.dev/reference/methods/files.delete
- Pagination: none.
- Used for: "Delete file" from the file menu, after a confirm dialog. Slack then sends `file_deleted` plus a message update. OBSERVED: some-du6e/clack/internal/core/manage.go:363-364, some-du6e/clack/docs/FILES-AUDIT.md:116

### files.sharedPublicURL

- Transport: POST form, see conventions.
- Visibility: public, Tier 3. OBSERVED: https://docs.slack.dev/reference/methods/files.sharedPublicURL
- Request params: `file` - string - required. OBSERVED: beeper/slackgo/files.go:649-653
- Response: `{"ok":true,"file":{..., "permalink_public":"https://slack-files.com/T024BE7LD-F024BERPE-8004f909b1"}}`. Only the file's creator may call it unless the file is in a public channel. OBSERVED: https://docs.slack.dev/reference/methods/files.sharedPublicURL
- Pagination: none.
- Used for: "Create external link" in the image file menu. OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:29

### files.revokePublicURL

- Transport: POST form, see conventions.
- Visibility: public, Tier 3. OBSERVED: https://docs.slack.dev/reference/methods/files.revokePublicURL
- Request params: `file` - string - required. OBSERVED: beeper/slackgo/files.go:625-629
- Response: `{"ok":true,"file":{...}}`. A tombstoned file comes back redacted with `hidden_by_limit`. OBSERVED: https://docs.slack.dev/reference/methods/files.revokePublicURL
- Pagination: none.
- Used for: revoking an external link.

### files.getOverview

- Transport: POST form, see conventions.
- Visibility: private. OBSERVED: https://docs.slack.dev/reference/methods/files.getOverview (HTTP 404, 2026-11-19)
- Request params: `file_id` - string - required. OBSERVED: some-du6e/clack/internal/core/channel_files_more.go:81
- Response:
  ```json
  {
    "ok": true,
    "users_count": 12,
    "channels_count": 3,
    "files_count": 0,
    "max_users_counted": false,
    "max_channels_counted": false,
    "max_files_counted": false
  }
  ```
  OBSERVED: some-du6e/clack/internal/core/channel_files_more.go:63-80
- Pagination: none.
- Used for: the share dialog's access line ("{n} users and {n} channels have access", "{count}+" when max-counted). OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:387

### files.hasShares

- Transport: POST form, see conventions.
- Visibility: private. OBSERVED: https://docs.slack.dev/reference/methods/files.hasShares (HTTP 404, 2026-11-19)
- Request params: `file_id`, `channel_id`, `exclude_channel_tab_shares=true`, `reason=remove_file_tab_confirmation_modal`. OBSERVED: some-du6e/clack/internal/core/channel_tabs_edit.go:331-332
- Response: `{"ok":true,"has_shares":false}`. OBSERVED: some-du6e/clack/internal/core/channel_tabs_edit.go:328-330
- Pagination: none.
- Used for: deciding whether removing a canvas or list tab needs a confirmation (channel-tabs area). OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:394

### files.recentlyDeleted

- Transport: POST form, see conventions.
- Visibility: private. OBSERVED: https://docs.slack.dev/reference/methods/files.recentlyDeleted (HTTP 404, 2026-11-19)
- Request params: `file_type` - string - optional - `quip` or `list`; omit for both. OBSERVED: some-du6e/clack/internal/core/files_view.go:155-162
- Response:
  ```json
  {
    "ok": true,
    "files": [
      {
        "id": "F9",
        "title": "Plan",
        "date_delete": 1763540000,
        "file_type": "quip"
      }
    ]
  }
  ```
  OBSERVED: some-du6e/clack/internal/core/files_view.go:163-170
- Pagination: none observed.
- Used for: the Files view's "Deleted" list (canvases and lists deleted in the last day, newest first). It loads when the sidebar mounts. OBSERVED: some-du6e/clack/internal/core/files_view.go:147-183, some-du6e/clack/docs/FILES-AUDIT.md:163-166

### files.undelete

- Transport: POST form, see conventions.
- Visibility: private. OBSERVED: https://docs.slack.dev/reference/methods/files.undelete (HTTP 404, 2026-11-19)
- Request params: `file_id` - string - required. OBSERVED: some-du6e/clack/internal/core/files_view.go:185-189
- Response: `{"ok":true}`, followed by a `file_undeleted` event. OBSERVED: some-du6e/clack/internal/core/events.go:412-415
- Pagination: none.
- Used for: "Restore canvas" / "Restore list" in Deleted. OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:164

### files.favorites.list

- Transport: POST form, see conventions.
- Visibility: private. OBSERVED: https://docs.slack.dev/reference/methods/files.favorites.list (HTTP 404, 2026-11-19)
- Request params: `type=all`. OBSERVED: some-du6e/clack/internal/core/files_view.go:63
- Response: `{"ok":true,"file_ids":["F1","F2"]}`. The web client reverses the list so the newest star comes first. OBSERVED: some-du6e/clack/internal/core/files_view.go:57-69, some-du6e/clack/docs/FILES-AUDIT.md:114
- Pagination: none observed.
- Used for: the Files view "Starred" section. When custom file sections are on, the client calls `files.collections.list` instead. OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:245

### files.favorites.add

- Transport: POST form, see conventions.
- Visibility: private. OBSERVED: https://docs.slack.dev/reference/methods/files.favorites.add (HTTP 404, 2026-11-19)
- Request params: `file_id` - required; `collection_id` - optional - add to a section. OBSERVED: some-du6e/clack/internal/core/files_view.go:103-117, some-du6e/clack/internal/core/files_sections.go:474
- Response: `{"ok":true}`. The star is applied optimistically and rolled back on failure. OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:113
- Pagination: none.
- Used for: starring a file, canvas or list; "Move file" into a section. OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:250

### files.favorites.remove

- Transport: POST form, see conventions.
- Visibility: private. OBSERVED: https://docs.slack.dev/reference/methods/files.favorites.remove (HTTP 404, 2026-11-19)
- Request params: `file_id` - required; `collection_id` - optional. OBSERVED: some-du6e/clack/internal/core/files_view.go:107-112, some-du6e/clack/internal/core/files_sections.go:505
- Response: `{"ok":true}`. OBSERVED: some-du6e/clack/internal/core/files_view.go:112
- Pagination: none.
- Used for: unstarring, or "Remove from {section}". OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:113,250

### files.collections.list

- Transport: POST form, no params. OBSERVED: some-du6e/clack/internal/core/files_sections.go:234
- Visibility: private. OBSERVED: https://docs.slack.dev/reference/methods/files.collections.list (HTTP 404, 2026-11-19)
- Request params: none.
- Response:
  ```json
  {
    "ok": true,
    "collections": [
      {
        "id": "...",
        "name": "...",
        "emoji": ":star:",
        "type": "...",
        "position": "a0",
        "sort": "date_added",
        "is_collapsed": false,
        "files": [{ "id": "F1", "position": "a0" }]
      }
    ]
  }
  ```
  OBSERVED: some-du6e/clack/internal/core/files_sections.go:85-97. `sort` is one of `date_added` (default), `recently_viewed`, `last_updated`, `alpha`. OBSERVED: some-du6e/clack/internal/core/files_sections.go:51-56
- Pagination: none. Refetched after a day. OBSERVED: some-du6e/clack/internal/core/files_sections.go:222
- Used for: Files view sidebar sections (custom_file_sections). OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:245

### files.collections.create

- Transport: POST form, see conventions.
- Visibility: private. OBSERVED: https://docs.slack.dev/reference/methods/files.collections.create (HTTP 404, 2026-11-19)
- Request params: `name` - required; `emoji` - optional. OBSERVED: some-du6e/clack/internal/core/files_sections.go:298-305
- Response: `{"ok":true,"collections":[{...new collection...}]}`. OBSERVED: some-du6e/clack/internal/core/files_sections.go:306-315
- Pagination: none.
- Used for: "Create a sidebar section". OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:248

### files.collections.update

- Transport: POST form, see conventions.
- Visibility: private. OBSERVED: https://docs.slack.dev/reference/methods/files.collections.update (HTTP 404, 2026-11-19)
- Request params: `collection` - required; `name` and `emoji` (rename), or `sort` = `recently_viewed` | `last_updated` | `alpha` | `date_added`. OBSERVED: some-du6e/clack/internal/core/files_sections.go:51-56,341,375, some-du6e/clack/docs/FILES-AUDIT.md:247-248
- Response: `{"ok":true,"collection":{...}}`. OBSERVED: some-du6e/clack/internal/core/files_sections.go:372-375
- Pagination: none.
- Used for: renaming and re-sorting a section.

### files.collections.delete

- Transport: POST form, see conventions.
- Visibility: private. OBSERVED: https://docs.slack.dev/reference/methods/files.collections.delete (HTTP 404, 2026-11-19)
- Request params: `collection` - required. OBSERVED: some-du6e/clack/internal/core/files_sections.go:397
- Response: `{"ok":true}`. OBSERVED: some-du6e/clack/internal/core/files_sections.go:397
- Pagination: none.
- Used for: "Delete sidebar section?". OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:249

### slack-imgs.com image proxy (GET)

- Transport: `GET https://slack-imgs.com/?c=1&o1=ro&url=<url-encoded original>`, with no credentials. OBSERVED: some-du6e/clack/internal/slack/hosts.go:119-142, mautrix/slack/pkg/msgconv/blocks.go:47-57. Sized variants appear in the live client, e.g. `?c=1&o1=wi32.he32.si&url=...`. OBSERVED: .omo/research/gist.md:638
- Visibility: private (undocumented proxy).
- Request params: `c=1`, `o1=<ops>` (`ro` is the "rotate" default, `wiN.heN.si` for sizes), `url` (required). OBSERVED: some-du6e/clack/internal/ui/popups/lightbox.go:235-238
- Response: image bytes.
- Pagination: none.
- Used for: third-party images (unfurls, attachments, bot icons, image blocks). Slack-owned hosts (slack.com, slack-edge.com, slack-imgs.com, slack-files.com) and Gravatar (`secure.gravatar.com`, `i0-i3.wp.com`) load directly without the proxy. OBSERVED: some-du6e/clack/internal/slack/hosts.go:67-82,111-117,128-142

### slack-edge.com CDN assets (GET)

- Transport: plain `GET` with no credentials. OBSERVED: some-du6e/clack/internal/core/files.go:19-23
- Visibility: private CDN.
- URL shapes:
  - Avatar fallback: `https://ca.slack-edge.com/<team>-<user>-<avatar_hash>-512`, used when `image_512`/`image_original` are missing. OBSERVED: mautrix/slack/pkg/connector/chatinfo.go:494-505
  - Default avatars: `https://a.slack-edge.com/df10d/img/avatars/ava_0002-<size>.png`. OBSERVED: some-du6e/clack/internal/core/sender.go:118-129
  - Team icons: `https://a.slack-edge.com/80588/img/avatars-teams/ava_0011-<size>.png`. OBSERVED: beeper/slackgo/slackevents/inner_events_test.go:1309-1315
  - Custom emoji: `https://emoji.slack-edge.com/<team>/<name>/<hash>.<ext>`. OBSERVED: mautrix/slack/pkg/slackid/mediaid.go:77-111, mautrix/slack/pkg/slackid/mediaid_test.go:40
- Response: image bytes.
- Pagination: none.
- Used for: avatars and emoji rendering (the profile and emoji areas own the APIs that return these URLs).

Cross-area methods touched by files features (owned by sibling docs):

- `search.modules.files`, `search.autocomplete.files` (Files browser, channel Files tab, file autocomplete). OBSERVED: .omo/research/gist.md:491,504, some-du6e/clack/docs/FILES-AUDIT.md:311,386
- `conversations.searchLinks` (channel Links tab). OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:311
- `saved.add` / `saved.delete` with `item_type=file` (Save for later). OBSERVED: some-du6e/clack/internal/core/manage.go:347,356
- `files.open`, `files.close`, `files.markRead`, `files.requestPermission` (canvases). OBSERVED: some-du6e/clack/internal/core/canvas.go:472,560,704, some-du6e/clack/internal/core/tab_peeks.go:123
- `files.remote.*` (apps' remote files). OBSERVED: beeper/slackgo/remotefiles.go:133-340
- `chat.postMessage` with `rich_text_preformatted` for inline code blocks, the alternative to a snippet file. OBSERVED: beeper/slackgo/block_rich_text.go:91

## Websocket events

All of these arrive on the RTM/websocket connection. The JSON excerpts show the inner event.

- `file_created`: `{"type":"file_created","file_id":"F..","file":{"id":"F.."},"user_id":"U..","event_ts":"1763550000.000100"}`. Fires when an upload finishes processing. The web client resolves the pending upload on it, then calls files.info. mautrix shares only after it sees the event for its own `user_id`, and waits at most 5 s. OBSERVED: beeper/slackgo/websocket_files.go:4-14, some-du6e/clack/internal/fakeslack/drafts.go:191-192, https://docs.slack.dev/reference/events/file_created, some-du6e/clack/internal/core/upload_send.go:28-35, mautrix/slack/pkg/connector/handleslack.go:138-151, mautrix/slack/pkg/connector/handlematrix.go:83-95
- `file_change`: `{"type":"file_change","file_id":"F..","file":{"id":"F.."}}`. Fires when a file changes, notably when processing finishes after its message was already delivered. Refetch with files.info and update every loaded message holding that file. OBSERVED: https://docs.slack.dev/reference/events/file_change, some-du6e/clack/internal/core/events.go:410-411, some-du6e/clack/docs/FILES-AUDIT.md:57-62
- `file_shared`: `{"type":"file_shared","channel_id":"D..","file_id":"F..","user_id":"U..","file":{"id":"F.."},"event_ts":"..."}`. Fires when a file is shared into a conversation. Handle it like `file_change`. OBSERVED: https://docs.slack.dev/reference/events/file_shared, some-du6e/clack/internal/core/events.go:410
- `file_public`: `{"type":"file_public","file_id":"F..","file":{"id":"F.."}}`. Fires when a public link is made. Handle it like `file_change`. OBSERVED: https://docs.slack.dev/reference/events/file_public, some-du6e/clack/internal/core/events.go:410
- `file_unshared`: `{"type":"file_unshared","file_id":"F..","file":{"id":"F.."}}`. emacs-slack treats it like a delete. OBSERVED: https://docs.slack.dev/reference/events/file_unshared, emacs-slack/emacs-slack/slack-websocket.el:482-484
- `file_deleted`: `{"type":"file_deleted","file_id":"F..","event_ts":"..."}`. The message update follows. OBSERVED: https://docs.slack.dev/reference/events/file_deleted, some-du6e/clack/internal/core/manage.go:363-364
- `file_undeleted`: carries `file_id`(s). Used by the Files view trash. OBSERVED: some-du6e/clack/internal/core/events.go:412-415
- `file_private`, and `file_comment_added` / `file_comment_edited` / `file_comment_deleted` (legacy comments): same `fileActionEvent` envelope, comment events add a `comment`. OBSERVED: beeper/slackgo/websocket_managed_conn.go:615-624, beeper/slackgo/websocket_files.go:31-50
- `message` with files: `files[]` holds file objects (or stubs), `upload: true` is set by the server, and the legacy subtype is `file_share`. OBSERVED: beeper/slackgo/messages.go:49,124-128, some-du6e/clack/docs/FILES-AUDIT.md:44

## Gotchas

- **Which upload flow.** For xoxc, mirror the web client: `files.getUploadURL` -> POST bytes -> `files.completeUpload` (no channel) -> wait for processing -> `files.share`. mautrix picks this path whenever the token starts with `xoxc-`/`xoxs-`. OBSERVED: mautrix/slack/pkg/connector/client.go:102, mautrix/slack/pkg/msgconv/from-matrix.go:180-215. The public pair works too, but it posts its own message via `initial_comment`, and the web client's `client_msg_id`/blocks placeholder echo does not apply to it. OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:31
- **Processing is asynchronous.** Wait for `file_created`. After 60 s, or without a websocket, poll with files.info: first after 7.5 s, then doubling to 30 s, for up to 10 min. Retry the first failed upload once. Share only when every upload has resolved; otherwise the echoed message carries stubs. OBSERVED: some-du6e/clack/internal/core/upload_send.go:28-57, some-du6e/clack/docs/FILES-AUDIT.md:41,44
- **Stub files in messages.** A message can arrive before processing finishes. Its file then has `name`, `url_private` and `permalink`, but empty `mimetype`/`filetype`/`pretty_type`, `size` 0, `media_display_type: "unknown"` and no thumbs. Treat a hosted file with no mimetype and no filetype as a stub, retry files.info for about 20 s, and refetch on `file_change`. OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:57-62
- **Slack Connect.** Files can come as `{"mode":"file_access","file_access":"check_file_info"}` stubs; call files.info to get the real object. OBSERVED: https://docs.slack.dev/reference/objects/file-object, mautrix/slack/pkg/msgconv/from-slack.go:335-341, wee-slack/wee-slack/slack/slack_message.py:312-316. Externally shared channels are not offered some snippet types (aspx, javascript, jsp, vb, vbscript). OBSERVED: some-du6e/clack/internal/ui/composer/snippet.go:64-66
- **Gone or hidden files.** `mode: tombstone` means deleted, `mode: hidden_by_limit` means over a free-plan limit, and `file_access: file_not_found` means missing. Render a placeholder instead of fetching. OBSERVED: wee-slack/wee-slack/slack/slack_message.py:320-334, mautrix/slack/pkg/msgconv/from-slack.go:71-72, emacs-slack/emacs-slack/slack-file.el:638
- **Remote/external files leak credentials.** For `is_external: true` / `mode: external` / `filetype: remote`, `url_private` is an app-chosen URL. Check the scheme and the exact host before attaching the cookie or token, and re-check every redirect. OBSERVED: https://docs.slack.dev/messaging/working-with-files, beeper/slackgo/misc.go:293, rusq/slackdump/internal/client/client.go:124-143
- **Session safety.** Send the session cookie only to file URLs. clack refuses to GET other slack.com pages with the cookie, because doing that outside the browser "can end the session everywhere". OBSERVED: some-du6e/clack/internal/slack/client.go:791-795
- **HTML instead of bytes.** A `text/html` 200 from a file URL is a sign-in redirect or an unprocessed file, not content. OBSERVED: some-du6e/clack/internal/slack/client.go:781-787, mautrix/slack/pkg/msgconv/from-slack.go:376-385
- **Enterprise Grid token routing.** With an org token, some methods fail with `team_is_restricted` or `enterprise_is_restricted`. emacs-slack forces the team token for `files.getUploadURLExternal`/`files.completeUploadExternal`. clack learns the right host per method and retries. `files.list` needs `team_id` with org tokens. OBSERVED: emacs-slack/emacs-slack/slack-request.el:215-238, some-du6e/clack/internal/slack/client.go:431-443, https://docs.slack.dev/reference/methods/files.list
- **Rate limits.** getUploadURLExternal, completeUploadExternal and info are Tier 4; list, delete, sharedPublicURL and revokePublicURL are Tier 3; the deprecated files.upload is Tier 2. slackdump paces files.list at tier3 (about 1 call per 1.2 s, burst 4). OBSERVED: https://docs.slack.dev/reference/methods/files.getUploadURLExternal, https://docs.slack.dev/reference/methods/files.completeUploadExternal, https://docs.slack.dev/reference/methods/files.info, https://docs.slack.dev/reference/methods/files.list, https://docs.slack.dev/reference/methods/files.delete, https://docs.slack.dev/reference/methods/files.upload, rusq/slackdump/internal/edge/limits.go:39
- **Size and quota limits.** Snippets max 1 MB (`snippet_too_large`); `alt_txt` max 1000 characters; free teams stop at 5 GB (`storage_limit_reached`); admins can disable uploads (`file_uploads_disabled`, `file_uploads_except_images_disabled`, `file_upload_size_restricted`); external guests cannot upload (`user_is_external_guest`). OBSERVED: https://docs.slack.dev/reference/methods/files.getUploadURLExternal
- **Voice clips.** The web client uploads webm/opus bytes named `*.m4a` with `subtype=slack_audio`, and the server stores AAC in MP4 (`audio_message.m4a`, the `aac` field). The waveform `audio_wave_samples` is 0..100. Voice clips render as a flat player with a waveform. OBSERVED: mautrix/slack/pkg/msgconv/from-matrix.go:155-159, mautrix/slack/pkg/msgconv/from-slack.go:420-424, gastownhall/gascity-packs/slack-full/adapter/attachment_mime.go:13-14,43-45, some-du6e/clack/docs/FILES-AUDIT.md:27
- **Video clips.** `subtype: slack_video` (video/mp4) carries `thumb_video*`, `mp4`/`mp4_low`, `hls*`, `vtt` and `transcription`. Galleries size them by `original_w/h` or the `thumb_video` dimensions and show a duration pill. The video menu has "generate_transcript". OBSERVED: gastownhall/gascity-packs/slack-full/adapter/attachment_mime.go:45, slack-go/slack/files.go:89-106, some-du6e/clack/docs/FILES-AUDIT.md:22,29
- **`media_display_type`** shows up as `"unknown"` on documents and unprocessed or huge images. The web client never branches on it, so do not drive rendering from it. OBSERVED: Bayselonarrend/OpenIntegrations/docs/en/results/Slack/GetFileData.log:26, some-du6e/clack/docs/FILES-AUDIT.md:64
- **Is it an image?** Slack's `fileIsImage` requires a `mimetype` containing `image/` AND (`thumb_360` or `thumb_360_gif`, or a HEIC/HEIF with `thumb_800`, or a Salesforce file). Huge images (e.g. 828x25208) get no thumbs and render as a file card; the original is never drawn inline. OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:68
- **Thumbnail sizes.** The teardown saw 64, 80, 160, 360, 480, 720, 800, 960 and 1024 px, all pre-generated. Every `thumb_*` needs the same auth as `url_private`. OBSERVED: .omo/research/gist.md:634, https://docs.slack.dev/reference/objects/file-object
- **Snippets.** Creating one = an upload with `snippet_type`, titled with the title or "Untitled". Display uses `preview_highlight` (server CodeMirror HTML); "Show more" fetches `content_highlight_html` via files.info `truncate=true`; the client cuts the content at 12,800 characters. Only `mode: "snippet"` renders as code; hosted text files are cards. OBSERVED: some-du6e/clack/internal/ui/composer/snippet.go:58-62, some-du6e/clack/docs/FILES-AUDIT.md:98-101
- **Multiple files / drag-drop.** Drop, paste and the + menu share one attach path. Each file is eagerly uploaded on its own and the whole set goes out in one `files.share` (Shift-drop shares at once). Galleries show at most 4 tiles (by width) plus "+N". emacs-slack caps attachments at 10 per message. OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:20,86, emacs-slack/emacs-slack/slack-message-attachment-preview-buffer.el:42
- **Rendering.** slack-libpurple and legacy wee-slack just link `url_private` (falling back to `permalink`). That is enough for a fallback, not for parity. OBSERVED: dylex/slack-libpurple/slack-message.c:347-357, wee-slack/wee-slack@2.x/wee_slack.py:5108-5112
- **Evidence quality.** clack's audit reads Slack's shipped JS bundle (gantry-v2 builds a84662d2d135 / 2c0d02ca130f / c07ab895506f), not live traffic. Re-verify private parameters with a live capture before relying on them. OBSERVED: some-du6e/clack/docs/FILES-AUDIT.md:3-9

## LEADS

- `files.uploadAsync` does not seem to exist. Its docs page is a 404, and no source mentions it.
- `upload_url` returned by private `files.getUploadURL`: host and path probably match the External slot (`https://files.slack.com/upload/v1/...`). Not observed.
- Video clip recording: probably uploaded via `files.getUploadURL` with `subtype=slack_video` (by analogy with `slack_audio`), possibly as webm, then transcoded server-side to `mp4`/`hls`. The recorded container, the `filetype` (`mp4` vs `webm`) and whether `subtype` also goes in the `files.completeUpload` JSON are unverified.
- `media_display_type` values other than `"unknown"` (perhaps `video`, `audio`, `image`) are unverified.
- `transcription.status` values (perhaps `none`, `processing`, `complete`, `failed`), and the API behind the "generate_transcript" menu item, are unknown.
- Whether `Authorization: Bearer <xoxc>` without the `d` cookie works for `url_private` is unverified. Plan to always send the cookie.
- The `slack-imgs.com` `o1` op grammar (`wi`/`he`/`si`/`ro`) and any signing or other params are inferred from two samples only.
- `files.edit` argument names (`file`, `title`, `alt_txt`) are partly inferred by clack. Editing a snippet's content probably uses `files.edit` with `content`.
- `files.collections.reorder` is named in clack's audit (some-du6e/clack/docs/FILES-AUDIT.md:274) as Slack's drag-to-reorder for sections, and its docs page is a 404. Its params are unknown.
- `files.completeUpload` with `channel`/`blocks` (emacs-slack's one-call style) may be an older server behaviour. The current web client always uses `files.share`.
- `files.share` may accept `text` (slackgo) as well as `comment` (web client). Which one wins is unknown.
- Real limits: max files per message (emacs-slack uses 10) and max clip duration (mautrix declares 5 min for voice, mautrix/slack/pkg/connector/capabilities.go:174) are unconfirmed for Slack itself.
- `files.upload` with xoxc after the 2025-11-12 sunset is unverified. Assume it is gone.
- `pjpeg`, `deanimate_gif`, `thumb_480_gif` and `thumb_1024_gif` may only appear for some image types.
- `file_created` delivery on Enterprise Grid / `wss-primary` may differ from classic RTM. Keep the files.info polling fallback.
- `files.sharedPublicURL` returns the file with `"public_url_shared": true` - unverified: the cited docs example (https://docs.slack.dev/reference/methods/files.sharedPublicURL) shows `"public_url_shared": false`. Check a live response before relying on the flag.
