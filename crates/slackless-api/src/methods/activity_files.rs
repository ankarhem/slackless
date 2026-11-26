//! Pins, bookmarks, drafts, files, activity feed, GIFs, saved items, Edge search, and prefs.

use super::*;

impl SlackApi {
    /// `pins.list`: a conversation's pinned messages and files, oldest pin first.
    pub async fn pins_list(&self, channel: &ChannelId) -> Result<PinsList> {
        let params = vec![("channel", channel.to_string())];
        Ok(self.call(method::PINS_LIST, params).await?.data)
    }

    /// `pins.add`: pin the message `ts` in `channel`.
    pub async fn pins_add(&self, channel: &ChannelId, ts: &Ts) -> Result<()> {
        let params = vec![
            ("channel", channel.to_string()),
            ("timestamp", ts.to_string()),
        ];
        self.call_ok(method::PINS_ADD, params).await
    }

    /// `pins.remove`: unpin the message `ts` in `channel`.
    pub async fn pins_remove(&self, channel: &ChannelId, ts: &Ts) -> Result<()> {
        let params = vec![
            ("channel", channel.to_string()),
            ("timestamp", ts.to_string()),
        ];
        self.call_ok(method::PINS_REMOVE, params).await
    }

    /// `bookmarks.list`: the conversation's bookmarks, folders included. Fetched lazily by
    /// the tab row (once per conversation, when a bookmarks surface is opened).
    pub async fn bookmarks_list(&self, channel: &ChannelId) -> Result<BookmarksList> {
        let params = vec![
            ("channel", channel.to_string()),
            ("include_folders", "true".to_owned()),
        ];
        Ok(self.call(method::BOOKMARKS_LIST, params).await?.data)
    }

    /// `bookmarks.add` of a `link` bookmark to the conversation's bar (or inside `parent_id`).
    pub async fn bookmarks_add_link(
        &self,
        channel: &ChannelId,
        title: &str,
        link: &str,
    ) -> Result<()> {
        self.bookmarks_add_link_in_folder(channel, title, link, None)
            .await
    }

    /// `bookmarks.add` of a `link` bookmark optionally placed inside a folder (`parent_id`).
    pub async fn bookmarks_add_link_in_folder(
        &self,
        channel: &ChannelId,
        title: &str,
        link: &str,
        parent_id: Option<&str>,
    ) -> Result<()> {
        let mut params = vec![
            ("channel_id", channel.to_string()),
            ("title", title.to_owned()),
            ("type", "link".to_owned()),
            ("link", link.to_owned()),
        ];
        if let Some(parent) = parent_id.filter(|s| !s.is_empty()) {
            params.push(("parent_id", parent.to_owned()));
        }
        self.call_ok(method::BOOKMARKS_ADD, params).await
    }

    /// `bookmarks.remove`: delete a bookmark from `channel` (`F-CORE-062`).
    pub async fn bookmarks_remove(&self, channel: &ChannelId, bookmark_id: &str) -> Result<()> {
        let params = vec![
            ("channel_id", channel.to_string()),
            ("bookmark_id", bookmark_id.to_owned()),
        ];
        self.call_ok(method::BOOKMARKS_REMOVE, params).await
    }

