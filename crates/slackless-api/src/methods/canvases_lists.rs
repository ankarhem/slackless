//! Canvases, Slack Lists, file collections/favorites/trash, and profile photos.

use super::*;

/// Returns whether a failed private `lists.*` call should fall back to the public
/// `slackLists.items.*` endpoint. Only expected method/token/scope rejections qualify; auth
/// expiry, rate limits, transport errors, and domain errors propagate immediately.
fn should_fallback_list_method(err: &ApiError) -> bool {
    matches!(
        err,
        ApiError::Slack(code)
            if matches!(
                code.as_str(),
                "not_allowed_token_type" | "unknown_method" | "missing_scope"
            )
    )
}

fn canvases_access_params(
    canvas_id: &str,
    access_level: Option<CanvasAccessLevel>,
    channel_ids: &[ChannelId],
    user_ids: &[UserId],
) -> Result<crate::client::Params> {
    if channel_ids.is_empty() == user_ids.is_empty() {
        return Err(ApiError::Slack("invalid_parameters".to_owned()));
    }
    let mut params = vec![("canvas_id", canvas_id.to_owned())];
    if let Some(level) = access_level {
        params.push(("access_level", level.as_str().to_owned()));
    }
    if !channel_ids.is_empty() {
        let ids: Vec<&str> = channel_ids.iter().map(ChannelId::as_str).collect();
        params.push(("channel_ids", serde_json::to_string(&ids)?));
    } else {
        let ids: Vec<&str> = user_ids.iter().map(UserId::as_str).collect();
        params.push(("user_ids", serde_json::to_string(&ids)?));
    }
    Ok(params)
}

fn lists_access_params(
    list_id: &str,
    access_level: Option<ListAccessLevel>,
    channel_ids: &[ChannelId],
    user_ids: &[UserId],
) -> Result<crate::client::Params> {
    if channel_ids.is_empty() == user_ids.is_empty() {
        return Err(ApiError::Slack("invalid_parameters".to_owned()));
    }
    let mut params = vec![("list_id", list_id.to_owned())];
    if let Some(level) = access_level {
        params.push(("access_level", level.as_str().to_owned()));
    }
    if !channel_ids.is_empty() {
        let ids: Vec<&str> = channel_ids.iter().map(ChannelId::as_str).collect();
        params.push(("channel_ids", serde_json::to_string(&ids)?));
    } else {
        let ids: Vec<&str> = user_ids.iter().map(UserId::as_str).collect();
        params.push(("user_ids", serde_json::to_string(&ids)?));
    }
    Ok(params)
}

impl SlackApi {
    /// Load a live Slack List (`files.info` for schema + `lists.records.list`, with public
    /// `slackLists.items.list` fallback) for native table/board rendering.
    pub async fn fetch_list(&self, list_id: &str) -> Result<SlackListData> {
        self.fetch_list_with_meta(list_id)
            .await
            .map(|(data, _)| data)
    }

