//! Search, GIFs, canvases, lists, files/uploads/favorites/collections/restore, app home,
//! workflows, modals, and block suggestions command handlers.

use super::super::*;
use crate::api::{
    ActionContainer, CanvasAccessLevel, CanvasEditOp, FilesFilter, ListCellInput, MessageContent,
    SearchSort, UploadFile,
};

impl<B: Backend> Core<B> {
    pub(super) fn search(&self, query: String) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("search", async move {
            let parsed = SearchQuery::parse(&query);
            let messages =
                with_store(&store, move |s| s.search(&ws, &parsed, SEARCH_LIMIT)).await?;
            events.emit(Event::SearchResults {
                query: query.clone(),
                messages,
            });
            let results = Box::new(backend.search(query.clone()).await?);
            events.emit(Event::RemoteSearchResults { query, results });
            Ok(())
        });
    }

    pub(super) fn quick_switch_search(&self, query: String) {
        let backend = self.backend.clone();
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("edgeapi.search", async move {
            let trimmed = query.trim().to_owned();
            if trimmed.is_empty() {
                return Ok(());
            }
            let results = backend.edge_search(ws, trimmed).await?;
            events.emit(Event::QuickSwitchResults {
                query,
                results: Box::new(results),
            });
            Ok(())
        });
    }

    pub(super) fn search_remote_messages(&self, query: String, sort: SearchSort, page: usize) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("search.modules.messages", async move {
            let res = backend
                .search_remote_messages(query.clone(), sort, page)
                .await;
            let emit = match res {
                Ok(data) => Ok(Box::new(data)),
                Err(e) => Err(e.to_string()),
            };
            events.emit(Event::RemoteMessagesPage {
                query,
                sort,
                page,
                result: emit,
            });
            Ok(())
        });
    }

    pub(super) fn search_remote_files(&self, query: String, sort: SearchSort, page: usize) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("search.modules.files", async move {
            let res = backend.search_remote_files(query.clone(), sort, page).await;
            let emit = match res {
                Ok(data) => Ok(Box::new(data)),
                Err(e) => Err(e.to_string()),
            };
            events.emit(Event::RemoteFilesPage {
                query,
                sort,
                page,
                result: emit,
            });
            Ok(())
        });
    }

    pub(super) fn search_gifs(&self, query: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        let method = if query.trim().is_empty() {
            "gif.featured"
        } else {
            "gif.search"
        };
        self.spawn(method, async move {
            let trimmed = query.trim().to_owned();
            let gifs = if trimmed.is_empty() {
                backend.gif_featured(24).await?
            } else {
                backend.gif_search(trimmed.clone(), 24).await?
            };
            events.emit(Event::GifResults { query, gifs });
            Ok(())
        });
    }

    pub(super) fn load_files(&self, filter: FilesFilter) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("files.list", async move {
            let files = backend.files(filter.clone()).await;
            answer_load(&events, files, |files| Event::FilesLoaded { filter, files })
        });
    }

    pub(super) fn upload_files(
        &self,
        channel: ChannelId,
        thread_ts: Option<Ts>,
        content: Option<MessageContent>,
        files: Vec<UploadFile>,
    ) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("files.upload", async move {
            let uploaded = backend
                .upload_files(channel.clone(), thread_ts, content, files)
                .await?;
            events.emit(Event::FilesUploaded {
                channel,
                count: uploaded.len(),
            });
            Ok(())
        });
    }

    pub(super) fn delete_file(&self, file_id: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("files.delete", async move {
            backend.delete_file(file_id.clone()).await?;
            events.emit(Event::FileDeleted { file_id });
            Ok(())
        });
    }

    pub(super) fn request_file_permission(&self, file_id: String, request_type: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("files.requestPermission", async move {
            backend
                .files_request_permission(file_id.clone(), request_type)
                .await?;
            events.emit(Event::FilePermissionRequested { file_id });
            Ok(())
        });
    }

    pub(super) fn load_files_extras(&self) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("files.extras", async move {
            reload_files_extras(&backend, &events).await;
            Ok(())
        });
    }

    pub(super) fn set_file_favorite(
        &self,
        file_id: String,
        collection_id: Option<String>,
        starred: bool,
    ) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("files.favorites", async move {
            backend
                .files_favorites_set(file_id, collection_id, starred)
                .await?;
            reload_files_extras(&backend, &events).await;
            Ok(())
        });
    }

    pub(super) fn create_file_collection(&self, name: String, emoji: Option<String>) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("files.collections.create", async move {
            let _ = backend.files_collections_create(name, emoji).await?;
            reload_files_extras(&backend, &events).await;
            Ok(())
        });
    }

    pub(super) fn delete_file_collection(&self, collection_id: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("files.collections.delete", async move {
            backend.files_collections_delete(collection_id).await?;
            reload_files_extras(&backend, &events).await;
            Ok(())
        });
    }

    pub(super) fn restore_deleted_file(&self, file_id: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("files.undelete", async move {
            backend.files_undelete(file_id.clone()).await?;
            events.emit(Event::FileRestored { file_id });
            reload_files_extras(&backend, &events).await;
            Ok(())
        });
    }

    pub(super) fn create_channel_canvas(&self, channel: ChannelId) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("conversations.canvases.create", async move {
            let canvas_id = backend.create_canvas(channel.clone()).await?;
            // Refresh the stored conversation so `properties.canvas` lands; the tab
            // row re-derives from the next snapshot.
            let fresh = backend.conversation_info(channel.clone()).await?;
            with_store(&store, move |s| s.merge_conversation(&ws, &fresh)).await?;
            events.emit(Event::Conversations(vec![channel.clone()]));
            events.emit(Event::ChannelCanvasCreated { channel, canvas_id });
            Ok(())
        });
    }

    pub(super) fn load_canvas(&self, file_id: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("files.info(canvas)", async move {
            emit_canvas_html(&backend, &events, file_id).await;
            Ok(())
        });
    }

    pub(super) fn create_canvas(
        &self,
        title: String,
        markdown: String,
        channel: Option<ChannelId>,
    ) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("canvases.create", async move {
            let created = backend
                .canvases_create(
                    Some(title.clone()),
                    (!markdown.is_empty()).then_some(markdown),
                    channel.clone(),
                )
                .await?;
            events.emit(Event::CanvasCreated {
                file_id: created.canvas_id,
                title,
                channel,
            });
            Ok(())
        });
    }

    pub(super) fn edit_canvas(&self, file_id: String, op: CanvasEditOp) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("canvases.edit", async move {
            backend.canvases_edit(file_id.clone(), op).await?;
            emit_canvas_html(&backend, &events, file_id).await;
            Ok(())
        });
    }

    pub(super) fn delete_canvas(&self, canvas_id: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("canvases.delete", async move {
            let result = match backend.canvases_delete(canvas_id.clone()).await {
                Ok(()) => Ok(()),
                Err(err) => {
                    if let ApiError::AuthExpired(ref code) = err {
                        events.emit(Event::AuthExpired(code.clone()));
                    }
                    Err(format_canvas_api_error(&err))
                }
            };
            events.emit(Event::CanvasDeleted { canvas_id, result });
            Ok(())
        });
    }

    pub(super) fn set_canvas_access(
        &self,
        canvas_id: String,
        access_level: CanvasAccessLevel,
        channel_ids: Vec<ChannelId>,
        user_ids: Vec<UserId>,
        post_permalink: Option<String>,
    ) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("canvases.access.set", async move {
            let summary = if !channel_ids.is_empty() {
                format!(
                    "Granted {} access to {} channel(s)",
                    access_level.as_str(),
                    channel_ids.len()
                )
            } else if !user_ids.is_empty() {
                format!(
                    "Granted {} access to {} user(s)",
                    access_level.as_str(),
                    user_ids.len()
                )
            } else {
                format!("Updated {} access", access_level.as_str())
            };
            let result = match backend
                .canvases_access_set(
                    canvas_id.clone(),
                    access_level,
                    channel_ids.clone(),
                    user_ids,
                )
                .await
            {
                Ok(()) => {
                    if let Some(url) = post_permalink.filter(|u| !u.trim().is_empty())
                        && !channel_ids.is_empty()
                    {
                        for channel in &channel_ids {
                            let _ = backend
                                .post_message(
                                    channel.clone(),
                                    MessageContent::plain(url.clone()),
                                    None,
                                    false,
                                )
                                .await;
                        }
                    }
                    Ok(())
                }
                Err(err) => {
                    if let ApiError::AuthExpired(ref code) = err {
                        events.emit(Event::AuthExpired(code.clone()));
                    }
                    Err(format_canvas_api_error(&err))
                }
            };
            events.emit(Event::CanvasAccessUpdated {
                canvas_id,
                summary,
                result,
            });
            Ok(())
        });
    }

    pub(super) fn delete_canvas_access(
        &self,
        canvas_id: String,
        channel_ids: Vec<ChannelId>,
        user_ids: Vec<UserId>,
    ) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("canvases.access.delete", async move {
            let summary = if !channel_ids.is_empty() {
                format!("Revoked access for {} channel(s)", channel_ids.len())
            } else if !user_ids.is_empty() {
                format!("Revoked access for {} user(s)", user_ids.len())
            } else {
                "Revoked access".to_owned()
            };
            let result = match backend
                .canvases_access_delete(canvas_id.clone(), channel_ids, user_ids)
                .await
            {
                Ok(()) => Ok(()),
                Err(err) => {
                    if let ApiError::AuthExpired(ref code) = err {
                        events.emit(Event::AuthExpired(code.clone()));
                    }
                    Err(format_canvas_api_error(&err))
                }
            };
            events.emit(Event::CanvasAccessUpdated {
                canvas_id,
                summary,
                result,
            });
            Ok(())
        });
    }

    pub(super) fn load_canvas_comments(&self, canvas_id: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("canvas.comments.list", async move {
            let result = match backend.canvas_comments_list(canvas_id.clone()).await {
                Ok(threads) => Ok(threads),
                Err(err) => {
                    if let ApiError::AuthExpired(ref code) = err {
                        events.emit(Event::AuthExpired(code.clone()));
                    }
                    Err(format_canvas_api_error(&err))
                }
            };
            events.emit(Event::CanvasCommentsLoaded { canvas_id, result });
            Ok(())
        });
    }

    pub(super) fn post_canvas_comment(
        &self,
        canvas_id: String,
        text: String,
        thread_ts: Option<Ts>,
    ) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("canvas.comment.post", async move {
            let result = match backend
                .canvas_comment_post(canvas_id.clone(), text, thread_ts)
                .await
            {
                Ok(_) => Ok(()),
                Err(err) => {
                    if let ApiError::AuthExpired(ref code) = err {
                        events.emit(Event::AuthExpired(code.clone()));
                    }
                    Err(format_canvas_api_error(&err))
                }
            };
            events.emit(Event::CanvasCommentPosted { canvas_id, result });
            Ok(())
        });
    }

    pub(super) fn resolve_canvas_comment(&self, canvas_id: String, thread_ts: Ts, resolved: bool) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        let context = if resolved {
            "quip.thread.archive"
        } else {
            "quip.thread.unarchive"
        };
        self.spawn(context, async move {
            let channel_id = crate::api::canvas_comment_channel_id(&canvas_id);
            let res = if resolved {
                backend
                    .quip_thread_archive(channel_id, thread_ts.clone())
                    .await
            } else {
                backend
                    .quip_thread_unarchive(channel_id, thread_ts.clone())
                    .await
            };
            let result = match res {
                Ok(()) => Ok(()),
                Err(err) => {
                    if let ApiError::AuthExpired(ref code) = err {
                        events.emit(Event::AuthExpired(code.clone()));
                    }
                    Err(format_canvas_api_error(&err))
                }
            };
            events.emit(Event::CanvasCommentResolved {
                canvas_id,
                thread_ts,
                resolved,
                result,
            });
            Ok(())
        });
    }

    pub(super) fn resolve_all_canvas_comments(&self, canvas_id: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("quip.thread.archiveAll", async move {
            let channel_id = crate::api::canvas_comment_channel_id(&canvas_id);
            let result = match backend.quip_thread_archive_all(channel_id).await {
                Ok(()) => Ok(()),
                Err(err) => {
                    if let ApiError::AuthExpired(ref code) = err {
                        events.emit(Event::AuthExpired(code.clone()));
                    }
                    Err(format_canvas_api_error(&err))
                }
            };
            events.emit(Event::CanvasCommentResolved {
                canvas_id,
                thread_ts: Ts::default(),
                resolved: true,
                result,
            });
            Ok(())
        });
    }

    pub(super) fn load_list(&self, file_id: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("lists.records.list", async move {
            emit_list(&backend, &events, file_id).await;
            Ok(())
        });
    }

    pub(super) fn create_list(
        &self,
        name: String,
        description: Option<String>,
        channel: Option<ChannelId>,
    ) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("lists.create", async move {
            let file_id = backend
                .lists_create(name.clone(), description, channel)
                .await?;
            events.emit(Event::ListCreated { file_id, name });
            Ok(())
        });
    }

    pub(super) fn create_list_item(&self, list_id: String, initial_fields: Vec<ListCellInput>) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("lists.records.create", async move {
            let _ = backend
                .lists_records_create(list_id.clone(), initial_fields)
                .await?;
            emit_list(&backend, &events, list_id).await;
            Ok(())
        });
    }

    pub(super) fn update_list_cell(
        &self,
        list_id: String,
        row_id: String,
        cells: Vec<ListCellInput>,
    ) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("lists.cells.update", async move {
            backend
                .lists_cells_update(list_id.clone(), row_id, cells)
                .await?;
            emit_list(&backend, &events, list_id).await;
            Ok(())
        });
    }

    pub(super) fn delete_list_item(&self, list_id: String, row_id: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("lists.records.delete", async move {
            backend
                .lists_records_delete(list_id.clone(), row_id)
                .await?;
            emit_list(&backend, &events, list_id).await;
            Ok(())
        });
    }

    pub(super) fn add_message_to_list(&self, list_id: String, channel: ChannelId, ts: Ts) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("lists.addToList", async move {
            let record_id = backend
                .lists_add_to_list(list_id.clone(), channel, ts)
                .await?;
            events.emit(Event::MessageAddedToList { list_id, record_id });
            Ok(())
        });
    }

    pub(super) fn archive_list_item(&self, list_id: String, row_id: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("lists.records.archive", async move {
            backend
                .lists_records_archive(list_id.clone(), row_id)
                .await?;
            emit_list(&backend, &events, list_id).await;
            Ok(())
        });
    }

    pub(super) fn set_list_access(
        &self,
        list_id: String,
        access_level: String,
        channel: Option<ChannelId>,
    ) {
        let backend = self.backend.clone();
        self.spawn("slackLists.access.set", async move {
            backend
                .lists_access_set(list_id, access_level, channel)
                .await?;
            Ok(())
        });
    }

    pub(super) fn fetch_list_record_comments(&self, list_id: String, record_id: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("lists.getThreadsForRecord", async move {
            let messages = backend
                .lists_get_threads_for_record(list_id.clone(), record_id.clone())
                .await
                .unwrap_or_default();
            events.emit(Event::ListRecordComments {
                list_id,
                record_id,
                messages,
            });
            Ok(())
        });
    }

    pub(super) fn load_app_home(&self, channel: ChannelId) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("conversations.info(app_home)", async move {
            let home = backend
                .load_app_home(channel.clone())
                .await
                .map(Box::new)
                .map_err(|e| e.to_string());
            events.emit(Event::AppHomeLoaded { channel, home });
            Ok(())
        });
    }

    pub(super) fn open_app_home(&self, app_id: String, team_id: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("apps.home.open", async move {
            if let Some(channel) = backend.open_app_home(app_id, team_id).await? {
                events.emit(Event::ConversationOpened(channel));
            }
            Ok(())
        });
    }

    pub(super) fn trip_workflow_trigger(&self, url: String, context: serde_json::Value) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("workflows.triggers.trip", async move {
            backend.trip_workflow_trigger(url.clone(), context).await?;
            events.emit(Event::WorkflowTripped { url });
            Ok(())
        });
    }

    pub(super) fn submit_modal(
        &self,
        view_id: String,
        hash: Option<String>,
        state_values: serde_json::Value,
    ) {
        let backend = self.backend.clone();
        self.spawn("views.submit", async move {
            backend.views_submit(view_id, hash, state_values).await?;
            Ok(())
        });
    }

    pub(super) fn close_modal(&self, view_id: String, hash: Option<String>) {
        let backend = self.backend.clone();
        self.spawn("views.close", async move {
            backend.views_close(view_id, hash).await?;
            Ok(())
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn block_suggestions(
        &self,
        key: String,
        bot: BotId,
        app_id: String,
        team: TeamId,
        action_id: String,
        block_id: String,
        value: String,
        container: ActionContainer,
    ) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("blocks.suggestions", async move {
            let options = backend
                .block_suggestions(bot, app_id, team, action_id, block_id, value, container)
                .await?;
            events.emit(Event::BlockSuggestions { key, options });
            Ok(())
        });
    }
}

fn format_canvas_api_error(err: &ApiError) -> String {
    match err {
        ApiError::Slack(code) | ApiError::AuthExpired(code) => {
            crate::api::format_canvas_error(code)
        }
        other => crate::api::format_canvas_error(&other.to_string()),
    }
}

async fn emit_canvas_html<B: Backend>(backend: &B, events: &Emitter, file_id: String) {
    match backend.fetch_canvas_with_meta(file_id.clone()).await {
        Ok((meta, html)) => {
            let title = meta.title.clone();
            events.emit(Event::CanvasLoaded {
                file_id: file_id.clone(),
                title,
                html: Ok(html),
            });
            events.emit(Event::CanvasMetaLoaded { file_id, meta });
        }
        Err(err) => {
            if let ApiError::AuthExpired(ref code) = err {
                events.emit(Event::AuthExpired(code.clone()));
            }
            events.emit(Event::CanvasLoaded {
                file_id,
                title: "Canvas".to_owned(),
                html: Err(format_canvas_api_error(&err)),
            });
        }
    }
}

async fn emit_list<B: Backend>(backend: &B, events: &Emitter, file_id: String) {
    let list = backend
        .fetch_list(file_id.clone())
        .await
        .map(Box::new)
        .map_err(|e| e.to_string());
    events.emit(Event::ListLoaded { file_id, list });
}

async fn reload_files_extras<B: Backend>(backend: &B, events: &Emitter) {
    let (favorites, collections, deleted) = tokio::join!(
        backend.files_favorites_list(),
        backend.files_collections_list(),
        backend.files_recently_deleted(),
    );
    events.emit(Event::FilesExtrasLoaded {
        favorites: favorites.unwrap_or_default(),
        collections: collections.unwrap_or_default(),
        deleted: deleted.unwrap_or_default(),
    });
}