    /// Raw `drafts.list` response (`is_active=true`, `limit=100`).
    pub(super) async fn drafts_list_raw(&self) -> Result<Value> {
        let params = vec![
            ("limit", "100".to_owned()),
            ("is_active", "true".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        Ok(self.call(method::DRAFTS_LIST, params).await?.data)
    }

    /// `drafts.list`: fetch the signed-in user's unsent cloud-synced drafts (`F-CORE-089`).
    pub async fn drafts_list(&self) -> Result<Vec<CloudDraft>> {
        let raw = self.drafts_list_raw().await?;
        Ok(parse_cloud_drafts(&raw))
    }

    /// `drafts.list`: fetch pending scheduled messages (`date_scheduled > 0`), used by Slack's
    /// web client on `xoxc-` tokens where `chat.scheduledMessages.list` returns
    /// `not_allowed_token_type`.
    pub(super) async fn drafts_list_scheduled(
        &self,
        channel: Option<&ChannelId>,
    ) -> Result<Vec<ScheduledMessage>> {
        let raw = self.drafts_list_raw().await?;
        Ok(parse_scheduled_drafts(&raw, channel))
    }

    /// `drafts.create` with `date_scheduled`: schedule a message the way Slack's web client does
    /// on `xoxc-` sessions (where public `chat.scheduleMessage` returns `not_allowed_token_type`).
    pub(super) async fn drafts_create_scheduled(
        &self,
        channel: &ChannelId,
        post_at: u64,
        content: &MessageContent,
        thread_ts: Option<&Ts>,
    ) -> Result<ScheduledMessageCreated> {
        let blocks_json = if !content.blocks.is_empty() {
            serde_json::to_string(&content.blocks)?
        } else {
            serde_json::to_string(&[Block::from(RichTextBlock::plain_text(content.text.clone()))])?
        };
        let mut dest = serde_json::json!({
            "channel_id": channel.as_str(),
            "broadcast": false,
        });
        if let Some(ts) = thread_ts
            && let Some(obj) = dest.as_object_mut()
        {
            obj.insert("thread_ts".to_owned(), Value::String(ts.to_string()));
        }
        let destinations = Value::Array(vec![dest]).to_string();
        let params = vec![
            ("blocks", blocks_json),
            ("destinations", destinations),
            ("file_ids", "[]".to_owned()),
            ("client_msg_id", uuid::Uuid::new_v4().to_string()),
            ("is_from_composer", "true".to_owned()),
            ("date_scheduled", post_at.to_string()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        let raw: Value = self.call(method::DRAFTS_CREATE, params).await?.data;
        let draft = raw.get("draft").unwrap_or(&raw);
        let id = draft
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let scheduled_at = draft
            .get("date_scheduled")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .filter(|&ts| ts > 0)
            .unwrap_or(post_at);
        Ok(ScheduledMessageCreated {
            scheduled_message_id: id,
            channel: channel.clone(),
            post_at: scheduled_at,
        })
    }

    /// `drafts.create`: create a cloud-synced draft for `channel` and return its draft id.
    pub async fn drafts_create(&self, channel: &ChannelId, text: &str) -> Result<Option<String>> {
        let blocks =
            serde_json::to_string(&[Block::from(RichTextBlock::plain_text(text.to_owned()))])?;
        let destinations = serde_json::json!([{ "channel_id": channel.as_str() }]);
        let params = vec![
            ("blocks", blocks),
            ("destinations", destinations.to_string()),
            ("file_ids", "[]".to_owned()),
            ("client_msg_id", uuid::Uuid::new_v4().to_string()),
            ("is_from_composer", "true".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        let raw: Value = self.call(method::DRAFTS_CREATE, params).await?.data;
        Ok(raw
            .get("draft")
            .and_then(|d| d.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned))
    }

    /// `drafts.update`: update an existing cloud draft's text.
    pub async fn drafts_update(
        &self,
        draft_id: &str,
        channel: &ChannelId,
        text: &str,
    ) -> Result<()> {
        let blocks =
            serde_json::to_string(&[Block::from(RichTextBlock::plain_text(text.to_owned()))])?;
        let destinations = serde_json::json!([{ "channel_id": channel.as_str() }]);
        let mut params = vec![
            ("draft_id", draft_id.to_owned()),
            ("blocks", blocks),
            ("destinations", destinations.to_string()),
            ("file_ids", "[]".to_owned()),
            ("client_msg_id", uuid::Uuid::new_v4().to_string()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Ok(raw) = self.drafts_list_raw().await
            && let Some(ts) = find_draft_last_updated_ts(&raw, draft_id)
        {
            params.push(("client_last_updated_ts", ts));
        }
        self.call_ok(method::DRAFTS_UPDATE, params).await
    }

    /// `drafts.delete`: remove a cloud draft (or cancel a scheduled draft), attaching the
    /// 7-decimal padded `client_last_updated_ts` when available in `drafts.list`.
    pub async fn drafts_delete(&self, draft_id: &str) -> Result<()> {
        let mut params = vec![
            ("draft_id", draft_id.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Ok(raw) = self.drafts_list_raw().await
            && let Some(ts) = find_draft_last_updated_ts(&raw, draft_id)
        {
            params.push(("client_last_updated_ts", ts));
        }
        self.call_ok(method::DRAFTS_DELETE, params).await
    }

    /// One page of `files.list`: the files matching `filter`, newest first. A `channel`
    /// scopes the call to a conversation; without it Slack answers every file the
    /// signed-in user can see workspace-wide. `files.list` paginates by `page`, so a
    /// single large first page covers both the Files & links tab and the Files view.
    pub async fn files_list(
        &self,
        filter: &FilesFilter,
        count: u32,
        page_no: u32,
    ) -> Result<Page<File>> {
        let mut params = Vec::new();
        if let Some(channel) = &filter.channel {
            params.push(("channel", channel.to_string()));
        }
        if let Some(user) = &filter.user {
            params.push(("user", user.to_string()));
        }
        if let Some(types) = &filter.types {
            params.push(("types", types.clone()));
        }
        params.push(("count", count.to_string()));
        params.push(("page", page_no.to_string()));
        let resp = self.call::<FilesWire>(method::FILES_LIST, params).await?;
        Ok(page(resp, |w| w.files))
    }

    /// `files.getUploadURLExternal`: reserve an upload slot for `filename` of `length` bytes.
    pub async fn files_get_upload_url_external(
        &self,
        filename: &str,
        length: usize,
    ) -> Result<UploadSlot> {
        let params = vec![
            ("filename", filename.to_owned()),
            ("length", length.to_string()),
        ];
        Ok(self
            .call::<UploadSlot>(method::FILES_GET_UPLOAD_U_R_L_EXTERNAL, params)
            .await?
            .data)
    }

    /// `files.completeUploadExternal`: finalize uploaded file IDs and share them to `channel`
    /// (optionally inside `thread_ts` and accompanied by `content`).
    pub async fn files_complete_upload_external(
        &self,
        files: &[(String, String)],
        channel: &ChannelId,
        thread_ts: Option<&Ts>,
        content: Option<&MessageContent>,
    ) -> Result<Vec<File>> {
        let entries: Vec<Value> = files
            .iter()
            .map(|(id, title)| serde_json::json!({ "id": id, "title": title }))
            .collect();
        let mut params = vec![
            ("files", serde_json::to_string(&entries)?),
            ("channel_id", channel.to_string()),
        ];
        if let Some(ts) = thread_ts {
            params.push(("thread_ts", ts.to_string()));
        }
        if let Some(content) = content {
            if !content.blocks.is_empty() {
                params.push(("blocks", serde_json::to_string(&content.blocks)?));
            } else if !content.text.trim().is_empty() {
                params.push(("initial_comment", content.text.clone()));
            }
        }
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default, deserialize_with = "lenient_vec")]
            files: Vec<File>,
        }
        Ok(self
            .call::<Wire>(method::FILES_COMPLETE_UPLOAD_EXTERNAL, params)
            .await?
            .data
            .files)
    }

    /// Full external upload v2 flow: reserve a slot per file, POST its raw bytes, and finalize
    /// all files in one `files.completeUploadExternal` call into `channel`.
    pub async fn upload_files(
        &self,
        channel: &ChannelId,
        thread_ts: Option<&Ts>,
        content: Option<&MessageContent>,
        files: &[UploadFile],
    ) -> Result<Vec<File>> {
        let mut completed = Vec::with_capacity(files.len());
        for file in files {
            if file.bytes.is_empty() {
                return Err(crate::ApiError::Slack("empty_file".to_owned()));
            }
            let slot = self
                .files_get_upload_url_external(&file.filename, file.bytes.len())
                .await?;
            self.post_upload_bytes(&slot.upload_url, file.bytes.clone())
                .await?;
            completed.push((slot.file_id, file.filename.clone()));
        }
        self.files_complete_upload_external(&completed, channel, thread_ts, content)
            .await
    }

    /// `conversations.canvases.create`: creates the conversation's empty channel canvas and
    /// returns its file id. Fails with `channel_canvas_already_exists` when one exists
    /// already (read the id from `conversations.info` -> `properties.canvas` then).
    pub async fn conversations_canvases_create(
        &self,
        channel: &ChannelId,
    ) -> Result<CanvasCreated> {
        let params = vec![("channel_id", channel.to_string())];
        Ok(self
            .call(method::CONVERSATIONS_CANVASES_CREATE, params)
            .await?
            .data)
    }

    /// `activity.feed`, page one (the view's first `limit` items, newest first; the next
    /// page's cursor rides in [`ActivityFeed::next_cursor`]). Row bodies arrive separately
    /// through [`SlackApi::messages_list`] when the feed itself carried none.
    pub async fn activity_feed(&self, limit: u32) -> Result<ActivityFeed> {
        let params = vec![
            ("types", ACTIVITY_TYPES.to_owned()),
            ("mode", "chrono_v1".to_owned()),
            ("limit", limit.to_string()),
            ("is_activity_inbox", "true".to_owned()),
        ];
        let resp = self
            .call::<ActivityFeedWire>(method::ACTIVITY_FEED, params)
            .await?;
        let next_cursor = resp.next_cursor().map(str::to_owned);
        Ok(ActivityFeed {
            items: resp.data.items.into_iter().filter_map(row).collect(),
            next_cursor,
        })
    }

    /// `messages.list`: the bodies of the given messages, as the web client fills the
    /// Activity feed's previews. At most [`MESSAGES_LIST_CHANNELS`] channels per call (a
    /// workspace that accepted 9 rejected 39), so wider asks are batched here.
    pub async fn messages_list(
        &self,
        ids: &[(ChannelId, Vec<Ts>)],
    ) -> Result<HashMap<ChannelId, Vec<Message>>> {
        let mut bodies = HashMap::new();
        for batch in ids.chunks(MESSAGES_LIST_CHANNELS) {
            let message_ids: Vec<Value> = batch
                .iter()
                .map(|(channel, stamps)| {
                    Value::Object(Map::from_iter([
                        (
                            "channel".to_owned(),
                            Value::String(channel.as_str().to_owned()),
                        ),
                        (
                            "timestamps".to_owned(),
                            Value::Array(
                                stamps
                                    .iter()
                                    .map(|ts| Value::String(ts.as_str().to_owned()))
                                    .collect(),
                            ),
                        ),
                    ]))
                })
                .collect();
            let params = vec![
                ("message_ids", serde_json::to_string(&message_ids)?),
                ("org_wide_aware", "true".to_owned()),
                ("cached_latest_updates", "{}".to_owned()),
            ];
            let resp = self
                .call::<MessagesListWire>(method::MESSAGES_LIST, params)
                .await?;
            for (channel, entry) in resp.data.messages_data {
                bodies.insert(ChannelId::from(channel.as_str()), entry.messages);
            }
        }
        Ok(bodies)
    }

    /// `activity.markRead`: marks feed entries read (each item's handles, see
    /// [`ActivityItem::entry`]).
    pub async fn activity_mark_read(&self, entries: &[ActivityEntry]) -> Result<()> {
        self.activity_mark(method::ACTIVITY_MARK_READ, entries)
            .await
    }

    /// `activity.markUnread`: marks feed entries unread.
    pub async fn activity_mark_unread(&self, entries: &[ActivityEntry]) -> Result<()> {
        self.activity_mark(method::ACTIVITY_MARK_UNREAD, entries)
            .await
    }

    async fn activity_mark(&self, method: &str, entries: &[ActivityEntry]) -> Result<()> {
        let params = vec![
            ("entries", serde_json::to_string(entries)?),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call_ok(method, params).await
    }
    /// Featured/trending GIFs for Slack's emoji/GIF picker (`gif.featured`).
    pub async fn gif_featured(&self, limit: u32) -> Result<Vec<GifItem>> {
        let resp: ApiResponse<GifListData> = self
            .call(method::GIF_FEATURED, vec![("limit", limit.to_string())])
            .await?;
        Ok(parse_gif_list(resp.data))
    }

    /// Search GIFs for Slack's emoji/GIF picker (`gif.search`).
    pub async fn gif_search(&self, query: &str, limit: u32) -> Result<Vec<GifItem>> {
        let resp: ApiResponse<GifListData> = self
            .call(
                method::GIF_SEARCH,
                vec![("query", query.to_owned()), ("limit", limit.to_string())],
            )
            .await?;
        Ok(parse_gif_list(resp.data))
    }

    /// `saved.list`: items saved for Later under `filter` (`"saved"`, `"completed"`, `"archived"`).
    pub async fn saved_list(&self, filter: &str, limit: u32) -> Result<Vec<SavedItem>> {
        let resp: ApiResponse<SavedListWire> = self
            .call(
                method::SAVED_LIST,
                vec![
                    ("filter", filter.to_owned()),
                    ("limit", limit.to_string()),
                    ("include_tombstones", "true".to_owned()),
                    ("_x_reason", "saved-api/savedList".to_owned()),
                    ("_x_mode", MODE_ONLINE.to_owned()),
                ],
            )
            .await?;
        Ok(parse_saved_list(resp.data, filter))
    }

    /// `saved.add`: save a message for Later, optionally with a reminder due time (unix seconds).
    pub async fn saved_add(
        &self,
        channel: &ChannelId,
        ts: &Ts,
        date_due: Option<u64>,
    ) -> Result<()> {
        let mut params = vec![
            ("item_type", "message".to_owned()),
            ("item_id", channel.to_string()),
            ("ts", ts.to_string()),
            ("_x_reason", "saved-api/addSavedMessage".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(due) = date_due {
            params.push(("date_due", due.to_string()));
        }
        self.call_ok(method::SAVED_ADD, params).await
    }

    /// `saved.add`: save a file for Later (`item_type=file`, `F-CORE-157`).
    pub async fn saved_add_file(&self, file_id: &str, date_due: Option<u64>) -> Result<()> {
        let mut params = vec![
            ("item_type", "file".to_owned()),
            ("item_id", file_id.to_owned()),
            ("_x_reason", "saved-api/addSavedFile".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(due) = date_due {
            params.push(("date_due", due.to_string()));
        }
        self.call_ok(method::SAVED_ADD, params).await
    }

    /// `files.info`: fetch metadata for one file or canvas (`F-COLLAB-125`).
    pub async fn files_info(&self, file_id: &str) -> Result<File> {
        #[derive(serde::Deserialize)]
        struct Resp {
            file: File,
        }
        let params = vec![("file", file_id.to_owned())];
        Ok(self
            .call::<Resp>(method::FILES_INFO, params)
            .await?
            .data
            .file)
    }

    /// `files.delete`: permanently delete a file/clip uploaded by the user (`F-CORE-130`).
    pub async fn files_delete(&self, file_id: &str) -> Result<()> {
        let params = vec![("file", file_id.to_owned())];
        self.call_ok(method::FILES_DELETE, params).await
    }

    /// Fetch a live Slack canvas's parsed metadata and HTML body (`files.info` -> `url_private`
    /// export, falling back to `canvases.getContent(content_type="html")` if `url_private` is
    /// absent).
    pub async fn fetch_canvas_with_meta(&self, file_id: &str) -> Result<(CanvasMeta, String)> {
        let file = self.files_info(file_id).await?;
        let meta = parse_canvas_meta_from_file(&file, None);
        let html = if let Some(url) = file
            .url_private
            .as_deref()
            .or(file.url_private_download.as_deref())
        {
            self.download_html_file(url).await?
        } else {
            match self.canvases_get_content(file_id, Some("html")).await {
                Ok(html) => html,
                Err(err) if err.is_auth_expired() => return Err(err),
                Err(_) => return Err(crate::ApiError::Slack("canvas_no_private_url".to_owned())),
            }
        };
        Ok((meta, html))
    }

    /// Fetch a live Slack canvas's read-only HTML export (`files.info` -> `url_private` with
    /// session cookie, `F-COLLAB-125`). Returns `(title, html)`.
    pub async fn fetch_canvas_html(&self, file_id: &str) -> Result<(String, String)> {
        let (meta, html) = self.fetch_canvas_with_meta(file_id).await?;
        Ok((meta.title, html))
    }

    /// `saved.update`: transition a saved item's state (`mark`) and/or update or clear (`Some(None)`)
    /// its reminder due time (`date_due`).
    pub async fn saved_update(
        &self,
        channel: &ChannelId,
        ts: &Ts,
        mark: Option<SavedMark>,
        date_due: Option<Option<u64>>,
    ) -> Result<()> {
        let mut params = vec![
            ("item_type", "message".to_owned()),
            ("item_id", channel.to_string()),
            ("ts", ts.to_string()),
            ("_x_reason", "saved-api/updateSavedMessage".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(mark) = mark {
            params.push(("mark", mark.as_str().to_owned()));
        }
        if let Some(due) = date_due {
            params.push(("date_due", due.unwrap_or(0).to_string()));
        }
        self.call_ok(method::SAVED_UPDATE, params).await
    }

    /// `saved.delete`: remove a message from Later.
    pub async fn saved_delete(&self, channel: &ChannelId, ts: &Ts) -> Result<()> {
        let params = vec![
            ("item_type", "message".to_owned()),
            ("item_id", channel.to_string()),
            ("ts", ts.to_string()),
            ("_x_reason", "saved-api/deleteSavedMessage".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call_ok(method::SAVED_DELETE, params).await
    }

    /// `saved.clearCompleted`: clear all completed Later items.
    pub async fn saved_clear_completed(&self) -> Result<()> {
        let params = vec![
            ("_x_reason", "manually_marked_all_completed".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call_ok(method::SAVED_CLEAR_COMPLETED, params).await
    }

    /// `client.appCommands`: workspace slash command catalog (`core`, `app`, `service`).
    pub async fn client_app_commands(&self) -> Result<Vec<AppCommand>> {
        let params = vec![
            ("_x_reason", "dispatch-client-extras".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        let resp: ApiResponse<AppCommandsWire> =
            self.call(method::CLIENT_APP_COMMANDS, params).await?;
        Ok(parse_app_commands(resp.data))
    }

    /// Edge API `users/search`: fast typeahead lookup across workspace & org people for Cmd+K.
    pub async fn edge_users_search(
        &self,
        team_id: &TeamId,
        query: &str,
        count: u32,
    ) -> Result<Vec<User>> {
        let payload = serde_json::json!({
            "query": query,
            "count": count,
            "fuzz": 1,
            "include_profile_only_users": true,
            "enable_workspace_ranking": true,
            "search_email": true,
            "filter": "NOT deactivated",
            "default_workspace": team_id.as_str(),
        });
        let resp: ApiResponse<EdgeUsersWire> = self
            .call_edge_json(team_id.as_str(), "users/search", payload)
            .await?;
        Ok(resp.data.results)
    }

    /// Edge API `channels/search`: fast typeahead lookup across joined and unjoined channels for Cmd+K.
    pub async fn edge_channels_search(
        &self,
        team_id: &TeamId,
        query: &str,
        count: u32,
    ) -> Result<Vec<Conversation>> {
        let payload = serde_json::json!({
            "query": query,
            "count": count,
            "fuzz": 1,
            "filter": "xws",
            "include_record_channels": false,
            "check_membership": true,
            "default_workspace": team_id.as_str(),
        });
        let resp: ApiResponse<EdgeChannelsWire> = self
            .call_edge_json(team_id.as_str(), "channels/search", payload)
            .await?;
        let members: std::collections::HashSet<&ChannelId> =
            resp.data.member_channels.iter().collect();
        let mut results = resp.data.results;
        for ch in &mut results {
            if members.contains(&ch.id) {
                ch.is_member = true;
            }
        }
        Ok(results)
    }

    fn search_module_params(
        module: &str,
        query: &str,
        sort: SearchSort,
        page: usize,
        search_context: &str,
        reason: &str,
        extra: &[(&'static str, &'static str)],
    ) -> crate::client::Params {
        let sort_key = match sort {
            SearchSort::Score => "score",
            SearchSort::Timestamp => "timestamp",
        };
        let mut params = vec![
            ("module", module.to_owned()),
            ("query", query.to_owned()),
            ("page", page.max(1).to_string()),
            ("count", "20".to_owned()),
            ("extracts", "1".to_owned()),
            ("highlight", "0".to_owned()),
            ("extra_message_data", "1".to_owned()),
            ("file_title_only", "false".to_owned()),
            ("query_rewrite_disabled", "false".to_owned()),
            ("include_files_shares", "1".to_owned()),
            ("search_context", search_context.to_owned()),
            ("sort", sort_key.to_owned()),
            ("sort_dir", "desc".to_owned()),
            ("search_tab_filter", module.to_owned()),
            ("search_tab_sort", sort_key.to_owned()),
            ("_x_reason", reason.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
            ("_x_app_name", "client".to_owned()),
        ];
        for &(k, v) in extra {
            params.push((k, v.to_owned()));
        }
        params
    }

    /// `search.modules.messages`: private web-client message search (`docs/research/api-search.md`).
    pub async fn search_modules_messages(
        &self,
        query: &str,
        sort: SearchSort,
        page: usize,
    ) -> Result<RemoteMessagePage> {
        let params = Self::search_module_params(
            "messages",
            query,
            sort,
            page,
            "desktop_messages_tab",
            "search-modules-messages",
            &[("max_extract_len", "200"), ("no_user_profile", "1")],
        );
        let wire: SearchModulesMessagesWire = self
            .call(method::SEARCH_MODULES_MESSAGES, params)
            .await?
            .data;
        Ok(parse_search_modules_messages(wire))
    }

    /// `search.modules.files`: private web-client file search (`docs/research/api-search.md`).
    pub async fn search_modules_files(
        &self,
        query: &str,
        sort: SearchSort,
        page: usize,
    ) -> Result<RemoteFilePage> {
        let params = Self::search_module_params(
            "files",
            query,
            sort,
            page,
            "desktop_files_tab",
            "search-modules-files",
            &[],
        );
        let wire: SearchModulesFilesWire =
            self.call(method::SEARCH_MODULES_FILES, params).await?.data;
        Ok(parse_search_modules_files(wire))
    }

    /// `users.profile.set`: sparse update to the caller's profile or custom status.
    pub async fn users_profile_set(
        &self,
        update: &ProfileUpdate,
    ) -> Result<slackless_types::Profile> {
        let profile_json = serde_json::to_string(update)?;
        let params = vec![
            ("profile", profile_json),
            ("_x_reason", "profile-edit".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        let resp: ProfileSetWire = self.call(method::USERS_PROFILE_SET, params).await?.data;
        Ok(resp.profile)
    }

    /// `presence.set`: private web-client manual presence toggle (`"active"` or `"away"`).
    pub async fn presence_set(&self, away: bool) -> Result<()> {
        let presence = if away { "away" } else { "active" };
        let params = vec![
            ("presence", presence.to_owned()),
            ("_x_reason", "manual-presence".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call_ok(method::PRESENCE_SET, params).await
    }

    /// `users.prefs.set`: write a single user preference key/value pair back to Slack.
    pub async fn users_prefs_set(&self, name: &str, value: &str) -> Result<()> {
        let params = vec![
            ("name", name.to_owned()),
            ("value", value.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call_ok(method::USERS_PREFS_SET, params).await
    }

    /// `users.prefs.setNotifications` (`global=true`): write one global notification pref
    /// (`global_desktop`, `global_mobile`, `global_keywords`, `threads_everything`,
    /// `no_text_in_notifications`, `push_idle_wait`, etc.; `F-CORE-147`, `F-CORE-155`).
    pub async fn users_prefs_set_global_notification(
        &self,
        name: &str,
        value: &str,
        sync: bool,
    ) -> Result<()> {
        let params = vec![
            ("global", "true".to_owned()),
            ("name", name.to_owned()),
            ("value", value.to_owned()),
            ("sync", sync.to_string()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call_ok(method::USERS_PREFS_SET_NOTIFICATIONS, params)
            .await
    }
}