    /// Load a live Slack List together with its parsed [`ListMeta`] (permalink, access level,
    /// editors, shared channels, and item limits).
    pub async fn fetch_list_with_meta(&self, list_id: &str) -> Result<(SlackListData, ListMeta)> {
        let info_val: Value = self
            .call(
                method::FILES_INFO,
                vec![("file", list_id.to_owned()), ("count", "0".to_owned())],
            )
            .await?
            .data;
        let file_obj = info_val.get("file").unwrap_or(&info_val);
        let (mut title, mut description, mut columns) = parse_list_schema(file_obj);
        let mut meta = parse_list_meta(file_obj);
        if meta.file_id.is_empty() {
            meta.file_id = list_id.to_owned();
            meta.comment_channel_id = list_comment_channel_id(list_id);
        }

        let private_res = self
            .call::<Value>(
                method::LISTS_RECORDS_LIST,
                vec![
                    ("list_id", list_id.to_owned()),
                    ("limit", "200".to_owned()),
                    ("archived", "false".to_owned()),
                ],
            )
            .await;

        let records_val = match private_res {
            Ok(resp) => resp.data,
            Err(ref err) if should_fallback_list_method(err) => {
                self.call::<Value>(
                    method::SLACK_LISTS_ITEMS_LIST,
                    vec![
                        ("list_id", list_id.to_owned()),
                        ("limit", "200".to_owned()),
                        ("include_list", "true".to_owned()),
                    ],
                )
                .await?
                .data
            }
            Err(err) => return Err(err),
        };

        if let Some(list_obj) = records_val.get("list") {
            if columns.is_empty() {
                let (t, d, cols) = parse_list_schema(list_obj);
                if !t.is_empty() {
                    title = t;
                }
                if !d.is_empty() {
                    description = d;
                }
                columns = cols;
            }
            if meta.permalink.is_none() {
                let fallback_meta = parse_list_meta(list_obj);
                if meta.permalink.is_none() {
                    meta.permalink = fallback_meta.permalink;
                }
                if meta.access.is_none() {
                    meta.access = fallback_meta.access;
                }
                if meta.org_or_workspace_access.is_none() {
                    meta.org_or_workspace_access = fallback_meta.org_or_workspace_access;
                }
                if meta.owner_id.is_none() {
                    meta.owner_id = fallback_meta.owner_id;
                }
                if meta.editors.is_empty() {
                    meta.editors = fallback_meta.editors;
                }
                if meta.shared_channels.is_empty() {
                    meta.shared_channels = fallback_meta.shared_channels;
                }
                if meta.row_count.is_none() {
                    meta.row_count = fallback_meta.row_count;
                }
                if meta.row_count_limit.is_none() {
                    meta.row_count_limit = fallback_meta.row_count_limit;
                }
            }
        }

        let raw_records = records_val
            .get("records")
            .or_else(|| records_val.get("items"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let rows = parse_list_records(raw_records);
        if meta.row_count.is_none() {
            meta.row_count = u32::try_from(rows.len()).ok();
        }
        if meta.title == "List" && !title.is_empty() {
            meta.title = title.clone();
        }

        Ok((
            SlackListData {
                file_id: list_id.to_owned(),
                title,
                description,
                columns,
                rows,
            },
            meta,
        ))
    }

    /// `files.favorites.list`: starred file IDs, newest star first (`F-CORE-133`).
    pub async fn files_favorites_list(&self) -> Result<Vec<String>> {
        let wire: FileFavoritesWire = self
            .call(
                method::FILES_FAVORITES_LIST,
                vec![("type", "all".to_owned())],
            )
            .await?
            .data;
        let mut ids = wire.file_ids;
        ids.reverse();
        Ok(ids)
    }

    /// `files.favorites.add` / `.remove`: star/unstar a file or add/remove it from a collection.
    pub async fn files_favorites_set(
        &self,
        file_id: &str,
        collection_id: Option<&str>,
        starred: bool,
    ) -> Result<()> {
        let mut params = vec![
            ("file_id", file_id.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(cid) = collection_id.filter(|s| !s.is_empty()) {
            params.push(("collection_id", cid.to_owned()));
        }
        let endpoint = if starred {
            method::FILES_FAVORITES_ADD
        } else {
            method::FILES_FAVORITES_REMOVE
        };
        self.call_ok(endpoint, params).await
    }

    /// `files.collections.list`: custom Files sidebar sections (`F-CORE-133`).
    pub async fn files_collections_list(&self) -> Result<Vec<FileCollection>> {
        let wire: FileCollectionsListWire = self
            .call(method::FILES_COLLECTIONS_LIST, Vec::new())
            .await?
            .data;
        Ok(wire.collections.into_iter().map(Into::into).collect())
    }

    /// `files.collections.create`: create a custom Files sidebar section (`F-CORE-133`).
    pub async fn files_collections_create(
        &self,
        name: &str,
        emoji: Option<&str>,
    ) -> Result<Vec<FileCollection>> {
        let mut params = vec![
            ("name", name.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(e) = emoji.filter(|s| !s.is_empty()) {
            params.push(("emoji", e.to_owned()));
        }
        let wire: FileCollectionsListWire = self
            .call(method::FILES_COLLECTIONS_CREATE, params)
            .await?
            .data;
        Ok(wire.collections.into_iter().map(Into::into).collect())
    }

    /// `files.collections.delete`: delete a custom Files sidebar section (`F-CORE-133`).
    pub async fn files_collections_delete(&self, collection_id: &str) -> Result<()> {
        let params = vec![
            ("collection", collection_id.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call_ok(method::FILES_COLLECTIONS_DELETE, params).await
    }

    /// `files.recentlyDeleted`: canvases & lists deleted in the last 24h (`F-CORE-133`).
    pub async fn files_recently_deleted(&self) -> Result<Vec<DeletedFile>> {
        let wire: RecentlyDeletedWire = self
            .call(method::FILES_RECENTLY_DELETED, Vec::new())
            .await?
            .data;
        Ok(wire
            .files
            .into_iter()
            .filter(|f| !f.id.is_empty())
            .map(|f| DeletedFile {
                id: f.id,
                title: if f.title.is_empty() {
                    "Untitled".to_owned()
                } else {
                    f.title
                },
                date_delete: f.date_delete,
                file_type: f.file_type,
            })
            .collect())
    }

    /// `files.undelete`: restore a recently deleted canvas or list (`F-CORE-133`).
    pub async fn files_undelete(&self, file_id: &str) -> Result<()> {
        let params = vec![
            ("file_id", file_id.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call_ok(method::FILES_UNDELETE, params).await
    }

    /// `canvases.create`: create a standalone canvas or channel-tabbed canvas (`F-COLLAB-119`).
    /// Falls back transparently to private `files.createCanvas` when `canvases.create` returns
    /// `not_allowed_token_type` on `xoxc-` web-client tokens.
    pub async fn canvases_create(
        &self,
        title: Option<&str>,
        markdown: Option<&str>,
        channel_id: Option<&ChannelId>,
    ) -> Result<CanvasCreated> {
        let mut params = Vec::new();
        if let Some(t) = title.filter(|s| !s.is_empty()) {
            params.push(("title", t.to_owned()));
        }
        if let Some(md) = markdown.filter(|s| !s.is_empty()) {
            let content = serde_json::json!({ "type": "markdown", "markdown": md }).to_string();
            params.push(("document_content", content));
        }
        if let Some(ch) = channel_id {
            params.push(("channel_id", ch.to_string()));
        }
        match self
            .call::<CanvasCreated>(method::CANVASES_CREATE, params)
            .await
        {
            Ok(resp) => Ok(resp.data),
            Err(ApiError::Slack(ref code)) if code == "not_allowed_token_type" => {
                self.files_create_canvas(title, markdown, channel_id).await
            }
            Err(err) => Err(err),
        }
    }

    /// Private web-client fallback for canvas creation (`files.createCanvas`).
    async fn files_create_canvas(
        &self,
        title: Option<&str>,
        markdown: Option<&str>,
        channel_id: Option<&ChannelId>,
    ) -> Result<CanvasCreated> {
        let mut params = vec![
            ("loosen_validation", "true".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(t) = title.filter(|s| !s.is_empty()) {
            params.push(("title", t.to_owned()));
        }
        if let Some(md) = markdown.filter(|s| !s.is_empty()) {
            params.push(("markdown", md.to_owned()));
        }
        if let Some(ch) = channel_id {
            params.push(("channel_id", ch.to_string()));
        }
        let raw: Value = self.call(method::FILES_CREATE_CANVAS, params).await?.data;
        let canvas_id = raw
            .get("file_id")
            .or_else(|| raw.get("canvas_id"))
            .or_else(|| raw.get("file").and_then(|f| f.get("id")))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if canvas_id.is_empty() {
            return Err(ApiError::Slack("canvas_creation_failed".to_owned()));
        }
        Ok(CanvasCreated { canvas_id })
    }

    /// `canvases.edit`: apply one section-level edit operation to a canvas (`F-COLLAB-132`, `F-SHELL-264`, `F-SHELL-277`).
    pub async fn canvases_edit(&self, canvas_id: &str, op: &CanvasEditOp) -> Result<()> {
        let changes = serde_json::Value::Array(vec![op.to_change_json()]).to_string();
        let params = vec![("canvas_id", canvas_id.to_owned()), ("changes", changes)];
        self.call_ok(method::CANVASES_EDIT, params).await
    }

    /// `canvases.getContent`: fetch a canvas's markdown or HTML body (`canvases.getContent`).
    pub async fn canvases_get_content(
        &self,
        canvas_id: &str,
        content_type: Option<&str>,
    ) -> Result<String> {
        let mut params = vec![("canvas_id", canvas_id.to_owned())];
        if let Some(ct) = content_type.filter(|s| !s.is_empty()) {
            params.push(("content_type", ct.to_owned()));
        }
        let wire: CanvasContentWire = self.call(method::CANVASES_GET_CONTENT, params).await?.data;
        Ok(wire.content)
    }

    /// `canvases.delete`: permanently delete a canvas, falling back to `files.delete` on
    /// `not_allowed_token_type` for `xoxc-` sessions.
    pub async fn canvases_delete(&self, canvas_id: &str) -> Result<()> {
        let params = vec![("canvas_id", canvas_id.to_owned())];
        match self.call_ok(method::CANVASES_DELETE, params).await {
            Ok(()) => Ok(()),
            Err(ApiError::Slack(ref code)) if code == "not_allowed_token_type" => {
                self.files_delete(canvas_id).await
            }
            Err(err) => Err(err),
        }
    }

    /// `canvases.access.set`: grant read/write/owner access on a canvas to channels or users (XOR).
    pub async fn canvases_access_set(
        &self,
        canvas_id: &str,
        access_level: CanvasAccessLevel,
        channel_ids: &[ChannelId],
        user_ids: &[UserId],
    ) -> Result<()> {
        let params = canvases_access_params(canvas_id, Some(access_level), channel_ids, user_ids)?;
        self.call_ok(method::CANVASES_ACCESS_SET, params).await
    }

    /// `canvases.access.delete`: revoke canvas access from channels or users (XOR).
    pub async fn canvases_access_delete(
        &self,
        canvas_id: &str,
        channel_ids: &[ChannelId],
        user_ids: &[UserId],
    ) -> Result<()> {
        let params = canvases_access_params(canvas_id, None, channel_ids, user_ids)?;
        self.call_ok(method::CANVASES_ACCESS_DELETE, params).await
    }

    /// `quip.thread.archive`: resolve one canvas comment thread.
    pub async fn quip_thread_archive(&self, channel_id: &ChannelId, thread_ts: &Ts) -> Result<()> {
        let params = vec![
            ("channel_id", channel_id.to_string()),
            ("thread_ts", thread_ts.to_string()),
        ];
        self.call_ok(method::QUIP_THREAD_ARCHIVE, params).await
    }

    /// `quip.thread.unarchive`: reopen one resolved canvas comment thread.
    pub async fn quip_thread_unarchive(
        &self,
        channel_id: &ChannelId,
        thread_ts: &Ts,
    ) -> Result<()> {
        let params = vec![
            ("channel_id", channel_id.to_string()),
            ("thread_ts", thread_ts.to_string()),
        ];
        self.call_ok(method::QUIP_THREAD_UNARCHIVE, params).await
    }

    /// `quip.thread.archiveAll`: resolve every open comment thread in a canvas comment channel.
    pub async fn quip_thread_archive_all(&self, channel_id: &ChannelId) -> Result<()> {
        let params = vec![("channel_id", channel_id.to_string())];
        self.call_ok(method::QUIP_THREAD_ARCHIVE_ALL, params).await
    }

    /// List comment threads on a canvas by reading its derived file comment channel (`C…`).
    pub async fn canvas_comments_list(&self, canvas_id: &str) -> Result<Vec<CanvasCommentThread>> {
        let comment_channel = canvas_comment_channel_id(canvas_id);
        let history_opts = HistoryOpts {
            limit: Some(100),
            ..Default::default()
        };
        let history = match self
            .conversations_history(&comment_channel, &history_opts, None)
            .await
        {
            Ok(page) => page.items,
            Err(ApiError::Slack(ref code)) if code == "channel_not_found" => {
                return Ok(Vec::new());
            }
            Err(err) => return Err(err),
        };

        let reply_opts = HistoryOpts {
            limit: Some(200),
            ..Default::default()
        };
        let mut replies_by_root: HashMap<Ts, Vec<Message>> = HashMap::new();
        for msg in &history {
            if msg.ts.as_str().is_empty()
                || msg.is_thread_reply()
                || msg.reply_count.unwrap_or(0) == 0
            {
                continue;
            }
            let replies = self
                .conversations_replies(&comment_channel, &msg.ts, &reply_opts, None)
                .await?;
            replies_by_root.insert(msg.ts.clone(), replies.items);
        }

        Ok(parse_canvas_comment_threads(&history, &replies_by_root))
    }

    /// Post a top-level comment or thread reply into a canvas's derived file comment channel (`C…`).
    pub async fn canvas_comment_post(
        &self,
        canvas_id: &str,
        text: &str,
        thread_ts: Option<&Ts>,
    ) -> Result<PostedMessage> {
        let comment_channel = canvas_comment_channel_id(canvas_id);
        self.chat_post_message(
            &comment_channel,
            &MessageContent::plain(text),
            thread_ts,
            false,
        )
        .await
    }

    /// `files.requestPermission`: request edit (`upgrade`) or comment (`comment_upgrade`) access (`F-COLLAB-129`).
    pub async fn files_request_permission(&self, file_id: &str, request_type: &str) -> Result<()> {
        let params = vec![
            ("file_id", file_id.to_owned()),
            ("request_type", request_type.to_owned()),
            ("reason", "IndicatorRequestEditAccessButton".to_owned()),
        ];
        self.call_ok(method::FILES_REQUEST_PERMISSION, params).await
    }

    /// `lists.create`: create a new Slack List from scratch (`F-COLLAB-213`).
    pub async fn lists_create(
        &self,
        name: &str,
        description: Option<&str>,
        channel_id: Option<&ChannelId>,
    ) -> Result<String> {
        let mut params = vec![("name", name.to_owned())];
        if let Some(desc) = description.filter(|s| !s.is_empty()) {
            params.push(("description", desc.to_owned()));
        }
        if let Some(ch) = channel_id {
            params.push(("context_channel_id", ch.to_string()));
        }
        let wire: ListCreateWire = self.call(method::LISTS_CREATE, params).await?.data;
        let id = wire
            .list_id
            .or_else(|| {
                wire.list
                    .as_ref()
                    .and_then(|v| v.get("id"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_default();
        Ok(id)
    }

    /// `lists.records.create` (with `slackLists.items.create` fallback): add an item to a List (`F-COLLAB-226`).
    pub async fn lists_records_create(
        &self,
        list_id: &str,
        initial_fields: &[ListCellInput],
        parent_record_id: Option<&str>,
    ) -> Result<SlackListRow> {
        let fields_json =
            Value::Array(initial_fields.iter().map(|c| c.to_json(None)).collect()).to_string();
        let mut params = vec![
            ("list_id", list_id.to_owned()),
            ("initial_fields", fields_json.clone()),
        ];
        if let Some(parent) = parent_record_id.filter(|s| !s.is_empty()) {
            params.push(("parent_record_id", parent.to_owned()));
        }
        let wire: ListRecordCreateWire = match self.call(method::LISTS_RECORDS_CREATE, params).await
        {
            Ok(resp) => resp.data,
            Err(ref err) if should_fallback_list_method(err) => {
                let mut fallback = vec![
                    ("list_id", list_id.to_owned()),
                    ("initial_fields", fields_json),
                ];
                if let Some(parent) = parent_record_id.filter(|s| !s.is_empty()) {
                    fallback.push(("parent_item_id", parent.to_owned()));
                }
                self.call(method::SLACK_LISTS_ITEMS_CREATE, fallback)
                    .await?
                    .data
            }
            Err(err) => return Err(err),
        };
        let raw = wire.record.or(wire.item);
        let row = raw
            .as_ref()
            .and_then(|v| {
                parse_list_records(std::slice::from_ref(v))
                    .into_iter()
                    .next()
            })
            .unwrap_or_default();
        Ok(row)
    }

    /// `lists.cells.update` (with `slackLists.items.update` fallback): update one or more cells (`F-COLLAB-227`, `F-COLLAB-248`).
    pub async fn lists_cells_update(
        &self,
        list_id: &str,
        row_id: &str,
        cells: &[ListCellInput],
    ) -> Result<()> {
        let cells_json =
            Value::Array(cells.iter().map(|c| c.to_json(Some(row_id))).collect()).to_string();
        let params = vec![
            ("list_id", list_id.to_owned()),
            ("cells", cells_json.clone()),
        ];
        match self.call_ok(method::LISTS_CELLS_UPDATE, params).await {
            Ok(()) => Ok(()),
            Err(ref err) if should_fallback_list_method(err) => {
                let fallback = vec![("list_id", list_id.to_owned()), ("cells", cells_json)];
                self.call_ok(method::SLACK_LISTS_ITEMS_UPDATE, fallback)
                    .await
            }
            Err(err) => Err(err),
        }
    }

    /// `lists.records.delete` (with `slackLists.items.delete` fallback): delete one list item (`F-COLLAB-228`).
    pub async fn lists_records_delete(&self, list_id: &str, row_id: &str) -> Result<()> {
        let params = vec![("list_id", list_id.to_owned()), ("id", row_id.to_owned())];
        match self
            .call_ok(method::LISTS_RECORDS_DELETE, params.clone())
            .await
        {
            Ok(()) => Ok(()),
            Err(ref err) if should_fallback_list_method(err) => {
                self.call_ok(method::SLACK_LISTS_ITEMS_DELETE, params).await
            }
            Err(err) => Err(err),
        }
    }

    /// `lists.addToList`: add a Slack message to an existing List (`F-COLLAB-231`).
    pub async fn lists_add_to_list(
        &self,
        list_id: &str,
        channel: &ChannelId,
        message_ts: &Ts,
    ) -> Result<String> {
        let params = vec![
            ("list_id", list_id.to_owned()),
            ("channel_id", channel.to_string()),
            ("message_ts", message_ts.to_string()),
            ("reason", "add-to-list".to_owned()),
        ];
        let wire: ListRecordCreateWire = self.call(method::LISTS_ADD_TO_LIST, params).await?.data;
        let id = wire
            .record
            .as_ref()
            .and_then(|v| v.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Ok(id)
    }

    /// `lists.records.archive`: archive one list item (`F-COLLAB-255`).
    pub async fn lists_records_archive(&self, list_id: &str, row_id: &str) -> Result<()> {
        let params = vec![
            ("list_id", list_id.to_owned()),
            ("record_id", row_id.to_owned()),
            ("id", row_id.to_owned()),
        ];
        self.call_ok(method::LISTS_RECORDS_ARCHIVE, params).await
    }

    /// `slackLists.access.set`: grant read/write/owner access on a list to channels or users (XOR)
    /// (`F-COLLAB-243..245`, `F-COLLAB-274..278`).
    pub async fn lists_access_set(
        &self,
        list_id: &str,
        access_level: ListAccessLevel,
        channel_ids: &[ChannelId],
        user_ids: &[UserId],
    ) -> Result<()> {
        let params = lists_access_params(list_id, Some(access_level), channel_ids, user_ids)?;
        self.call_ok(method::SLACK_LISTS_ACCESS_SET, params).await
    }

    /// `slackLists.access.delete`: revoke list access from channels or users (XOR)
    /// (`F-COLLAB-243..245`, `F-COLLAB-274..278`).
    pub async fn lists_access_delete(
        &self,
        list_id: &str,
        channel_ids: &[ChannelId],
        user_ids: &[UserId],
    ) -> Result<ListAccessDeleteResult> {
        let params = lists_access_params(list_id, None, channel_ids, user_ids)?;
        Ok(self
            .call(method::SLACK_LISTS_ACCESS_DELETE, params)
            .await?
            .data)
    }

    /// `lists.getThreadsForRecord`: load comment thread messages attached to a list item
    /// (`F-COLLAB-250`, `F-COLLAB-251`, `F-COLLAB-272`), falling back to `conversations.replies`
    /// on the list's file channel (`C…`, `ts = "<list_id>-<record_id>"`) when needed.
    pub async fn lists_get_threads_for_record(
        &self,
        list_id: &str,
        record_id: &str,
    ) -> Result<Vec<Message>> {
        let params = vec![
            ("list_id", list_id.to_owned()),
            ("record_id", record_id.to_owned()),
        ];
        let mut out = match self
            .call::<Value>(method::LISTS_GET_THREADS_FOR_RECORD, params)
            .await
        {
            Ok(resp) => parse_list_record_threads(&resp.data),
            Err(ref err) if should_fallback_list_method(err) => Vec::new(),
            Err(err) => return Err(err),
        };
        if out.is_empty() && !list_id.is_empty() && !record_id.is_empty() {
            let comment_ch = list_comment_channel_id(list_id);
            let synthetic_ts = Ts::from(format!("{list_id}-{record_id}"));
            let opts = HistoryOpts {
                limit: Some(100),
                ..Default::default()
            };
            if let Ok(page) = self
                .conversations_replies(&comment_ch, &synthetic_ts, &opts, None)
                .await
            {
                out = page.items;
            }
        }
        Ok(out)
    }

    /// `users.setPhoto`: upload a new profile photo (`F-SHELL-004`).
    pub async fn users_set_photo(&self, filename: &str, bytes: &[u8]) -> Result<()> {
        let lower = filename.to_ascii_lowercase();
        let ct = if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
            "image/jpeg"
        } else if lower.ends_with(".gif") {
            "image/gif"
        } else if lower.ends_with(".webp") {
            "image/webp"
        } else {
            "image/png"
        };
        let _ = self
            .post_multipart_file(method::USERS_SET_PHOTO, "image", filename, ct, bytes)
            .await?;
        Ok(())
    }
}
