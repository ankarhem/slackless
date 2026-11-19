# Canvases, Lists and Clips/Posts - Slack web-client API

## Summary

- **Scope:** canvases (`filetype: "quip"`), channel/DM tabs, Slack Lists (`filetype: "list"`), clips (audio/video message files plus `media.*`) and legacy Posts (`filetype: "post"`). All of them are Slack _files_, so `files.info`, file permalinks, `file_*` RTM events and a per-file comment channel (file id with `F` replaced by `C`) are the shared spine.
- **Public:** `canvases.*` (create/edit/delete/access/sections.lookup plus the recent `canvases.getContent`, which returns markdown or HTML) and `slackLists.*` (public since 2025-09-02).
- **Private (what the web client actually calls):** Quip's binary protobuf endpoints under `https://<ws>.slack.com/canvas/…` for canvas documents, wrapped by `files.open/close/ping`, `files.createCanvas` and `quip.*`; `lists.records.*`, `lists.cells.update`, `lists.views.*` for Lists; `conversations.addTab/updateTab/removeTab` for tabs; `media.*` for clips.
- **Content formats:** canvas markdown (`canvases.getContent`), canvas HTML (the Quip export behind the file's `url_private`, which keeps section ids, or `canvases.getContent content_type=html`), Quip protobuf (editor only); Lists are JSON records plus a `list_metadata` schema.
- **Biggest risks:** the Quip protobuf is undocumented and versioned per Quip build; `canvases.getContent` is new and untested with `xoxc`; the HTML export is a scraped format; canvas markdown cannot express underline, dates, callouts, layouts or comment anchors; private `lists.*` and public `slackLists.*` differ in names and argument shapes.
- **Plan:** read-only first (HTML export to native blocks, Lists via `files.info` + `lists.records.list`, refresh on RTM events), then section-level edits through public `canvases.edit` and private `lists.cells.update`; leave the Quip protocol for last (see "Feasibility assessment and staged plan").

## Endpoints

**Conventions for every method below unless its subsection says otherwise.**

- Transport: `POST https://<workspace>.slack.com/api/<method>`, header `Cookie: d=<xoxd…>`, form body with `token=<xoxc…>`; array and object arguments are JSON strings inside the form. OBSERVED: some-du6e/clack/docs/DESKTOP-SIGNIN.md:9; some-du6e/clack/docs/DESKTOP-SIGNIN.md:292; beeper/slackgo/canvas.go:137-156; some-du6e/clack/docs/CANVASES.md:72
- The web client snake_cases argument keys and tags calls with `_x_reason` (slackdump sends `fetch-quip-ids` and `canvas-comments`); the generic `_x_*` envelope belongs to the transport reference. OBSERVED: some-du6e/clack/docs/LISTS.md:107; rusq/slackdump/internal/edge/canvas.go:117; rusq/slackdump/internal/edge/canvas.go:396
- "Catalogue" means the method or argument name appears in the live web-client bundle (build 137958, `versionTs` 1795093060, mined 2026-11-19). The catalogue holds names only, so for catalogue-only arguments the type and required-ness are written `?`. OBSERVED: 3kh0/slack-datamine/build/meta.json:2-6
- Public methods also accept `application/json` bodies; rate tiers are quoted from each docs page. OBSERVED: https://docs.slack.dev/reference/methods/canvases.create

---

**Canvases: public methods**

### canvases.getContent

- **Transport:** `POST /api/canvases.getContent`, form or JSON.
- **Visibility:** public (scope `canvases:read`, tier 3, bot and user tokens). OBSERVED: https://docs.slack.dev/reference/methods/canvases.getContent
- **Request params:**
  - `canvas_id` - string - required - canvas file id (`F…`).
  - `content_type` - string - optional - `markdown` (default) or `html`.
    OBSERVED: https://docs.slack.dev/reference/methods/canvases.getContent; slack-go/slack/canvas.go:61-69; slack-go/slack/canvas.go:150-152
- **Response:** `{"ok": true, "content": "# Project plan\n\n- [ ] Draft spec\n- [x] Kickoff meeting\n"}`; `canvas_not_found` when the canvas is missing or the caller can't view it. OBSERVED: https://docs.slack.dev/reference/methods/canvases.getContent; slack-go/slack/canvas.go:153-160
- **Pagination:** none; the whole canvas comes back in `content`. OBSERVED: https://docs.slack.dev/reference/methods/canvases.getContent
- **Used for:** read-only rendering and read-before-edit; the markdown is the dialect `canvases.create`/`canvases.edit` accept. The Rust CLI `slk` uses it for `canvas read` (markdown or `--format html`). OBSERVED: https://docs.slack.dev/reference/methods/canvases.getContent; https://docs.rs/crate/slk/latest/source/docs/canvas.md
- **Age and client use:** first wrapped by slack-go in 0.34.0 (2026-11-03). It is absent from the web client's catalogue, whose `canvases.*` entries are only `agents.*`, `drafts.get`, `getCannedTemplates`, `getTemplates`, `listHeaders`, `notificationSections`. OBSERVED: slack-go/slack/CHANGELOG.md:237; slack-go/slack/CHANGELOG.md:324-325; 3kh0/slack-datamine/build/api-methods.txt:364-370

### canvases.sections.lookup

- **Transport:** `POST /api/canvases.sections.lookup`, form or JSON.
- **Visibility:** public (`canvases:read`, tier 3). OBSERVED: https://docs.slack.dev/reference/methods/canvases.sections.lookup
- **Request params:**
  - `canvas_id` - string - required.
  - `criteria` - JSON object - required - `{"section_types": [...], "contains_text": "..."}`. Section types: `any_header`, `blockquote`, `callout`, `canvas_unfurl`, `chart`, `citation`, `code_block`, `file_unfurl`, `flexbox`, `h1`, `h2`, `h3`, `horizontal_line`, `list`, `message_unfurl`, `sfdc_record_mention`, `sfdc_record_unfurl`, `table`, `user_mention`, `user_unfurl`.
    OBSERVED: https://docs.slack.dev/reference/methods/canvases.sections.lookup; beeper/slackgo/canvas.go:259-276
- **Response:** `{"ok": true, "sections": [{"id": "temp:C:eBa219af721c664422cb90a52fac"}]}`. It returns ids only: no bodies, and no guarantee that every section is listed. OBSERVED: https://docs.slack.dev/reference/methods/canvases.sections.lookup; https://docs.rs/crate/slk/latest/source/docs/canvas.md
- **Pagination:** none documented.
- **Used for:** finding the `section_id` that `canvases.edit` targets. The same `temp:C:…` ids are the `id` attributes in the HTML export, so a renderer built on the export never needs this call. OBSERVED: some-du6e/clack/docs/CANVASES.md:68

### canvases.edit

- **Transport:** `POST /api/canvases.edit`, form or JSON.
- **Visibility:** public (`canvases:write`, tier 3). OBSERVED: https://docs.slack.dev/reference/methods/canvases.edit
- **Request params:**
  - `canvas_id` - string - required.
  - `changes` - JSON array with exactly 1 element - required - `{operation, section_id?, document_content?, title_content?}`:
    - `insert_after` / `insert_before`: `section_id` + `document_content`;
    - `insert_at_start` / `insert_at_end`: `document_content`;
    - `replace`: `document_content`, optional `section_id` (omitted = the whole canvas);
    - `delete`: `section_id`;
    - `rename`: `title_content: {"type": "markdown", "markdown": "…"}`, no `section_id`.
      `document_content` is `{"type": "markdown", "markdown": "…"}`, at most 1 MiB per change.
      OBSERVED: https://docs.slack.dev/reference/methods/canvases.edit; beeper/slackgo/canvas.go:137-156
- **Response:** `{"ok": true}`; the wrappers read no other field. OBSERVED: beeper/slackgo/canvas.go:137-161; some-du6e/clack/internal/core/canvas.go:690
- **Pagination:** n/a.
- **Used for:** every canvas edit clack makes with its `xoxc` session: block replace, insert after, delete an emptied block, checklist toggle, rename. OBSERVED: some-du6e/clack/docs/CANVASES.md:72-79; some-du6e/clack/docs/CANVASES.md:91-96

### canvases.create

- **Transport:** `POST /api/canvases.create`, form or JSON.
- **Visibility:** public (`canvases:write`, tier 2). OBSERVED: https://docs.slack.dev/reference/methods/canvases.create
- **Request params:**
  - `title` - string - optional.
  - `document_content` - JSON `{"type": "markdown", "markdown": "…"}` - optional - at most 1 MiB.
  - `channel_id` - string - optional - tabs the new canvas into that channel; required on free teams.
    OBSERVED: https://docs.slack.dev/reference/methods/canvases.create; slack-go/slack/CHANGELOG.md:500-502
- **Response:** `{"ok": true, "canvas_id": "F1234ABCD"}`. Bad markdown returns `{"ok": false, "error": "canvas_creation_failed", "detail": "'content' error: line 28: Unsupported block type (List) within block quote"}`. Free-plan errors are `free_teams_cannot_create_standalone_canvases` and `free_team_canvas_tab_already_exists`. OBSERVED: https://docs.slack.dev/reference/methods/canvases.create
- **Pagination:** n/a.
- **Used for:** "+ New Canvas" in clack's Files view. The web client's own create call is `files.createCanvas` (below). OBSERVED: some-du6e/clack/docs/CANVASES.md:80; some-du6e/clack/docs/CANVASES.md:97; 3kh0/slack-datamine/build/api-methods.txt:658

### canvases.delete

- **Transport:** `POST /api/canvases.delete`, form or JSON.
- **Visibility:** public (`canvases:write`, tier 3). OBSERVED: https://docs.slack.dev/reference/methods/canvases.delete
- **Request params:** `canvas_id` - string - required. OBSERVED: https://docs.slack.dev/reference/methods/canvases.delete; beeper/slackgo/canvas.go:108-121
- **Response:** `{"ok": true}`; `canvas_not_found` otherwise. Deletion can't be undone. OBSERVED: https://docs.slack.dev/reference/methods/canvases.delete
- **Pagination:** n/a.
- **Used for:** deleting a canvas. The web client's own delete call was not observed; clack doesn't implement delete. OBSERVED: some-du6e/clack/docs/CANVASES.md:109

### canvases.access.set

- **Transport:** `POST /api/canvases.access.set`, form or JSON.
- **Visibility:** public (`canvases:write`, tier 3). OBSERVED: https://docs.slack.dev/reference/methods/canvases.access.set
- **Request params:**
  - `canvas_id` - string - required.
  - `access_level` - string - required - `read`, `write` or `owner`.
  - `channel_ids` - JSON array (1-20) - optional - XOR with `user_ids`; DM/MPDM ids are rejected.
  - `user_ids` - JSON array (1-20) - optional - `owner` works only with users and only when the current owner calls it.
    OBSERVED: https://docs.slack.dev/reference/methods/canvases.access.set; beeper/slackgo/canvas.go:172-200
- **Response:** `{"ok": true}`. Passing both id lists or neither returns `invalid_parameters`; a channel canvas returns `canvas_not_found`. OBSERVED: https://docs.slack.dev/reference/methods/canvases.access.set
- **Pagination:** n/a.
- **Used for:** the Share dialog ("Can edit" / "Can view"). After the call the canvas permalink is posted to each conversation, and Slack unfurls it as the canvas card. There is no "can comment" level. OBSERVED: some-du6e/clack/docs/CANVASES.md:82; some-du6e/clack/docs/CANVASES.md:152; some-du6e/clack/internal/core/canvas.go:753

### canvases.access.delete

- **Transport:** `POST /api/canvases.access.delete`, form or JSON.
- **Visibility:** public (`canvases:write`, tier 3). OBSERVED: https://docs.slack.dev/reference/methods/canvases.access.delete
- **Request params:** `canvas_id` - string - required; `channel_ids` - JSON array of exactly 1 - optional; `user_ids` - JSON array of exactly 1 - optional. OBSERVED: https://docs.slack.dev/reference/methods/canvases.access.delete; beeper/slackgo/canvas.go:216-243
- **Response:** `{"ok": true}`. A grant made at channel level can't be revoked for one user. OBSERVED: https://docs.slack.dev/reference/methods/canvases.access.delete
- **Pagination:** n/a.
- **Used for:** revoking a share.

### conversations.canvases.create

- **Transport:** `POST /api/conversations.canvases.create`, form or JSON.
- **Visibility:** public (`canvases:write`, tier 2). OBSERVED: https://docs.slack.dev/reference/methods/conversations.canvases.create
- **Request params:** `channel_id` - string - required; `title` - string - optional; `document_content` - JSON markdown object - optional. OBSERVED: https://docs.slack.dev/reference/methods/conversations.canvases.create; slack-go/slack/conversation.go:1057-1085
- **Response:** `{"ok": true, "canvas_id": "F1234ABCD"}`. If a canvas already exists the call returns `channel_canvas_already_exists`; read that canvas's id from `conversations.info` → `channel.properties.canvas`. A canvas created empty doesn't change the channel's canvas icon. OBSERVED: https://docs.slack.dev/reference/methods/conversations.canvases.create
- **Pagination:** n/a.
- **Used for:** clack's tab bar "+ → Canvas". This method is not in the web client's catalogue; the catalogue's tab method is `conversations.addTab`. OBSERVED: some-du6e/clack/docs/CANVASES.md:81; some-du6e/clack/docs/CANVASES.md:97; 3kh0/slack-datamine/build/api-methods.txt:453

---

**Canvases: web-client (private) surface**

### files.info (canvas, list, clip and post fields)

- **Transport:** `POST /api/files.info`, form. The generic method belongs to the Files reference; only type-specific fields are listed here. OBSERVED: slack-go/slack/files.go:321-323; slack-go/slack/files.go:339-347
- **Visibility:** public method; most fields below are undocumented.
- **Request params:** `file` - string - sent by every client - file id; `count`, `page` - int - optional - comment paging. OBSERVED: slack-go/slack/files.go:339-344
- **Response, canvas** (`filetype: "quip"`, `mode: "quip"`, `pretty_type: "Canvas"`): `quip_thread_id`, `title`, `title_blocks` (rich text), `canvas_creator_id`, `edit_timestamp`, `editable`, `access` (`read`/`write`/`owner`), `editors`, `canvas_readtime`, `show_badge`, `is_starred`, `dm_mpdm_users_with_file_access`, `org_or_workspace_access`, `canvas_printing_enabled`, `has_rich_preview`, `source_canvas_id`, `url_static_preview`, `permalink` (`https://<ws>.slack.com/docs/<T>/<F>`), `team_pref_version_history_enabled`, `last_read` (milliseconds on canvases). OBSERVED: some-du6e/clack/docs/CANVASES.md:7; slack-go/slack/files.go:136-162; stevekinney/openclaw-plugin-slack-workspace/src/canvas-list.test.ts:19-28
- **Response, list** (`filetype: "list"`, `mode: "list"`, `mimetype: "application/vnd.slack-list"`, `pretty_type: "List"`):
  - `list_metadata` = `{schema, subtask_schema, views, integrations, icon, description, description_blocks, is_trial, creation_source{type, reference_id}, todo_mode, default_view}`.
  - `list_limits` = `{row_count, row_count_limit, column_count, column_count_limit, view_count, view_count_limit, archived_row_count, max_attachments_per_cell, over_row_maximum, over_column_maximum, over_view_maximum}`.
  - Also `list_csv_download_url`, `editors`, `last_editor`, `is_ai_suggested`, `is_modified_by_ai`.
    OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.info; slack-go/slack/files.go:164-168; slack-go/slack/lists_types.go:291-348; some-du6e/clack/docs/LISTS.md:9-13
  - Column object: `{id: "Col…", key, name, type, is_primary_column, options}`.
  - View object: `{id: "View…", name, type, position, columns[{id, key, visible, position, width, should_wrap_text}], sorts[{key, column_id, ascending}], filters[{key, column_id, operator, values, typed_values}], grouping{group_by, group_by_column_id}, is_all_items_view, is_locked, show_completed_items, stick_column_left, created_by, date_created, default_view_key}`.
  - View types: `table`, `board`, `timeline`, `list`, `gallery`, `calendar`, `map`, `record`, `canvas_inline`. `record` and `canvas_inline` are never tabs.
    OBSERVED: some-du6e/clack/docs/LISTS.md:21; some-du6e/clack/docs/LISTS.md:41-43; slack-go/slack/lists_types.go:96-104; slack-go/slack/lists_types.go:312-333
- **Response, clip** (audio/video message files): `subtype` (`slack_audio` for audio clips), `media_display_type`, `duration_ms`, `mp4`, `mp4_low`, `hls`, `hls_embed`, `hls_preview`, `vtt`, `thumb_video`, `thumb_video_w`, `thumb_video_h`, `thumb_video_ts`, `audio_wave_samples` (values 0-100), `transcription {status, locale, preview {content, has_more}}`. OBSERVED: slack-go/slack/files.go:89-106; slack-go/slack/files.go:178-188; mautrix/slack/pkg/msgconv/from-slack.go:360; mautrix/slack/pkg/msgconv/from-slack.go:457
- **Response, legacy post** (`filetype: "post"`, labelled "Slack Post"): the files.info envelope carries a top-level `content_html` next to `file`; file fields include `editable`, `non_owner_editable`, `last_editor`, `preview`. OBSERVED: emacs-slack/emacs-slack/slack-file.el:159; wee-slack/wee-slack/typings/slack_api/slack_files_info.pyi:18; wee-slack/wee-slack/typings/slack_api/slack_files_info.pyi:31-34; wee-slack/wee-slack/typings/slack_api/slack_files_info.pyi:50-54
- **Pagination:** comments only, via `count`/`page` (Files area).
- **Used for:**
  - recognising canvases, lists, clips and posts in messages and file lists;
  - canvas and list headers;
  - edit gating (`editable`, `access`);
  - list schema and views (the client loads a list with `files.info` in parallel with `lists.records.list`);
  - polling when realtime is off.
    OBSERVED: mautrix/slack/pkg/msgconv/from-slack.go:272-291; some-du6e/clack/docs/CANVASES.md:70; some-du6e/clack/docs/CANVASES.md:86; some-du6e/clack/docs/LISTS.md:73

### GET <file url_private> (canvas HTML export)

- **Transport:** `GET <file.url_private>` with `Cookie: d=…`. The URL and every redirect hop must stay on file URLs (`files.slack.com`, `/files-pri/…`). OBSERVED: some-du6e/clack/docs/CANVASES.md:41; some-du6e/clack/internal/slack/canvas.go:16-21
- **Visibility:** private; the markup is undocumented.
- **Request params:** none.
- **Response:** `text/html` (or `text/plain`) Quip export starting with `<div class="quip-canvas-content">`. A sign-in redirect also answers with HTML, so check for that marker. OBSERVED: some-du6e/clack/internal/slack/canvas.go:51; some-du6e/clack/internal/slack/canvas.go:65
  Vocabulary of the export, from 200 probed canvases (OBSERVED: some-du6e/clack/docs/CANVASES.md:43-68):

  | export HTML                                                                                                         | block                                        |
  | ------------------------------------------------------------------------------------------------------------------- | -------------------------------------------- |
  | `h1` `h2` `h3`, `id` = section id (`temp:C:…`)                                                                      | headings                                     |
  | `p.line`                                                                                                            | paragraph (empty = blank line)               |
  | `div[data-section-style=5] > ul > li > span`, nested `ul` = next level, wrapper `style="--indent0: N"` = base level | bullets                                      |
  | `data-section-style=6`, `li[value]`                                                                                 | numbered list (value restarts count)         |
  | `data-section-style=7`, `li.checked`                                                                                | checklist                                    |
  | `blockquote`                                                                                                        | quote (contains paragraphs/lists)            |
  | consecutive `p.prettyprint.line`                                                                                    | one code block, one `p` per line             |
  | `hr`                                                                                                                | divider                                      |
  | `table[data-header] > tr > th/td > blocks`                                                                          | table                                        |
  | `div[data-section-style=11] > img[src=/collab-slack-blob/<thread>/<F…>?size_name=…]`                                | image (`div.empty` = placeholder)            |
  | `div.flexbox > div.flexbox-column`                                                                                  | columns                                      |
  | `p.embedded-link`; `p.embedded-file` with `lnk[data-slack-file-id=sf:F…]`                                           | unfurl; embedded file                        |
  | `b` `i` `del` `u` `code`, `span[style=background-color]`, `lnk[href]`                                               | inline styles, links                         |
  | `control > a` with `#C…` / `@U…` / `@S…` / `@here`                                                                  | mentions (ids only, never names)             |
  | `control > img[alt=name]`                                                                                           | emoji                                        |
  | `.default-content` without `.customized-default-content`                                                            | unedited placeholder text; draw it invisible |

- **Pagination:** none. clack caps the body at 16 MiB and never caches it. OBSERVED: some-du6e/clack/docs/CANVASES.md:41
- **Used for:** read-only rendering that keeps section ids. slackdump archives the same file (its downloader skips only `hidden_by_limit`, `external`, `tombstone` modes). It sanitises the HTML with bluemonday's UGC policy and serves it under a `default-src 'none'` CSP. OBSERVED: rusq/slackdump/internal/convert/transform/fileproc/fileproc.go:113-117; rusq/slackdump/internal/convert/html.go:239-259; rusq/slackdump/internal/viewer/canvas.go:9; rusq/slackdump/internal/viewer/canvas.go:21-24

### files.createCanvas

- **Transport:** `POST /api/files.createCanvas`, form.
- **Visibility:** private.
- **Request params:** `title` - ? - ?; `markdown` - ? - ?; `loosen_validation` - ? - ?; `reason` - ? - ?. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:436; some-du6e/clack/docs/CANVASES.md:24
- **Response:** `{"file_id": "F…"}`. OBSERVED: some-du6e/clack/docs/CANVASES.md:24
- **Pagination:** n/a.
- **Used for:** the web client's canvas creation (catalogue entry). The public equivalent is `canvases.create`. OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:658

### files.open

- **Transport:** `POST /api/files.open`, form.
- **Visibility:** private.
- **Request params:** `file_id` - string - sent - canvas file id; `supports_read_only_check` - bool string - sent as `true`; `last_updated` - ms epoch - sent. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:470; some-du6e/clack/internal/core/canvas.go:472-473
- **Response:** `{"viewers": [{"user_id": "U…", "is_focused": …}], "should_subscribe_and_ping": …}`. OBSERVED: some-du6e/clack/docs/CANVASES.md:24; some-du6e/clack/internal/core/canvas.go:468-471
- **Pagination:** n/a.
- **Used for:** opening a canvas view (the `openFile` thunk). The client records the viewers for the header facepile and subscribes to the document's realtime events. OBSERVED: some-du6e/clack/docs/CANVASES.md:26; some-du6e/clack/docs/CANVASES.md:70

### files.close

- **Transport:** `POST /api/files.close`, form.
- **Visibility:** private.
- **Request params:** `file_id` - string - sent; `last_updated` - ? - ? (catalogue). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:430; some-du6e/clack/internal/core/canvas.go:560
- **Response:** not observed; clack ignores it.
- **Pagination:** n/a.
- **Used for:** leaving a canvas view. clack calls it once no view has drawn the canvas for 6 s. OBSERVED: some-du6e/clack/docs/CANVASES.md:70

### files.ping

- **Transport:** `POST /api/files.ping`, form.
- **Visibility:** private.
- **Request params:** `file_id` - ? - ?; `is_focused` - ? - ? (catalogue). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:471
- **Response:** not observed.
- **Pagination:** n/a.
- **Used for:** not observed beyond the name and `files.open`'s `should_subscribe_and_ping` flag (see LEADS). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:701

### files.requestPermission

- **Transport:** `POST /api/files.requestPermission`, form.
- **Visibility:** private.
- **Request params:** `file_id` - ? - ?; `request_type` - string - ? - `upgrade` (edit) or `comment_upgrade` (comment); `reason` - string - ? - e.g. `IndicatorRequestEditAccessButton`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:475; some-du6e/clack/docs/CANVASES.md:200-201
- **Response:** not observed. The UI says "Request sent"; an earlier request is known from `most_recent_access_request_ts`. OBSERVED: some-du6e/clack/docs/CANVASES.md:202
- **Pagination:** n/a.
- **Used for:** "Request Edit Access" / "Request Comment Access" in the View-only pill. OBSERVED: some-du6e/clack/docs/CANVASES.md:199-201

### quip.lookupThreadIds

- **Transport:** `POST /api/quip.lookupThreadIds`, form (slackdump's `Post` form-encodes the request struct). OBSERVED: rusq/slackdump/internal/edge/edge.go:230-231; rusq/slackdump/internal/edge/edge.go:247-251
- **Visibility:** private.
- **Request params:** `file_ids` - comma-separated string of `F…` ids - sent. OBSERVED: rusq/slackdump/internal/edge/canvas.go:96-117; 3kh0/slack-datamine/build/api-args.txt:623
- **Response:** `{"ok": true, "lookup": {"F06R4HA3ZS8": "<quip thread id>"}}`; a missing entry means no document. OBSERVED: rusq/slackdump/internal/edge/canvas.go:102-105; rusq/slackdump/internal/edge/canvas.go:477-484
- **Pagination:** n/a (batch by ids).
- **Used for:** mapping a canvas file to the Quip thread ("OYP") id that `/canvas/-/load-data` needs. The same id appears as `quip_thread_id` on files and on `properties.canvas`. OBSERVED: rusq/slackdump/internal/edge/canvas.go:107-109; slack-go/slack/conversation.go:101-105

### quip.lookupFileId

- **Transport:** `POST /api/quip.lookupFileId`, form.
- **Visibility:** private.
- **Request params:** `quip_thread_id` - ? - ? (catalogue). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:622
- **Response:** not observed.
- **Pagination:** n/a.
- **Used for:** listed among the canvas methods the client uses; it is the reverse of `quip.lookupThreadIds`. OBSERVED: some-du6e/clack/docs/CANVASES.md:24

### POST /canvas/collab/controller-init

- **Transport:** `POST https://<ws>.slack.com/canvas/collab/controller-init?format=map`, multipart form. OBSERVED: rusq/slackdump/internal/edge/canvas.go:143-160; rusq/slackdump/internal/edge/edge.go:311; rusq/slackdump/internal/edge/edge.go:342-343; .omo/research/gist.md:196
- **Visibility:** private.
- **Request params:** `token` - xoxc - the only form field slackdump sends; query `format=map`. OBSERVED: rusq/slackdump/internal/edge/canvas.go:144-151
- **Response:** JSON. slackdump reads only `user_id`, which it passes as `_user_id` to load-data. The live capture also saw:
  - `init_options`: a serialized protobuf with document state and user context;
  - a large `js_initializer` (Quip config);
  - Quip hosts: realtime `listenweb3.slack-prod.onquip.com` (`/-/listen/3`), API `api.slack-prod.onquip.com`, CDN `a.slack-edge.com/canvas_blob/`;
  - Quip team `T5J4Q04QG`;
  - a separate canvas OAuth token with a 12 h TTL;
  - build `quipbuild_2026_02_26T21_00_d87d87e`.
    OBSERVED: rusq/slackdump/internal/edge/canvas.go:139-141; .omo/research/gist.md:252-263
- **Pagination:** n/a.
- **Used for:** bootstrapping the Quip editor; slackdump calls it before load-data. The catalogue also has `quip.getControllerInitData` (args not mined). OBSERVED: rusq/slackdump/internal/edge/canvas.go:468; 3kh0/slack-datamine/build/api-methods.txt:916

### POST /canvas/-/load-data

- **Transport:** `POST https://<ws>.slack.com/canvas/-/load-data?_x_version_ts=…`, form; slackdump uses the `/canvas/-/load-data/editor/1?_x_version_ts=<unix ms>` variant. The web client sends the token as `Authorization: Bearer <token>` on the same origin, or as a `token=` form field cross-origin; slackdump sends the `token` form field to the team host. OBSERVED: some-du6e/clack/docs/CANVASES.md:17; some-du6e/clack/docs/CANVASES.md:22; rusq/slackdump/internal/edge/canvas.go:357-368; rusq/slackdump/internal/edge/edge.go:247-251
- **Visibility:** private; Quip protobuf ("pblite").
- **Request params** (editor/1 as slackdump sends it):
  - `token` = the xoxc token (`PostFormRaw` adds it to any form that lacks one);
  - `_resource_bundle` = `collab_controller`;
  - `_user_id` = controller-init `user_id`;
  - `_version` = `10`;
  - `_window_session_id` = 8 random bytes, base64url;
  - `request_binary` = base64 protobuf `{1: <thread id>, 3: {1: 1, 2: <thread id>}, 5: "editor", 6: 1}`.
    OBSERVED: rusq/slackdump/internal/edge/canvas.go:177-203; rusq/slackdump/internal/edge/canvas.go:362-368; rusq/slackdump/internal/edge/edge.go:247-251
- **Response:** binary `ResponseWrapper`. slackdump decodes only the comment-thread table at protobuf path `2 → 2 → 6 → 16`; each entry (field `1`) is `{1: thread id (string), 2: reply count (varint), 4: latest message time (µs varint)}`. OBSERVED: some-du6e/clack/docs/CANVASES.md:17; rusq/slackdump/internal/edge/canvas.go:327-355
- **Pagination:** none observed.
- **Used for:** the web editor's document load (thread and sections). slackdump uses it to enumerate a canvas's comment threads. OBSERVED: some-du6e/clack/docs/CANVASES.md:17; rusq/slackdump/internal/edge/canvas.go:468

### POST /canvas/-/fetch-latest

- **Transport:** `POST https://<ws>.slack.com/canvas/-/fetch-latest` on the team host; the request is a Quip protobuf message. How the body is encoded was not observed (see LEADS). OBSERVED: some-du6e/clack/docs/CANVASES.md:18; some-du6e/clack/docs/CANVASES.md:22
- **Visibility:** private.
- **Request params:** `Request{ThreadRequest{thread_id, client_thread_sequence, client_max_section_sequence}}`. OBSERVED: some-du6e/clack/docs/CANVASES.md:18
- **Response:** `ChangesData`, the changes since that sequence. OBSERVED: some-du6e/clack/docs/CANVASES.md:18
- **Pagination:** by sequence numbers.
- **Used for:** catching up after a `document_update` event (`fetchThreadChanges`). OBSERVED: some-du6e/clack/docs/CANVASES.md:28

### POST /canvas/-/edit-document

- **Transport:** `POST https://<ws>.slack.com/canvas/-/edit-document`, binary edit transactions. The sibling `/canvas/-/save-diagnostics-report` sends diagnostics. OBSERVED: some-du6e/clack/docs/CANVASES.md:19-20
- **Visibility:** private.
- **Request params:** Quip-internal protobuf; schema not documented. OBSERVED: some-du6e/clack/docs/CANVASES.md:22
- **Response:** not observed.
- **Pagination:** n/a.
- **Used for:** every editor write, and the only write path for Quip-only constructs: comment highlights (`CONTROL_HIGHLIGHT_TYPE`), section reactions, cover images. OBSERVED: some-du6e/clack/docs/CANVASES.md:117; some-du6e/clack/docs/CANVASES.md:190

### quip.history.getVersions

- **Transport:** `POST /api/quip.history.getVersions`, form.
- **Visibility:** private.
- **Request params:** `file_id`, `limit`, `newest_created_usec`, `oldest_created_usec` - ? - ? (catalogue). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:620
- **Response:** not observed.
- **Pagination:** by the `*_created_usec` window plus `limit`.
- **Used for:** canvas version history (team pref `canvas_version_history_enabled`). OBSERVED: rusq/slackdump/internal/edge/assets/client.userBoot.json:151

### quip.history.restoreVersion

- **Transport:** `POST /api/quip.history.restoreVersion`, form.
- **Visibility:** private.
- **Request params:** `file_id`, `sequence`, `version_id` - ? - ? (catalogue). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:621
- **Response:** not observed.
- **Pagination:** n/a.
- **Used for:** restoring a canvas version (catalogue name only).

### quip.getCanvasBulkSectionIdsData

- **Transport:** `POST /api/quip.getCanvasBulkSectionIdsData`, form.
- **Visibility:** private.
- **Request params:** `file_id`, `section_ids` - ? - ? (catalogue). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:618
- **Response:** not observed.
- **Pagination:** n/a.
- **Used for:** labels of canvas-section links in messages: "{canvasName}: {sectionText}", 25 characters. OBSERVED: some-du6e/clack/docs/CANVASES.md:192

### canvases.listHeaders

- **Transport:** `POST /api/canvases.listHeaders`, form.
- **Visibility:** private (undocumented).
- **Request params:** `header_id` - ? - ? (catalogue). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:242
- **Response:** not observed.
- **Pagination:** n/a.
- **Used for:** "Add cover", which picks a random header from this list. The cover itself is saved through `/canvas/-/edit-document`. OBSERVED: some-du6e/clack/docs/CANVASES.md:190

### Catalogue: other private canvas methods

Names and argument names only; none of the catalogue rows has an observed response. The two `agents.conversations.*` rows are different: they are public, documented methods that slack-go wraps, their docs require a granular bot token (scope `code_channels:manage`), and they are not in the web client's catalogue.

| method                                                                                                | args (catalogue)                                                                                                                  | OBSERVED                                                                                                                                                               |
| ----------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `canvases.getTemplates`                                                                               | `section_type`                                                                                                                    | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:241                                                                                                                   |
| `canvases.getCannedTemplates`                                                                         | `is_user_locale`, `template_names`                                                                                                | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:240                                                                                                                   |
| `canvases.notificationSections`                                                                       | `file_id`, `timestamp`                                                                                                            | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:243                                                                                                                   |
| `canvases.drafts.get`                                                                                 | `canvas_id`, `draft_id`                                                                                                           | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:239                                                                                                                   |
| `canvases.agents.list`                                                                                | not mined                                                                                                                         | OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:364                                                                                                                |
| `canvases.agents.markRead`                                                                            | `canvas_id`                                                                                                                       | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:238                                                                                                                   |
| `quip.getControllerInitData`                                                                          | not mined                                                                                                                         | OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:916                                                                                                                |
| `quip.cloneCanvas`                                                                                    | `file_id`, `keep_original_title`, `target_file_created`, `target_file_id`, `title`                                                | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:617                                                                                                                   |
| `quip.getMentions`                                                                                    | `channel_ids`                                                                                                                     | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:619                                                                                                                   |
| `quip.thread.mark`                                                                                    | `channel_id`, `mark_unread`, `thread_ts`, `ts`                                                                                    | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:624                                                                                                                   |
| `quip.thread.synchronize`                                                                             | `channel_id`, `thread_ts`                                                                                                         | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:625                                                                                                                   |
| `quip.unfurl`                                                                                         | `acknowledged_private_share`, `file_id`, `url`                                                                                    | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:626                                                                                                                   |
| `quip.updateRootComment`                                                                              | `content_snippet`, `file_id`, `is_source_deleted`, `thread_id`                                                                    | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:627                                                                                                                   |
| `files.docs.setNonOwnerEditability`                                                                   | `file`, `non_owner_editability`                                                                                                   | OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:667; 3kh0/slack-datamine/build/api-args.txt:443                                                                    |
| `files.createSkillCanvas`                                                                             | `markdown`, `title`                                                                                                               | OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:660; 3kh0/slack-datamine/build/api-args.txt:438                                                                    |
| `admin.canvases.publishTemplate`                                                                      | `file_id`, `publish_sales_template`                                                                                               | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:29                                                                                                                    |
| `admin.canvases.unpublishTemplate`                                                                    | `file_id`, `unset_org_sharing_setting`                                                                                            | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:30                                                                                                                    |
| `agents.conversations.getCanvas` (Slack Code channels; returns markdown plus comment threads)         | `channel`, `canvas_id` (required); `content_format`, `include_resolved` (optional); from slack-go and the docs, not the catalogue | OBSERVED: slack-go/slack/agents_canvas.go:86-98; slack-go/slack/CHANGELOG.md:495-499; https://docs.slack.dev/reference/methods/agents.conversations.getCanvas          |
| `agents.conversations.setCanvasContent` (keeps section ids and comment anchors on unchanged sections) | `channel`, `canvas_id`, `content` (all required); from slack-go and the docs, not the catalogue                                   | OBSERVED: slack-go/slack/agents_canvas.go:126-133; slack-go/slack/CHANGELOG.md:495-499; https://docs.slack.dev/reference/methods/agents.conversations.setCanvasContent |

---

**Canvas comments (the file channel)**

### conversations.history (canvas comment stream)

- **Transport:** `POST /api/conversations.history`. The method belongs to the Messages reference; this subsection covers only the canvas-specific use. OBSERVED: some-du6e/clack/docs/CANVASES.md:123
- **Visibility:** public method, private parameters.
- **Request params:**
  - `channel` - string - the canvas's file channel, `"C" + file_id[1:]`;
  - `document_stream` - string - `latest_reply`, `unread` or `archived`;
  - `limit` - 20;
  - also `include_pin_count`, `inclusive`, `no_user_profile`, `include_free_team_extra_messages`, `include_date_joined=false`, `cursor`.
    OBSERVED: some-du6e/clack/docs/CANVASES.md:115; some-du6e/clack/docs/CANVASES.md:123; rusq/slackdump/internal/structures/canvases.go:34-39
- **Response:** roots are messages with `subtype: "document_comment_root"` and `document_comment {thread_id, authors, is_archived, is_visible, last_seen (µs), most_recent_editor}`; the other messages are those threads' latest replies. `channel_not_found` means the canvas has no threads. OBSERVED: some-du6e/clack/docs/CANVASES.md:115; some-du6e/clack/docs/CANVASES.md:123; rusq/slackdump/internal/edge/canvas.go:49-52; rusq/slackdump/internal/structures/structures.go:34
- **Pagination:** `cursor`.
- **Used for:** the comment stream: "All threads" / "Unread comments" / "Resolved threads".

### conversations.replies (canvas and list comment threads)

- **Transport:** `POST /api/conversations.replies` (Messages reference), form. OBSERVED: rusq/slackdump/internal/edge/canvas.go:380-396
- **Visibility:** public method, private `ts` forms.
- **Request params:** `channel` - the file channel; `ts` - root ts; `limit` - slackdump sends 1000; `cursor`; `inclusive`. Two special `ts` forms:
  - `<channel>-Qpc:<thread id with "temp:" shortened to "t:">` for a canvas comment thread whose root isn't known yet;
  - `<list id>-<record id>` for a list item that has no `thread_ts`. This returns the root message, subtype `list_record_comment`.
    OBSERVED: rusq/slackdump/internal/edge/canvas.go:380-396; some-du6e/clack/docs/CANVASES.md:115; some-du6e/clack/docs/CANVASES.md:124; some-du6e/clack/docs/LISTS.md:69
- **Response:** `messages[]` (comment roots carry `document_comment`), `has_more`, `response_metadata.next_cursor`. OBSERVED: rusq/slackdump/internal/edge/canvas.go:90-94; rusq/slackdump/internal/edge/canvas.go:398-417
- **Pagination:** `cursor` ← `response_metadata.next_cursor`. OBSERVED: rusq/slackdump/internal/edge/canvas.go:412-416
- **Used for:** canvas comment threads and list-item comment threads. Replying, editing, deleting and reacting use ordinary `chat.postMessage` (with `thread_ts` = the root), `chat.update`, `chat.delete` and `reactions.*` (Messages area). OBSERVED: some-du6e/clack/docs/CANVASES.md:125

### quip.thread.archive / quip.thread.unarchive

- **Transport:** `POST /api/quip.thread.archive` and `/api/quip.thread.unarchive`, form.
- **Visibility:** private.
- **Request params:** `channel_id` - the file channel; `thread_ts` - comment root. OBSERVED: some-du6e/clack/docs/CANVASES.md:126
- **Response:** includes `message`, which replaces the root. OBSERVED: some-du6e/clack/docs/CANVASES.md:126
- **Pagination:** n/a.
- **Used for:** "Resolve thread" / "Re-open thread". OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:922; 3kh0/slack-datamine/build/api-methods.txt:926

### quip.thread.archiveAll

- **Transport:** `POST /api/quip.thread.archiveAll`, form.
- **Visibility:** private.
- **Request params:** `channel_id` - the file channel. OBSERVED: some-du6e/clack/docs/CANVASES.md:127
- **Response:** not observed.
- **Pagination:** n/a.
- **Used for:** "Resolve all threads", which asks for confirmation first. OBSERVED: some-du6e/clack/docs/CANVASES.md:131; 3kh0/slack-datamine/build/api-methods.txt:923

---

**Channel tabs**

### conversations.info (properties.tabs / properties.canvas)

- **Transport:** `POST /api/conversations.info` (Channels reference). Only the tab-related response fields are listed here.
- **Visibility:** public method; the tab fields are undocumented.
- **Request params:** see the Channels reference.
- **Response:**
  ```json
  "properties": {
    "canvas": {"file_id": "F…", "is_empty": …, "quip_thread_id": "…", "is_migrated": …, "is_open_by_default": …},
    "tabs": [
      {"id": "Ct0CD2AQBDBR", "type": "canvas", "label": "Runbook", "data": {"file_id": "F0CD2AQ7XBR"}},
      {"id": "files", "type": "files", "label": ""}
    ]
  }
  ```
  - Tab types seen: `canvas`, `list`, `files`, and the legacy `channel_canvas`, which is still listed with `is_disabled: true`.
  - `data` may also carry `shared_ts` and `mute_edit_updates`.
  - DMs carry canvas tabs the same way.
    OBSERVED: slack-go/slack/conversation.go:73-105; slack-go/slack/conversation_test.go:830-832; some-du6e/clack/docs/CANVASES.md:9; some-du6e/clack/docs/LISTS.md:17; rusq/slackdump/internal/structures/canvases.go:22-30; rusq/slackdump/internal/fixtures/channels.go:346
- **Pagination:** n/a.
- **Used for:** drawing a channel's tab bar and finding its channel canvas.

### conversations.addTab

- **Transport:** `POST /api/conversations.addTab`, form.
- **Visibility:** private.
- **Request params:**
  - `channel_id` - string - sent;
  - `type` - string - sent - e.g. `list`;
  - `data` - JSON - for lists `{file_id, lists_channel_tab_default_refine_view_id?, access_level?}`;
  - `label` - string - optional;
  - `after` - ? - ?;
  - `target_team_id` - ? - ?;
  - `reason` - string - `add-channel-tab`.
    OBSERVED: 3kh0/slack-datamine/build/api-args.txt:299; some-du6e/clack/docs/LISTS.md:117; some-du6e/clack/internal/core/channel_tabs_edit.go:80
- **Response:** `{"tabz": [...]}` (spelled that way). Error `too_many_tabs` shows "Maximum number of tabs added."; any other error shows "Cannot add tab.". OBSERVED: some-du6e/clack/docs/LISTS.md:117
- **Pagination:** n/a.
- **Used for:** adding a List (or other) tab to a channel (`addTabToChannel`).

### conversations.updateTab

- **Transport:** `POST /api/conversations.updateTab`, form.
- **Visibility:** private.
- **Request params:** `channel_id`, `id`, `data`, `is_pinned`, `label` - ? - ? (catalogue); clack sends `id`, `channel_id`, `label` and `reason` (`updateTabLabel` or `folder_tab_view_header`). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:359; some-du6e/clack/internal/core/bookmark_folders.go:444-451
- **Response:** not observed.
- **Pagination:** n/a.
- **Used for:** renaming or pinning a tab.

### conversations.removeTab

- **Transport:** `POST /api/conversations.removeTab`, form.
- **Visibility:** private.
- **Request params:** `channel_id` - sent; `id` - sent - tab id; `keep_file_share` - bool string - optional - clack sends `true` to keep the file shared; `reason` - optional - clack's tab-bar removal sends `remove-channel-tab`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:344; some-du6e/clack/internal/core/browse.go:1105; some-du6e/clack/internal/core/channel_tabs_edit.go:265-270
- **Response:** not observed.
- **Pagination:** n/a.
- **Used for:** removing a tab.

### conversations.moveTab

- **Transport:** `POST /api/conversations.moveTab`, form.
- **Visibility:** private.
- **Request params:** not mined (see LEADS). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:508
- **Response:** not observed.
- **Pagination:** n/a.
- **Used for:** reordering tabs (catalogue name only).

---

**Lists: web-client (private) methods.** The public `slackLists.*` methods are not what the client uses, and none of them is in its catalogue. OBSERVED: some-du6e/clack/docs/LISTS.md:120; 3kh0/slack-datamine/build/api-methods.txt:809-854

### lists.records.list

- **Transport:** `POST /api/lists.records.list`, form.
- **Visibility:** private.
- **Request params:**
  - `list_id` - string - sent;
  - `archived` - bool - ?;
  - `include_subtasks` - bool - ?;
  - `include_suggested` - bool - ?;
  - `exclude_deprecated_values` - bool - ?;
  - `cursor` - string - ?;
  - clack also sends `limit=1000`, which is not in the catalogue.
    OBSERVED: 3kh0/slack-datamine/build/api-args.txt:572; some-du6e/clack/docs/LISTS.md:47; some-du6e/clack/internal/core/lists.go:176-180
- **Response:**
  ```json
  {"ok": true,
   "records": [{"id": "Rec…", "list_id": "F…", "date_created": …, "created_by": "U…", "updated_by": "U…",
                "updated_timestamp": "…", "position": "…", "parent_record_id": "Rec…", "thread_ts": "…",
                "view_positions": {"View…": "…"}, "platform_refs": …, "archived": …, "is_subscribed": …, "saved": …,
                "fields": [{"key": "…", "column_id": "Col…", "value": null, "<typed>": […]}]}],
   "response_metadata": {"next_cursor": "…"},
   "limits": {…}}
  ```
  Typed arrays per column type:
  - text → `text` + `rich_text`;
  - select → `select: ["Opt…"]`;
  - user, vote, created_by, last_edited_by, todo_assignee → `user: ["U…"]`;
  - channel → `channel`;
  - date, todo_due_date → `date: ["YYYY-MM-DD"]`, `timestamp`;
  - created_time, last_edited_time → `timestamp`;
  - number → `number`;
  - rating → `rating`;
  - checkbox, todo_completed → `checkbox: bool`;
  - email, phone → arrays;
  - attachment, canvas → `attachment: ["F…"]`;
  - link → `link: [{originalUrl, displayAsUrl, displayName?, attachment: {url…}}]` (camelCase);
  - message → `message: [{value: <permalink>, channel_id, ts, thread_ts?}]`.
    OBSERVED: some-du6e/clack/docs/LISTS.md:47-63
- **Pagination:** pass `cursor` ← `response_metadata.next_cursor` until it comes back empty. OBSERVED: some-du6e/clack/internal/core/lists.go:178-196
- **Used for:** loading a list (`fetchRecords`), in parallel with `files.info`. OBSERVED: some-du6e/clack/docs/LISTS.md:73

### lists.records.info / lists.records.infoBatch

- **Transport:** `POST /api/lists.records.info` and `/api/lists.records.infoBatch`, form.
- **Visibility:** private.
- **Request params:** info takes `list_id`, `id`, `include_is_subscribed`, `exclude_deprecated_values`; infoBatch takes `list_id`, `ids`, `include_is_subscribed`, `exclude_deprecated_values`. Types and required-ness `?` (catalogue). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:570-571
- **Response:** not observed.
- **Pagination:** n/a.
- **Used for:** refetching single items. OBSERVED: some-du6e/clack/docs/LISTS.md:73

### lists.open

- **Transport:** `POST /api/lists.open`, form.
- **Visibility:** private.
- **Request params:** `list_id` - string - sent. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:564; some-du6e/clack/internal/core/lists.go:306
- **Response:** `{"pinned_view_ids": ["View…"]}`. OBSERVED: some-du6e/clack/docs/LISTS.md:74; some-du6e/clack/internal/core/lists.go:303-305
- **Pagination:** n/a.
- **Used for:** opening a list (`fetchListsOpen`); the result gives the pinned view tabs.

### lists.records.create

- **Transport:** `POST /api/lists.records.create`, form.
- **Visibility:** private.
- **Request params:**
  - `list_id` - sent;
  - `initial_fields` - JSON `[{column_id, <typed>}]`;
  - `position` - fractional string;
  - `updated_timestamp` - hybrid-logical-clock stamp `<ms>-<counter, 4 hex>-<node, 16>`;
  - `parent_record_id` - optional;
  - `duplicated_record_id` - optional;
  - `fields` - ? (catalogue).
    OBSERVED: 3kh0/slack-datamine/build/api-args.txt:567; some-du6e/clack/docs/LISTS.md:112; some-du6e/clack/internal/core/lists.go:626-637
- **Response:** `{"record": {…}}`. OBSERVED: some-du6e/clack/internal/core/lists.go:634-637
- **Pagination:** n/a.
- **Used for:** "Add item", subtasks, "Duplicate item". OBSERVED: some-du6e/clack/docs/EXPERIMENTS.md:167

### lists.cells.update

- **Transport:** `POST /api/lists.cells.update`, form.
- **Visibility:** private.
- **Request params:** `list_id` - sent; `cells` - JSON `[{row_id, column_id, <typed>}]` - sent. Typed values:
  - `rich_text: [blocks]`; plain text becomes one `rich_text_section`, or a `link` element when the whole text is a URL;
  - `number: [n]`;
  - `select: ["Opt…"]`;
  - `date: ["YYYY-MM-DD"]`;
  - `user: ["U…"]` (people and votes);
  - `channel: ["C…"]`;
  - `attachment: ["F…"]` (canvas and file columns);
  - `checkbox: bool`;
  - `email: [s]`, `phone: [s]`;
  - `rating: [n]`;
  - `link: [{original_url, display_as_url, display_name?}]`.
    OBSERVED: 3kh0/slack-datamine/build/api-args.txt:552; some-du6e/clack/docs/LISTS.md:111; some-du6e/clack/internal/core/lists.go:573
- **Response:** `{"cells": […]}`. OBSERVED: some-du6e/clack/docs/LISTS.md:111
- **Pagination:** n/a.
- **Used for:** every cell edit in the grid, board and item pane.

### lists.records.delete

- **Transport:** `POST /api/lists.records.delete`, form.
- **Visibility:** private.
- **Request params:** `list_id`, `id` - sent. Siblings: `lists.records.deleteMultiple {ids, list_id}` and `lists.records.updateDeletion {deleted, id, list_id}`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:568-569; 3kh0/slack-datamine/build/api-args.txt:577; some-du6e/clack/internal/core/lists.go:656
- **Response:** not observed.
- **Pagination:** n/a.
- **Used for:** "Delete item".

### lists.records.archive

- **Transport:** `POST /api/lists.records.archive`, form.
- **Visibility:** private.
- **Request params:** `list_id` - sent; `ids` - JSON array - sent; `restore` - bool - ? (catalogue). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:565; some-du6e/clack/internal/core/lists_records.go:196
- **Response:** not observed.
- **Pagination:** n/a.
- **Used for:** "Archive item" and restore. OBSERVED: some-du6e/clack/docs/EXPERIMENTS.md:167

### lists.create

- **Transport:** `POST /api/lists.create`, form.
- **Visibility:** private.
- **Request params:** `name`, `description`?, `description_blocks`?, `schema`?, `icon`?, `template_id`?, `todo_mode`?, `context_channel_id`?, `include_canvas_inline_view`, `include_copied_list_records`, `is_template`. Types `?`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:557; some-du6e/clack/docs/LISTS.md:114; some-du6e/clack/internal/core/lists.go:838
- **Response:** `{"list": {<file object>}}`. OBSERVED: some-du6e/clack/docs/LISTS.md:114
- **Pagination:** n/a.
- **Used for:** "New list", from scratch or from a template.

### lists.templates

- **Transport:** `POST /api/lists.templates`, form.
- **Visibility:** private.
- **Request params:** `template_ids` - ? - ? (catalogue); clack sends only `reason=get-templates`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:580; some-du6e/clack/internal/core/lists.go:791
- **Response:** `{"templates": […], "starter_templates": […]}`; some configurations return `template_files` / `starter_template_files` instead. OBSERVED: some-du6e/clack/docs/LISTS.md:114
- **Pagination:** n/a.
- **Used for:** the template picker.

### lists.addToList

- **Transport:** `POST /api/lists.addToList`, form.
- **Visibility:** private.
- **Request params:** `list_id`, `channel_id`, `message_ts` - sent; `reason` - `add-to-list`; `slackbot_help`, `track_thread` - ? (catalogue). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:551; some-du6e/clack/docs/LISTS.md:116; some-du6e/clack/internal/core/lists_add.go:41
- **Response:** `{"record": {…}, "list": {…}}`. Errors: `list_not_found`, `over_row_maximum`, `permission_denied`, `message_not_found`, `missing_primary_column`, `channel_not_found`. OBSERVED: some-du6e/clack/docs/LISTS.md:116
- **Pagination:** n/a.
- **Used for:** the "Add to list" message action.

### lists.records.open

- **Transport:** `POST /api/lists.records.open`, form.
- **Visibility:** private.
- **Request params:** `list_id`, `id` - sent; `reason` - `open-record`. OBSERVED: 3kh0/slack-datamine/build/api-args.txt:573; some-du6e/clack/internal/core/lists_add.go:61
- **Response:** not used by clack.
- **Pagination:** n/a.
- **Used for:** opening an item (`openRecord`), e.g. after adding a message to a list. OBSERVED: some-du6e/clack/docs/LISTS.md:116

### lists.getThreadsForRecord

- **Transport:** `POST /api/lists.getThreadsForRecord`, form.
- **Visibility:** private.
- **Request params:** `list_id`, `record_id`. OBSERVED: some-du6e/clack/docs/LISTS.md:69; 3kh0/slack-datamine/build/api-methods.txt:822
- **Response:** not observed.
- **Pagination:** n/a.
- **Used for:** listing other threads that mention the item.

### Catalogue: other private lists methods

Names and argument names only; none of these has an observed response.

| method                                                                                       | args (catalogue)                                                               | OBSERVED                                                                                                                                                                                             |
| -------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `lists.columns.create`                                                                       | `list_id`, `name`, `type`, `view_id`                                           | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:553                                                                                                                                                 |
| `lists.columns.update`                                                                       | `backfill_missing_cells_with_default`, `key`, `list_id`                        | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:556                                                                                                                                                 |
| `lists.columns.delete`                                                                       | `key`, `list_id`                                                               | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:554                                                                                                                                                 |
| `lists.columns.revert`                                                                       | `id`, `list_id`                                                                | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:555                                                                                                                                                 |
| `lists.views.create`                                                                         | `list_id`, `name`, `type`                                                      | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:582                                                                                                                                                 |
| `lists.views.update`                                                                         | `id`, `list_id`, `skip_simple_array_serialization_in_get_form_data`            | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:585                                                                                                                                                 |
| `lists.views.delete`                                                                         | `id`, `list_id`                                                                | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:583                                                                                                                                                 |
| `lists.views.pins.list`                                                                      | `list_id`                                                                      | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:584                                                                                                                                                 |
| `lists.views.pins.add`, `lists.views.pins.remove`                                            | not mined                                                                      | OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:851; 3kh0/slack-datamine/build/api-methods.txt:853                                                                                               |
| `lists.update`                                                                               | `id`, `template_type`                                                          | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:581                                                                                                                                                 |
| `lists.setDefaultView`                                                                       | `list_id`, `view_id`                                                           | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:578                                                                                                                                                 |
| `lists.getDefaultView`                                                                       | `list_id`                                                                      | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:560                                                                                                                                                 |
| `lists.duplicate`                                                                            | `copy_records`, `copy_views`, `list_id`, `target_list_id`                      | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:558                                                                                                                                                 |
| `lists.history.load`                                                                         | `list_id`, `timestamp`                                                         | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:561                                                                                                                                                 |
| `lists.history.restorePreview`                                                               | `list_id`, `restore_timestamp_micros`                                          | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:563                                                                                                                                                 |
| `lists.history.restoreExecute`                                                               | `list_id`, `preview_token`, `restore_timestamp_micros`                         | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:562                                                                                                                                                 |
| `lists.history.list`                                                                         | not mined                                                                      | OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:823                                                                                                                                              |
| `lists.edits.getLastForCell`                                                                 | `column_id`, `list_id`, `row_id`                                               | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:559                                                                                                                                                 |
| `lists.edits.list`                                                                           | not mined                                                                      | OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:818                                                                                                                                              |
| `lists.records.count`                                                                        | `list_id`                                                                      | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:566                                                                                                                                                 |
| `lists.records.subscriptions.add` / `.get` / `.remove`                                       | `id`, `list_id`                                                                | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:574-576                                                                                                                                             |
| `lists.records.reorder`, `lists.records.restore`                                             | not mined                                                                      | OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:838-839                                                                                                                                          |
| `lists.setupTodos`                                                                           | `assignee_access_level`, `assignee_column_id`, `due_date_column_id`, `list_id` | OBSERVED: 3kh0/slack-datamine/build/api-args.txt:579                                                                                                                                                 |
| `lists.getMyItems`, `lists.import`, `lists.updateApproval`, `lists.getApprovalReviewPending` | not mined                                                                      | OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:819; 3kh0/slack-datamine/build/api-methods.txt:821; 3kh0/slack-datamine/build/api-methods.txt:827; 3kh0/slack-datamine/build/api-methods.txt:848 |

---

**Lists: public `slackLists.*` methods** (Lists API launch on 2025-09-02; paid plans only). OBSERVED: https://docs.slack.dev/changelog/2025/09/02/list-api; https://docs.slack.dev/reference/methods/slackLists.items.list

### slackLists.items.list

- **Transport:** `POST /api/slackLists.items.list`, form or JSON.
- **Visibility:** public (`lists:read`, tier 2). OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.list
- **Request params:**
  - `list_id` - string - required;
  - `limit` - int - optional - default and maximum 5000;
  - `cursor` - string - optional;
  - `archived` - bool - optional - returns archived items instead of normal ones;
  - `include_list` - bool - optional - also returns the parent `list` (title, schema, row count).
    OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.list; slack-go/slack/lists.go:770-791
- **Response:**
  ```json
  {"ok": true,
   "items": [{"id": "Rec…", "list_id": "F…", "date_created": 1758744346,
              "fields": [{"key": "…", "value": …, "column_id": "Col…", "text": "…", "rich_text": […],
                          "select": ["Opt…"], "user": ["U…"], "date": ["2025-10-10"], "checkbox": [true],
                          "link": [{"originalUrl": "…", "displayAsUrl": false, "displayName": "…"}],
                          "message": [{"value": "<permalink>", "channel_id": "C…", "ts": "…", "thread_ts": "…"}],
                          "reference": [{"canvas_section": {"file_id": "F…", "section_id": "…"}}]}],
              "created_by": "U…", "updated_timestamp": "…", "updated_by": "U…", "parent_record_id": "Rec…",
              "archived": false, "is_subscribed": false,
              "saved": {"is_archived": false, "date_due": …, "state": "in_progress"}, "saved_fields": {"Col…": {…}}}],
   "list": {…}}
  ```
  - `key`/`value` are slated for deprecation in favour of `column_id` and the typed arrays.
  - `saved.state` is one of `archived`, `in_progress`, `completed`.
  - `reference` can also be `message`, `list_record {list_id, row_id}` or `file {file_id}`.
    OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.list; slack-go/slack/lists.go:45-48; slack-go/slack/lists_types.go:172-250
- **Pagination:** `cursor` / `limit`. OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.list
- **Used for:** the documented read path, as a fallback for `lists.records.list`.

### slackLists.items.info

- **Transport:** `POST /api/slackLists.items.info`, form or JSON.
- **Visibility:** public (`lists:read`, tier 2). OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.info
- **Request params:** `list_id` - string - required; `id` - string - required - row id (`Rec…`); `include_is_subscribed` - bool - optional, default false. OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.info; slack-go/slack/lists.go:696-706
- **Response:** `{"ok": true, "list": {<full list file object incl. list_metadata, list_limits, url_private, permalink>}, "record": {"id": "Rec…", "list_id": "F…", "date_created": …, "created_by": "…", "updated_by": "…", "fields": [], "updated_timestamp": "…"}, "subtasks": []}`. OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.info; slack-go/slack/lists.go:37-41
- **Pagination:** n/a.
- **Used for:** one item together with its list schema.

### slackLists.items.create

- **Transport:** `POST /api/slackLists.items.create`, form or JSON.
- **Visibility:** public (`lists:write`, tier 3). OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.create
- **Request params:**
  - `list_id` - required;
  - `duplicated_item_id` - optional;
  - `parent_item_id` - optional - makes a subtask;
  - `initial_fields` - JSON array - optional - `[{column_id, <one of rich_text, user, date, select, checkbox (bare bool), number, email, phone, attachment, link {original_url, display_as_url, display_name}, message (permalink strings), rating, timestamp, channel, reference>}]`. Plain text must be sent as `rich_text`.
    OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.create; slack-go/slack/lists.go:580-601; slack-go/slack/lists_types.go:137-161
- **Response:** `{"ok": true, "item": {"id": "Rec018ALE9718", "list_id": "F1234567", "date_created": 1758744345, "created_by": "W0AB1CDE2", "updated_by": "W0AB1CDE2", "fields": [], "updated_timestamp": "1758744345"}}`. OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.create; slack-go/slack/lists.go:31-33
- **Pagination:** n/a.
- **Used for:** the documented item-create path.

### slackLists.items.update

- **Transport:** `POST /api/slackLists.items.update`, form or JSON.
- **Visibility:** public (`lists:write`, tier 3). OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.update
- **Request params:** `list_id` - string - required; `cells` - JSON array (1-100) - required - `[{row_id, column_id, <typed>}]`. slack-go also models `row_id_to_create`. OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.update; slack-go/slack/lists.go:810-821; slack-go/slack/lists_types.go:164-170
- **Response:** `{"ok": true}`; errors such as `invalid_row_id`. OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.update
- **Pagination:** n/a.
- **Used for:** the documented cell-edit path.

### slackLists.items.delete

- **Transport:** `POST /api/slackLists.items.delete`, form or JSON.
- **Visibility:** public (`lists:write`, tier 2). OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.delete
- **Request params:** `list_id` - string - required; `id` - string - required. OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.delete; slack-go/slack/lists.go:620-626
- **Response:** `{"ok": true}`.
- **Pagination:** n/a.
- **Used for:** deleting one item.

### slackLists.items.deleteMultiple

- **Transport:** `POST /api/slackLists.items.deleteMultiple`, form or JSON.
- **Visibility:** public (`lists:write`, tier 2). OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.deleteMultiple
- **Request params:** `list_id` - string - required; `ids` - JSON array - required. OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.deleteMultiple; slack-go/slack/lists.go:645-656
- **Response:** `{"ok": true}`.
- **Pagination:** n/a.
- **Used for:** bulk delete.

### slackLists.create

- **Transport:** `POST /api/slackLists.create`, form or JSON.
- **Visibility:** public (`lists:write`, tier 2). OBSERVED: https://docs.slack.dev/reference/methods/slackLists.create
- **Request params:**
  - `name` - string - required;
  - `description_blocks` - JSON rich_text blocks - optional;
  - `schema` - JSON array - optional - columns `{key, name, type, is_primary_column?, options?}`;
  - `copy_from_list_id` - optional;
  - `include_copied_list_records` - bool - optional;
  - `todo_mode` - bool - optional - adds `todo_completed`, `todo_assignee`, `todo_due_date` columns.
    Column types:
  - docs: `text`, `message`, `number`, `select`, `date`, `user`, `attachment`, `checkbox`, `email`, `phone`, `channel`, `rating`, `created_by`, `last_edited_by`, `created_time`, `last_edited_time`, `vote`, `canvas`, `reference`, `link`;
  - slack-go adds `rich_text`, `multi_select`, `assignee`, `due_date`, `completed` and the `todo_*` types;
  - clack also saw `currency`.
    Options: `choices[{value, label, color}]`, `format` (`single_select`, `multi_select`, `single_entity`, `multi_entity`), `precision`, `date_format`, `emoji`, `emoji_team_id`, `max`, `default_value_typed{user, channel, select}`, `show_member_name`, `notify_users`. Colors: indigo, blue, cyan, pink, yellow, green, gray, red, purple, orange, brown.
    OBSERVED: https://docs.slack.dev/reference/methods/slackLists.create; slack-go/slack/lists.go:116-149; slack-go/slack/lists_types.go:23-55; slack-go/slack/lists_types.go:106-126; some-du6e/clack/docs/LISTS.md:21
- **Response:** `{"ok": true, "list_id": "F…", "list_metadata": {…}}`. OBSERVED: slack-go/slack/lists.go:11-14
- **Pagination:** n/a.
- **Used for:** the documented list-create path.

### slackLists.update

- **Transport:** `POST /api/slackLists.update`, form or JSON.
- **Visibility:** public (`lists:write`, tier 2). OBSERVED: https://docs.slack.dev/reference/methods/slackLists.update
- **Request params:** `id` - required - list id; `name` - string - optional; `description_blocks` - JSON - optional; `todo_mode` - bool - optional (shows or hides the task-tracking columns). OBSERVED: https://docs.slack.dev/reference/methods/slackLists.update; slack-go/slack/lists.go:205-226
- **Response:** `{"ok": true}`; `list_not_found` otherwise.
- **Pagination:** n/a.
- **Used for:** rename, description, todo mode.

### slackLists.access.set

- **Transport:** `POST /api/slackLists.access.set`, form or JSON.
- **Visibility:** public (`lists:write`, tier 3). OBSERVED: https://docs.slack.dev/reference/methods/slackLists.access.set
- **Request params:** `list_id` - required; `access_level` - required - `read`, `write` or `owner`; `channel_ids` - JSON array - XOR `user_ids` - JSON array; exactly one of the two lists is required. OBSERVED: https://docs.slack.dev/reference/methods/slackLists.access.set; slack-go/slack/lists.go:274-296
- **Response:** `{"ok": true}`; errors such as `restricted_action`.
- **Pagination:** n/a.
- **Used for:** sharing a list.

### slackLists.access.delete

- **Transport:** `POST /api/slackLists.access.delete`, form or JSON.
- **Visibility:** public (`lists:write`, tier 3). OBSERVED: https://docs.slack.dev/reference/methods/slackLists.access.delete
- **Request params:** `list_id` - required; `channel_ids` XOR `user_ids` - JSON array. OBSERVED: https://docs.slack.dev/reference/methods/slackLists.access.delete; slack-go/slack/lists.go:344-365
- **Response:** `{"ok": true}`, or `{"ok": true, "failed_to_update_channel_ids": ["C…"]}` / `{"ok": true, "failed_to_update_user_ids": ["U…"]}` on partial failure.
- **Pagination:** n/a.
- **Used for:** unsharing a list.

### slackLists.download.start

- **Transport:** `POST /api/slackLists.download.start`, form or JSON.
- **Visibility:** public (`lists:read`, tier 2). OBSERVED: https://docs.slack.dev/reference/methods/slackLists.download.start
- **Request params:** `list_id` - string - required; `include_archived` - bool - optional; `format` - optional - `csv` (default) or `json`; `include_threads` and `include_attachments` - bool - optional, json only. OBSERVED: https://docs.slack.dev/reference/methods/slackLists.download.start; slack-go/slack/lists.go:429-450
- **Response:** `{"ok": true, "job_id": "LeF1234567"}`. OBSERVED: https://docs.slack.dev/reference/methods/slackLists.download.start
- **Pagination:** n/a; poll `slackLists.download.get`.
- **Used for:** CSV/JSON export. The private client exposes `list_csv_download_url` on the file instead. OBSERVED: slack-go/slack/files.go:166

### slackLists.download.get

- **Transport:** `POST /api/slackLists.download.get`, form or JSON.
- **Visibility:** public (`lists:read`, tier 4). OBSERVED: https://docs.slack.dev/reference/methods/slackLists.download.get
- **Request params:** `list_id` - required; `job_id` - required; `format`, `include_threads`, `include_attachments`, `include_archived` - optional - must match the values sent to `download.start`. OBSERVED: https://docs.slack.dev/reference/methods/slackLists.download.get; slack-go/slack/lists.go:506-524
- **Response:** `{"ok": true, "status": "COMPLETED", "download_url": "https://files.com/files-pri/1234567890-F12345678/csv/list?origin_team=T1234567890"}`. OBSERVED: https://docs.slack.dev/reference/methods/slackLists.download.get; slack-go/slack/lists.go:24-27
- **Pagination:** poll until `status` is `COMPLETED`.
- **Used for:** fetching the export.

---

**Clips (audio/video messages)**

### media.progress.update

- **Transport:** `POST /api/media.progress.update`, form.
- **Visibility:** private.
- **Request params:** `file`, `offset_ms`, `duration_ms` - ? - ? (catalogue). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:586
- **Response:** not observed.
- **Pagination:** n/a.
- **Used for:** not observed beyond the name (see LEADS). OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:859

### media.reactions.add / media.reactions.remove

- **Transport:** `POST /api/media.reactions.add` and `/api/media.reactions.remove`, form.
- **Visibility:** private.
- **Request params:** `channel`, `file`, `message_timestamp`, `name`, `offset_ms` - ? - ? (catalogue). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:587; 3kh0/slack-datamine/build/api-args.txt:589
- **Response:** not observed. RTM counterparts: `media_reaction_added` / `media_reaction_removed`. OBSERVED: 3kh0/slack-datamine/build/rtm-events.txt:216-217
- **Pagination:** n/a.
- **Used for:** emoji reactions pinned to a playback offset (`offset_ms`).

### media.reactions.get

- **Transport:** `POST /api/media.reactions.get`, form.
- **Visibility:** private.
- **Request params:** `channel`, `file`, `message_timestamp` - ? - ? (catalogue). OBSERVED: 3kh0/slack-datamine/build/api-args.txt:588
- **Response:** not observed.
- **Pagination:** n/a.
- **Used for:** loading a clip's timed reactions.

Clip recording and upload go through the Files area (`files.getUploadURL` → bytes → `files.completeUpload`). Audio clips use `subtype: "slack_audio"`. OBSERVED: 3kh0/slack-datamine/build/api-methods.txt:656; 3kh0/slack-datamine/build/api-methods.txt:689; mautrix/slack/pkg/msgconv/from-matrix.go:159-161

---

**Legacy Posts and type filters**

### files.list (types filter for Posts and canvases)

- **Transport:** the docs give `GET https://slack.com/api/files.list` with a form content type; slack-go sends it as a `POST` form and the openclaw plugin as a `POST`. The method belongs to the Files reference; only the type values that matter here are listed. OBSERVED: https://docs.slack.dev/reference/methods/files.list; slack-go/slack/files.go:411; slack-go/slack/files.go:321-323; slack-go/slack/misc.go:498-504; stevekinney/openclaw-plugin-slack-workspace/src/client.ts:299-300
- **Visibility:** public.
- **Request params:** `types` - comma-separated string - optional. Documented values: `all`, `spaces` (= Posts), `snippets`, `images`, `gdocs`, `zips`, `pdfs`. A canvas plugin passes `types=canvas` and gets canvases back (`filetype: "quip"`, `mode: "quip"`). OBSERVED: https://docs.slack.dev/reference/methods/files.list; stevekinney/openclaw-plugin-slack-workspace/src/canvas-list.test.ts:10; stevekinney/openclaw-plugin-slack-workspace/src/canvas-list.test.ts:48
- **Response:** `files[]` file objects (Files reference).
- **Pagination:** `count` / `page`. OBSERVED: https://docs.slack.dev/reference/methods/files.list
- **Used for:** the Files browser's Posts and Canvases filters. The web client lists Lists differently, with `search.modules.files` and `query: "type:lists"` (Search area). OBSERVED: some-du6e/clack/docs/LISTS.md:17

## Websocket events

Everything except the Quip collab socket arrives on the normal RTM gateway as JSON frames with a `type` field; the gateway itself is covered in the realtime reference. OBSERVED: some-du6e/clack/docs/CANVASES.md:26-31; some-du6e/clack/docs/LISTS.md:92

### Canvas document and viewer events

| event                                                                                                                                                                                                                    | payload excerpt                                                                                                                                                   | fires when                                                                                             | OBSERVED                                                                                                                                                                       |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `document_update`                                                                                                                                                                                                        | `{"type": "document_update", "file_id": "F…", "payload": …}`                                                                                                      | an edit to the canvas was saved; the client maps file → Quip thread and calls `/canvas/-/fetch-latest` | OBSERVED: some-du6e/clack/docs/CANVASES.md:28; 3kh0/slack-datamine/build/rtm-events.txt:133                                                                                    |
| `document_change`                                                                                                                                                                                                        | `{"type": "document_change", "channel": "F…", "payload": "<base64>"}`; the client also sends its own `{type: "document_change", user, channel, payload}` upstream | transient sections: cursors, selections, presence                                                      | OBSERVED: some-du6e/clack/docs/CANVASES.md:29; 3kh0/slack-datamine/build/rtm-events.txt:132                                                                                    |
| `file_opened` / `file_closed`                                                                                                                                                                                            | `{"file": {"id": "F…"}, "viewer": {"user_id": "U…"}, "event_ts": "…"}`                                                                                            | someone opens or closes the canvas (header facepile)                                                   | OBSERVED: some-du6e/clack/docs/CANVASES.md:30; some-du6e/clack/docs/CANVASES.md:70; 3kh0/slack-datamine/build/rtm-events.txt:160; 3kh0/slack-datamine/build/rtm-events.txt:170 |
| `file_channel_canvas_reset`                                                                                                                                                                                              | not observed                                                                                                                                                      | not observed (clack refreshes on it)                                                                   | OBSERVED: some-du6e/clack/docs/CANVASES.md:31; some-du6e/clack/docs/CANVASES.md:70; 3kh0/slack-datamine/build/rtm-events.txt:159                                               |
| `file_change`, `file_shared`, `file_deleted`, `file_created`, `file_public`, `file_unshared`                                                                                                                             | generic file events (Files area)                                                                                                                                  | they cover canvas/list title, schema, view and sharing changes                                         | OBSERVED: slack-go/slack/websocket_managed_conn.go:586-595; some-du6e/clack/docs/LISTS.md:103; some-du6e/clack/docs/CANVASES.md:70                                             |
| `canvas_created`, `agent_canvas_created`, `canvas_content_draft_proposed`, `canvas_content_draft_resolved`, `canvas_content_draft_apply_failed`, `canvas_content_draft_view_triggered`, `canvas_ai_sections_highlighted` | not observed                                                                                                                                                      | names only                                                                                             | OBSERVED: 3kh0/slack-datamine/build/rtm-events.txt:8; 3kh0/slack-datamine/build/rtm-events.txt:76-81                                                                           |
| Quip collab socket                                                                                                                                                                                                       | `wss://listenweb3.slack-prod.onquip.com/-/listen/3`; frames not captured                                                                                          | the web editor's realtime collaboration connection                                                     | OBSERVED: .omo/research/gist.md:254; .omo/research/gist.md:347                                                                                                                 |

### List events

| event                   | payload excerpt                                                                                    | fires when / client action                                              | OBSERVED                                                                                  |
| ----------------------- | -------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| `list_record_created`   | `{"type": "list_record_created", "list_id": "F…", "record": {…}}`                                  | item created → upsert                                                   | OBSERVED: some-du6e/clack/docs/LISTS.md:96; 3kh0/slack-datamine/build/rtm-events.txt:209  |
| `list_record_updated`   | `{…, "list_id": "F…", "record": {…}}` or `{…, "deleted": true, "id": "Rec…"}`                      | item changed or deleted → upsert / remove                               | OBSERVED: some-du6e/clack/docs/LISTS.md:97; 3kh0/slack-datamine/build/rtm-events.txt:211  |
| `list_record_removed`   | `{…, "list_id": "F…", "id": "Rec…"}`                                                               | item removed → remove                                                   | OBSERVED: some-du6e/clack/docs/LISTS.md:98; 3kh0/slack-datamine/build/rtm-events.txt:210  |
| `list_records_archived` | `{…, "list_id": "F…", "ids": ["Rec…"], "restore": <bool>}`                                         | items archived or restored                                              | OBSERVED: some-du6e/clack/docs/LISTS.md:99; 3kh0/slack-datamine/build/rtm-events.txt:212  |
| `list_cells_updated`    | `{…, "list_id": "F…", "cells": [{"row_id": "Rec…", "column_id": "Col…", …typed}], "column": {…}?}` | cell edits; `column` is present when a new schema column came with them | OBSERVED: some-du6e/clack/docs/LISTS.md:100; 3kh0/slack-datamine/build/rtm-events.txt:207 |
| `list_contents_merged`  | `{…, "list_id": "F…"}`                                                                             | refetch all records                                                     | OBSERVED: some-du6e/clack/docs/LISTS.md:101; 3kh0/slack-datamine/build/rtm-events.txt:208 |

Activity-feed items (not RTM frames) for lists: `list_record_assigned`, `list_record_edited`, `list_user_mentioned`, `list_todo_notification`, `list_approval_request`, `list_approval_reviewed`, each with a `list_*_payload {list_id, row_id, …}`. OBSERVED: some-du6e/clack/docs/LISTS.md:103

### Clip events

| event                                             | payload excerpt | fires when                                                | OBSERVED                                                   |
| ------------------------------------------------- | --------------- | --------------------------------------------------------- | ---------------------------------------------------------- |
| `media_reaction_added` / `media_reaction_removed` | not observed    | names only (counterparts of `media.reactions.add/remove`) | OBSERVED: 3kh0/slack-datamine/build/rtm-events.txt:216-217 |

### Message subtypes and inline references for this area

- Message subtypes (delivered as `message` events): `canvas`, `canvas_editing_permissions_changed`, `canvas_open_by_default`, `canvas_section_stub`, `canvas_sharing_message`, `channel_canvas_updated` (+ `channel_canvas_updated_unread_ignore`), `tabbed_canvas_updated` (+ `tabbed_canvas_updated_unread_ignore`), `document_comment_root`, `document_mention`, `list_edit`, `list_mention`, `list_record_comment`. OBSERVED: 3kh0/slack-datamine/build/message-subtypes.txt:19-26; 3kh0/slack-datamine/build/message-subtypes.txt:48-49; 3kh0/slack-datamine/build/message-subtypes.txt:70-72; 3kh0/slack-datamine/build/message-subtypes.txt:91-92
- clack re-reads an open canvas on `channel_canvas_updated` / `tabbed_canvas_updated` messages and on the events above, coalescing bursts into one refresh per 400 ms. OBSERVED: some-du6e/clack/docs/CANVASES.md:70
- Rich-text elements:
  - `{"type": "canvas", "file_id": "F…", "label": …, "hide_title": …, "section_id": "temp:C:…", "text": …, "url": "https://<ws>.slack.com/docs/T…/F…"}`;
  - `canvas_message_unfurl`;
  - `canvas_user_mention`;
  - `{"type": "list_record", "file_id": "F…", "record_id": "Rec…", "view_id": "View…", "text": …}`.
    OBSERVED: slack-go/slack/block_rich_text.go:290-292; slack-go/slack/block_rich_text.go:300; slack-go/slack/block_rich_text.go:545-551; slack-go/slack/block_rich_text.go:626-632; slack-go/slack/block_rich_text_canvas_test.go:25
- A rich-text `canvas` element renders as the canvas name, as "{canvasName}: {sectionText}" when it points at a section, or as "Deleted canvas" / "Private canvas". A shared canvas section arrives as a `document_section` attachment: the section's blocks plus a footer with "View section" and "Canvas". OBSERVED: some-du6e/clack/docs/CANVASES.md:192
- A huddle room points at its notes canvas through `canvas_thread_ts` / `canvas_background` on the room object (Huddles area). OBSERVED: slack-go/slack/websocket_misc.go:186; slack-go/slack/websocket_misc.go:200; slack-go/slack/websocket_misc.go:209

## Gotchas

- **Session auth works for this area in practice.** clack authenticates with the browser's `xoxc` token plus `Cookie: d=…` and makes these calls with it:
  - public `canvases.edit`, `canvases.create`, `canvases.access.set`;
  - private `lists.*`, `files.open/close` and the tab methods.

  slackdump's canvas-comment path also requires an xoxc-backed client. OBSERVED: some-du6e/clack/docs/DESKTOP-SIGNIN.md:9; some-du6e/clack/internal/core/canvas.go:690; some-du6e/clack/internal/core/canvas.go:724; some-du6e/clack/internal/core/canvas.go:753; some-du6e/clack/internal/core/lists.go:187; some-du6e/clack/internal/core/canvas.go:472; some-du6e/clack/internal/core/channel_tabs_edit.go:80; rusq/slackdump/internal/client/pool.go:126-128

- **Enterprise Grid.** Every public canvas method documents `enterprise_is_restricted` ("The method cannot be called from an Enterprise"). Grid requests go through `<workspace>.enterprise.slack.com` with a `slack_route` parameter (transport reference). OBSERVED: https://docs.slack.dev/reference/methods/canvases.create; https://docs.slack.dev/reference/methods/canvases.getContent; .omo/research/gist.md:77; .omo/research/gist.md:86; .omo/research/gist.md:341
- **Free plan.** One canvas tab per channel and no standalone canvases (`free_team_canvas_tab_already_exists`, `free_teams_cannot_create_standalone_canvases`). Lists and canvas access methods are documented as paid-plan only. OBSERVED: https://docs.slack.dev/reference/methods/canvases.create; https://docs.slack.dev/reference/methods/canvases.access.set; https://docs.slack.dev/reference/methods/slackLists.items.list
- **`canvases.edit` limits.**
  - One change per call.
  - At most 1 MiB of markdown per change; tables at most 300 cells.
  - Canvas markdown is not Slack mrkdwn: `*x*` is italic and `<url|text>` is not a link.
  - No raw HTML.
    OBSERVED: https://docs.slack.dev/reference/methods/canvases.edit; stevekinney/openclaw-plugin-slack-workspace/skills/slack-canvas/references/canvas-markdown.md:11; stevekinney/openclaw-plugin-slack-workspace/skills/slack-canvas/references/canvas-markdown.md:23-30
- **Mentions in canvas markdown** are `![](@U123ABCDEFG)` for a user and `![](#C123ABC456)` for a channel. clack also writes `:emoji:`, `@here`, `- [ ]`/`- [x]`, `~~strike~~` and `_italic_`, with two spaces per nesting level. OBSERVED: https://docs.slack.dev/surfaces/canvases; some-du6e/clack/docs/CANVASES.md:84
- **What markdown can't express** (keep these blocks read-only in an editor):
  - underline, highlight, dates, inline images;
  - @here/@channel/@everyone and user groups;
  - hard line breaks inside a block;
  - links styled as code, and code blocks that contain a fence;
  - callouts and layouts (columns).
    OBSERVED: some-du6e/clack/docs/CANVASES.md:103; some-du6e/clack/docs/CANVASES.md:213-215
- **Access levels.**
  - `canvases.access.set` takes only read, write and owner. There is no comment level, although the client has a `canvas_comment_only_mode`.
  - At most 20 ids, and DM/MPDM channel ids are rejected.
  - Per-user changes fail until the user has been sent the canvas.
  - `canvases.access.delete` takes exactly one id.
    OBSERVED: https://docs.slack.dev/reference/methods/canvases.access.set; https://docs.slack.dev/reference/methods/canvases.access.delete; some-du6e/clack/docs/CANVASES.md:148-152
- **A canvas title is derived from its document.** Quip takes the TITLE section, else the first heading, else the first five words, else "Untitled". The HTML export has no separate title element, and editing the opening heading renames the canvas. OBSERVED: some-du6e/clack/docs/CANVASES.md:33
- **The file channel.** Comments for canvas `F06R4HA3ZS8` live in channel `C06R4HA3ZS8`, and list-item comments use the same rule. OBSERVED: rusq/slackdump/internal/structures/canvases.go:34-39; some-du6e/clack/docs/CANVASES.md:115; some-du6e/clack/docs/LISTS.md:69
- **Time units are mixed.** Canvas `last_read` is in milliseconds; Quip comment metadata and `document_comment.last_seen` are in microseconds. OBSERVED: slack-go/slack/files.go:136-137; rusq/slackdump/internal/edge/canvas.go:323-325; some-du6e/clack/docs/CANVASES.md:115
- **HTML export safety.** A file URL that answers with HTML may be a sign-in page, so check the host chain (`files.slack.com/files-pri/…`), the content type and the `quip-canvas-content` marker, and cap the size. Sanitize before display; slackdump uses bluemonday's UGC policy and a `default-src 'none'` CSP. OBSERVED: some-du6e/clack/internal/slack/canvas.go:16-21; some-du6e/clack/internal/slack/canvas.go:65; rusq/slackdump/internal/viewer/canvas.go:9; rusq/slackdump/internal/viewer/canvas.go:21
- **Images in canvases** point at `/collab-slack-blob/<thread>/<F…>?size_name=…`, and embedded files carry `sf:F…` ids. clack resolves both through `files.info` (at most 24 per canvas, 4 at a time) and never loads Slack web pages as images. OBSERVED: some-du6e/clack/docs/CANVASES.md:41; some-du6e/clack/docs/CANVASES.md:56; some-du6e/clack/docs/CANVASES.md:59; some-du6e/clack/docs/CANVASES.md:216
- **Quip protocol churn.**
  - load-data carries `_version=10` and `_x_version_ts`.
  - controller-init reports a dated Quip build string.
  - The section/edit schema is Quip-internal and changes with Quip's builds.
    OBSERVED: rusq/slackdump/internal/edge/canvas.go:358-366; .omo/research/gist.md:259; some-du6e/clack/docs/CANVASES.md:22
- **Lists: private vs public shapes.**
  - Cell edits: `lists.cells.update {cells}` vs `slackLists.items.update {cells}`.
  - Item create: `lists.records.create {initial_fields, position, updated_timestamp, duplicated_record_id, parent_record_id}` vs `slackLists.items.create {initial_fields, duplicated_item_id, parent_item_id}`.
  - Text cells must be sent as `rich_text`.
  - Link keys are snake_case in requests and camelCase in responses.
  - Checkbox: the docs' response template shows `checkbox: [bool]`, while slack-go and clack decode a bare bool.
    OBSERVED: some-du6e/clack/docs/LISTS.md:59; some-du6e/clack/docs/LISTS.md:111-112; https://docs.slack.dev/reference/methods/slackLists.items.list; https://docs.slack.dev/reference/methods/slackLists.items.create; slack-go/slack/lists_types.go:157-161; slack-go/slack/lists_types.go:181; slack-go/slack/lists_types.go:195-200
- **List limits move.** The docs give a 5000-row page/list limit, but the docs' own example has `list_limits.row_count_limit: 1000`, and the client runs a `lists_records_list_5k` experiment. Read the limit from `list_limits` instead of hard-coding it. OBSERVED: https://docs.slack.dev/reference/methods/slackLists.items.list; https://docs.slack.dev/reference/methods/slackLists.items.info; .omo/research/gist.md:239
- **List ids and ordering.**
  - Id prefixes: `Col…`, `Opt…`, `View…`, `Rec…`; the primary column's key is `name`.
  - Sort by `view_positions[view]`, else `position` (fractional strings compared as strings), then `date_created`.
  - Multi-values flatten with `|`, `,` or `:` for filters and sorts.
    OBSERVED: some-du6e/clack/docs/LISTS.md:15; some-du6e/clack/docs/LISTS.md:65; some-du6e/clack/docs/LISTS.md:75
- **Permalinks.**
  - Canvas: `https://<ws>.slack.com/docs/<T>/<F>`; section links add `focus_section_id`.
  - List: `https://<ws>.slack.com/lists/<T>/<F>`; item links add `?record_id=Rec…`.
    OBSERVED: stevekinney/openclaw-plugin-slack-workspace/src/client.ts:444-448; some-du6e/clack/docs/CANVASES.md:155; some-du6e/clack/docs/LISTS.md:17
- **Prefs that change behaviour.**
  - `client.userBoot` prefs: `open_quip_doc_in_flexpane`, `channel_canvas_variant`, `channel_canvas_layouts`, `canvas_version_history_enabled`, `enable_quip_file_browser_view`, `lists_add_to_list_default_list`.
  - Team prefs `allow_video_clips` / `allow_audio_clips` gate the clip buttons.
  - `lists_tabbing_navigation_option_type` controls grid keyboard mode.
    OBSERVED: rusq/slackdump/internal/edge/assets/client.userBoot.json:151; rusq/slackdump/internal/edge/assets/client.userBoot.json:1244; rusq/slackdump/internal/edge/assets/client.userBoot.json:1251; rusq/slackdump/internal/edge/assets/client.userBoot.json:1257-1258; rusq/slackdump/internal/edge/assets/client.userBoot.json:1451; some-du6e/clack/docs/GAPS-2026-11-12.md:261-264; some-du6e/clack/docs/LISTS.md:128
- **Clips.**
  - Audio clips are uploaded as webm/opus named `*.m4a` with `subtype: "slack_audio"`; mautrix notes that Slack's servers appear to convert them to AAC.
  - Waveform samples are 0-100.
  - Recording uses the browser's `MediaRecorder`, and the clip experiments include `clips_pip`, `clips_thumbnail`, `clips_video_effects_pipeline`, `transcribe_opt_in_ui_v2`.
    OBSERVED: mautrix/slack/pkg/msgconv/from-matrix.go:155-161; mautrix/slack/pkg/msgconv/from-slack.go:457; some-du6e/clack/docs/GAPS-2026-11-12.md:261-268; some-du6e/clack/docs/EXPERIMENTS.md:744
- **Legacy Posts.** `files.list types=spaces` means Posts, and emacs-slack labels `filetype "post"` "Slack Post". The current client catalogue has no `posts.*` or `spaces.*` methods, only the "Post" file-type label, so treat posts as read-only. OBSERVED: https://docs.slack.dev/reference/methods/files.list; emacs-slack/emacs-slack/slack-file.el:159; 3kh0/slack-datamine/build/strings-en-US.txt:9888
- **What the other clients do.**
  - mautrix-slack bridges a canvas as its title plus an "Open the canvas in Slack" link (it fetches `files.info` when title or permalink is missing) and skips `mode: "tombstone"` files.
  - emacs-slack renders a rich-text `canvas` element as its URL.
  - wee-slack only types the canvas fields.
  - slack-libpurple has no canvas/list code (grep for `canvas|quip` over dylex/slack-libpurple@1115ec1 found nothing).
    OBSERVED: mautrix/slack/CHANGELOG.md:17; mautrix/slack/pkg/msgconv/from-slack.go:72; mautrix/slack/pkg/msgconv/from-slack.go:272-291; emacs-slack/emacs-slack/slack-block-rich-text-element.el:367-383; wee-slack/wee-slack/typings/slack_api/slack_conversations_history.pyi:387
- **Public rate tiers.**
  - Tier 2: `canvases.create`, `conversations.canvases.create`, `slackLists.create`, `slackLists.update`, `slackLists.items.list`, `slackLists.items.info`, `slackLists.items.delete`, `slackLists.items.deleteMultiple`, `slackLists.download.start`.
  - Tier 3: `canvases.edit`, `canvases.delete`, `canvases.sections.lookup`, `canvases.getContent`, `canvases.access.set`/`.delete`, `slackLists.items.create`, `slackLists.items.update`, `slackLists.access.set`/`.delete`.
  - Tier 4: `slackLists.download.get`.
  - Private-method limits are not documented.
    OBSERVED: the `rate_limit` field of each method's docs page, e.g. https://docs.slack.dev/reference/methods/slackLists.download.get
- **Source pins** (re-diff these when Slack changes something):
  - slackdump fec2e68 (2026-11-14), beeper/slackgo d0ae74c (2026-11-17), slack-go bb921e9 (2026-11-17), mautrix-slack 38d69d5 (2026-11-18);
  - clack 6548bfb (2026-11-19), slack-datamine 08459f0 (2026-11-19, web build 137958);
  - wee-slack aa904c2, emacs-slack eafa2d4, slack-libpurple 1115ec1, openclaw-plugin-slack-workspace 0fff93f.

  mautrix-slack pins the beeper fork via a `replace` directive. OBSERVED: mautrix/slack/go.mod:48; `git log -1` of each clone in this research session

## Feasibility assessment and staged plan

**Which canvas content format to build on.**

| option                   | how                                                        | for                                                                                                                                            | against                                                                                                        | verdict                           |
| ------------------------ | ---------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- | --------------------------------- |
| A. HTML export           | `files.info` → `GET url_private` with the `d` cookie       | already used with xoxc sessions (clack) and in archives (slackdump); full structure incl. section ids, tables, columns, images, mentions by id | undocumented markup that must be sanitised and mapped; images and embedded files need extra `files.info` calls | **primary for rendering**         |
| B. `canvases.getContent` | one public call, `markdown` or `html`                      | documented, same dialect as `canvases.edit`, no cookie download, no pagination                                                                 | new; xoxc behaviour unverified; the markdown example carries no section ids, so it can't target edits          | **fallback and read-before-edit** |
| C. Quip protobuf         | controller-init → load-data → fetch-latest / edit-document | full fidelity, live collaboration, comment anchors, presence                                                                                   | undocumented pblite schema, versioned per Quip build, edit/merge rules unknown                                 | **research only**                 |

OBSERVED: some-du6e/clack/docs/CANVASES.md:39-41; rusq/slackdump/internal/convert/html.go:239-259; https://docs.slack.dev/reference/methods/canvases.getContent; some-du6e/clack/docs/CANVASES.md:17-22

**Stage 1: read-only canvases.**

1. Detect canvases: `filetype == "quip"` on files, `properties.tabs[].type == "canvas"` and `properties.canvas.file_id` on conversations, and rich-text `canvas` elements.
2. Fetch `files.info` for title, `editable`, `access`, `editors` and `quip_thread_id`, then the HTML export. Validate the marker, sanitize, and map the export vocabulary (table above) to a native block tree that keeps section ids. Resolve mentions and emoji from the local user/channel/emoji caches, and images from `files.info` thumbnails.
3. Stay live: refetch on `document_update`, `file_change`, `file_channel_canvas_reset` and `channel_canvas_updated`/`tabbed_canvas_updated`, debounced. Use `files.open`/`files.close` with `file_opened`/`file_closed` for the facepile, and poll `files.info` when realtime is down.
4. Show comments read-only: `conversations.history` with `document_stream` plus `conversations.replies` on the file channel.

OBSERVED: some-du6e/clack/docs/CANVASES.md:41-70; some-du6e/clack/docs/CANVASES.md:123-124

**Stage 2: read-only Lists.** `files.info` (schema, views, limits) in parallel with `lists.records.list` (cursor loop) and `lists.open` (pinned views). Draw table, board and record views client-side, following the filter, sort, search and grouping rules in clack's notes. Apply `list_*` RTM events. Item comments live on the file channel. Fallback: `slackLists.items.list` with `include_list=true`. OBSERVED: some-du6e/clack/docs/LISTS.md:73-85; some-du6e/clack/docs/LISTS.md:92-101; https://docs.slack.dev/reference/methods/slackLists.items.list

**Stage 3: editing on the documented canvas surface.**

- Per-section `canvases.edit` (`replace`, `insert_after`, `delete`, `rename`) using the export's section ids.
- A fidelity gate: open a block in the editor only if its markdown round-trips; everything else stays read-only.
- Checklist ticks as a single-item `replace`.
- Comment replies via `chat.postMessage` to the file channel; resolve via `quip.thread.archive`.
- Create via `canvases.create` / `conversations.canvases.create`; share via `canvases.access.set` and then post the permalink.

OBSERVED: some-du6e/clack/docs/CANVASES.md:72-105; some-du6e/clack/docs/CANVASES.md:125-127

**Stage 4: List editing and tabs.** `lists.cells.update`; `lists.records.create` with a hybrid-logical-clock `updated_timestamp`; `lists.records.delete` / `lists.records.archive`; `lists.create`; `lists.addToList`; `conversations.addTab` / `conversations.updateTab` / `conversations.removeTab`. Schema and view editing (`lists.columns.*`, `lists.views.*`) come last because only names are known. OBSERVED: some-du6e/clack/docs/LISTS.md:111-118

**Stage 5: clips.** Player from `mp4`/`hls` with `thumb_video`; waveform from `audio_wave_samples`; transcript from `transcription.preview` / `vtt`; timed reactions via `media.reactions.*`. Recording and upload come last and belong to the Files area. OBSERVED: slack-go/slack/files.go:89-106; 3kh0/slack-datamine/build/api-args.txt:586-589

**Stage 6: Quip protocol spike (optional).** Some features exist only through `/canvas/-/edit-document`: starting comment threads (highlight anchors), section reactions, cover images, cursors/presence (`document_change`), callouts, dates and layouts. Size the reverse-engineering only after stages 1-5 ship. OBSERVED: some-du6e/clack/docs/CANVASES.md:109; some-du6e/clack/docs/CANVASES.md:117; some-du6e/clack/docs/CANVASES.md:190

**Legacy Posts:** show a read-only card or a link out (see LEADS for `content_html`).

| stage | deliverable                                                                                        | main calls                                                                                                | risk                  |
| ----- | -------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- | --------------------- |
| 0     | recognise canvas/list/clip/post files and tabs; card with title + "Open in Slack" (mautrix parity) | `files.info`, `conversations.info`                                                                        | low                   |
| 1     | read-only canvas (tab, side panel, full view) with live refresh and read-only comments             | `url_private` export, `files.open/close`, RTM `document_update`/`file_*`, `conversations.history/replies` | medium (scraped HTML) |
| 2     | read-only Lists (table/board/record) with live events                                              | `files.info`, `lists.records.list`, `lists.open`, `list_*` events                                         | medium (private API)  |
| 3     | canvas section editing, checklists, rename, comment replies/resolve, create/share                  | `canvases.edit/create/access.set`, `chat.postMessage`, `quip.thread.archive`                              | medium                |
| 4     | List cell/item editing, list create, tab management                                                | `lists.cells.update`, `lists.records.*`, `conversations.*Tab`                                             | medium                |
| 5     | clip playback, transcripts, timed reactions; recording later                                       | `files.info` media fields, `media.*`                                                                      | medium                |
| 6     | Quip protocol research (presence, anchors, full fidelity)                                          | `/canvas/collab/controller-init`, `/canvas/-/load-data`, `fetch-latest`, `edit-document`                  | high                  |

## LEADS

1. `canvases.getContent`, and public `canvases.*` / `slackLists.*` in general, probably accept an `xoxc` token: clack's `canvases.edit/create/access.set` calls do. `getContent` itself is untested with xoxc and could answer `not_allowed_token_type`.
2. `canvases.getContent content_type=html` probably returns the same Quip export as `url_private`, section ids included. Unverified.
3. It is unclear whether today's web editor still opens `wss://listenweb3.slack-prod.onquip.com/-/listen/3` or relies only on RTM `document_update`/`document_change`. The live capture and clack's build a84662d2d135 describe different paths.
4. `files.ping` is probably a heartbeat while a canvas is open or focused, driven by `files.open`'s `should_subscribe_and_ping`; the cadence is unknown.
5. The web client probably creates a channel canvas with `files.createCanvas` + `conversations.addTab {type: "canvas"}`, since `conversations.canvases.create` isn't in its catalogue.
6. `conversations.moveTab` probably takes `channel_id`, `id` and a position or `after` argument.
7. Response shapes are unknown for `files.close`, `files.ping`, `files.requestPermission`, `quip.lookupFileId`, `quip.history.*`, `quip.cloneCanvas`, `lists.records.info(Batch)`, `lists.records.delete/archive` and `media.*`.
8. Payloads are unknown for `canvas_created`, `agent_canvas_created`, `file_channel_canvas_reset`, `media_reaction_added/removed` and the `canvas_content_draft_*` events. The draft events are likely AI drafting.
9. `files.list types=canvas` works for one third-party plugin but isn't in the documented type list; a `lists` type value is unverified. The client uses `search.modules.files` with `type:lists`.
10. Legacy Posts: `files.info`'s top-level `content_html` probably holds the rendered post body; posts are probably no longer creatable.
11. Clip media URLs (`mp4`, `hls`, `vtt`) probably need the `d` cookie like other `files.slack.com` URLs, so native HLS playback would need cookie injection or a local proxy. `media.progress.update` probably syncs the resume position. `media_display_type` values are unknown.
12. Rate limits for private `lists.*`, `quip.*`, `files.open` and `/canvas/-/…` are unknown. Treat them like tier 3 and back off on `ratelimited` / HTTP 429.
13. On Enterprise Grid, private `lists.*` and `/canvas/-/…` may need the `<ws>.enterprise.slack.com` host plus `slack_route`. Whether `enterprise_is_restricted` hits xoxc sessions on org-level hosts is unknown.
14. When a `canvases.edit replace` races a live Quip edit on the same section, the outcome (last writer wins, rejection, or merge) is unknown.
15. clack only guessed the export markup for callouts (`div.callout`); confirm it on a real canvas.
16. `quip.getControllerInitData` may be the newer replacement for `/canvas/collab/controller-init`; its args weren't mined.
17. The 12 h canvas OAuth token from controller-init may be needed only for the Quip websocket/API hosts, not for `/canvas/-/…` on the team host.
18. `lists.records.list` with clack's `limit=1000`: whether the server honours `limit` is unknown; it isn't in the catalogue.
19. `slackLists.download.get` `status` values other than `COMPLETED` (pending or failed) are unknown.
20. unverified: `/canvas/-/fetch-latest` takes a raw binary protobuf body. clack's notes name only the message (`Request{ThreadRequest{…}}`), not its encoding (some-du6e/clack/docs/CANVASES.md:18), and load-data sends its protobuf base64-encoded in a `request_binary` form field (rusq/slackdump/internal/edge/canvas.go:366).
