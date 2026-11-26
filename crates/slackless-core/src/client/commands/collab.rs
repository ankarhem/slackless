//! Search, GIFs, canvases, lists, files/uploads/favorites/collections/restore, app home,
//! workflows, modals, and block suggestions command handlers.

use super::super::*;
use crate::api::{
    self, ActionContainer, BlockSuggestionsRequest, CanvasAccessLevel, CanvasEditOp, FilesFilter,
    ListAccessLevel, ListCellInput, MessageContent, SearchSort, UploadFile,
};
use crate::domain::canvas::format_canvas_api_error;
use crate::domain::lists::format_list_api_error;

/// Owned parameters for [`Core::block_suggestions`], avoiding multi-argument sprawl.
pub(super) struct BlockSuggestionsParams {
    pub key: String,
    pub bot: BotId,
    pub app_id: String,
    pub team: TeamId,
    pub action_id: String,
    pub block_id: String,
    pub value: String,
    pub container: ActionContainer,
}

impl<B: Backend> Core<B> {
    pub(super) fn quick_switch_search(&self, query: String) {
        let ctx = self.ctx();
        self.spawn("edgeapi.search", async move {
            let trimmed = query.trim().to_owned();
            if trimmed.is_empty() {
                return Ok(());
            }
            let results = ctx.backend.edge_search(ctx.workspace, trimmed).await?;
            ctx.events.emit(Event::QuickSwitchResults {
                query,
                results: Box::new(results),
            });
            Ok(())
        });
    }

    pub(super) fn search_remote_messages(&self, query: String, sort: SearchSort, page: usize) {
        let ctx = self.ctx();
        self.spawn("search.modules.messages", async move {
            let res = ctx
                .backend
                .search_remote_messages(query.clone(), sort, page)
                .await;
            let emit = match res {
                Ok(data) => Ok(Box::new(data)),
                Err(e) => {
                    emit_if_auth_expired(&ctx.events, &e);
                    Err(e.to_string())
                }
            };
            ctx.events.emit(Event::RemoteMessagesPage {
                query,
                sort,
                page,
                result: emit,
            });
            Ok(())
        });
    }

    pub(super) fn search_remote_files(&self, query: String, sort: SearchSort, page: usize) {
        let ctx = self.ctx();
        self.spawn("search.modules.files", async move {
            let res = ctx
                .backend
                .search_remote_files(query.clone(), sort, page)
                .await;
            let emit = match res {
                Ok(data) => Ok(Box::new(data)),
                Err(e) => {
                    emit_if_auth_expired(&ctx.events, &e);
                    Err(e.to_string())
                }
            };
            ctx.events.emit(Event::RemoteFilesPage {
                query,
                sort,
                page,
                result: emit,
            });
            Ok(())
        });
    }

    pub(super) fn search_gifs(&self, query: String) {
        let ctx = self.ctx();
        let method = if query.trim().is_empty() {
            "gif.featured"
        } else {
            "gif.search"
        };
        self.spawn(method, async move {
            let trimmed = query.trim().to_owned();
            let gifs = if trimmed.is_empty() {
                ctx.backend.gif_featured(24).await?
            } else {
                ctx.backend.gif_search(trimmed, 24).await?
            };
            ctx.events.emit(Event::GifResults { query, gifs });
            Ok(())
        });
    }

    pub(super) fn load_files(&self, filter: FilesFilter) {
        let ctx = self.ctx();
        self.spawn("files.list", async move {
            let files = ctx.backend.files(filter.clone()).await;
            answer_load(&ctx.events, files, |files| Event::FilesLoaded {
                filter,
                files,
            })
        });
    }

    pub(super) fn upload_files(
        &self,
        channel: ChannelId,
        thread_ts: Option<Ts>,
        content: Option<MessageContent>,
        files: Vec<UploadFile>,
    ) {
        let ctx = self.ctx();
        self.spawn("files.upload", async move {
            ctx.backend
                .upload_files(channel, thread_ts, content, files)
                .await?;
            Ok(())
        });
    }

    pub(super) fn delete_file(&self, file_id: String) {
        let ctx = self.ctx();
        self.spawn("files.delete", async move {
            ctx.backend.delete_file(file_id.clone()).await?;
            ctx.events.emit(Event::FileDeleted { file_id });
            Ok(())
        });
    }

    pub(super) fn request_file_permission(&self, file_id: String, request_type: String) {
        let ctx = self.ctx();
        self.spawn("files.requestPermission", async move {
            ctx.backend
                .files_request_permission(file_id.clone(), request_type)
                .await?;
            ctx.events.emit(Event::FilePermissionRequested { file_id });
            Ok(())
        });
    }

    pub(super) fn load_files_extras(&self) {
        let ctx = self.ctx();
        self.spawn("files.extras", async move {
            reload_files_extras(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn set_file_favorite(
        &self,
        file_id: String,
        collection_id: Option<String>,
        starred: bool,
    ) {
        let ctx = self.ctx();
        self.spawn("files.favorites", async move {
            ctx.backend
                .files_favorites_set(file_id, collection_id, starred)
                .await?;
            reload_files_extras(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn create_file_collection(&self, name: String, emoji: Option<String>) {
        let ctx = self.ctx();
        self.spawn("files.collections.create", async move {
            let _ = ctx.backend.files_collections_create(name, emoji).await?;
            reload_files_extras(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn delete_file_collection(&self, collection_id: String) {
        let ctx = self.ctx();
        self.spawn("files.collections.delete", async move {
            ctx.backend.files_collections_delete(collection_id).await?;
            reload_files_extras(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn restore_deleted_file(&self, file_id: String) {
        let ctx = self.ctx();
        self.spawn("files.undelete", async move {
            ctx.backend.files_undelete(file_id.clone()).await?;
            ctx.events.emit(Event::FileRestored { file_id });
            reload_files_extras(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn create_channel_canvas(&self, channel: ChannelId) {
        let ctx = self.ctx();
        self.spawn("conversations.canvases.create", async move {
            let canvas_id = ctx.backend.create_canvas(channel.clone()).await?;
            // Refresh the stored conversation so `properties.canvas` lands.
            let fresh = ctx.backend.conversation_info(channel.clone()).await?;
            let ws = ctx.workspace.clone();
            with_store(&ctx.store, move |s| s.merge_conversation(&ws, &fresh)).await?;
            ctx.events.emit(Event::Conversations(vec![channel.clone()]));
            ctx.events
                .emit(Event::ChannelCanvasCreated { channel, canvas_id });
            Ok(())
        });
    }

    pub(super) fn load_canvas(&self, file_id: String) {
        let ctx = self.ctx();
        self.spawn("files.info(canvas)", async move {
            emit_canvas_html(&ctx.backend, &ctx.events, file_id).await;
            Ok(())
        });
    }

    pub(super) fn create_canvas(
        &self,
        title: String,
        markdown: String,
        channel: Option<ChannelId>,
    ) {
        let ctx = self.ctx();
        self.spawn("canvases.create", async move {
            let created = ctx
                .backend
                .canvases_create(
                    Some(title.clone()),
                    (!markdown.is_empty()).then_some(markdown),
                    channel.clone(),
                )
                .await?;
            ctx.events.emit(Event::CanvasCreated {
                file_id: created.canvas_id,
                title,
                channel,
            });
            Ok(())
        });
    }

    pub(super) fn edit_canvas(&self, file_id: String, op: CanvasEditOp) {
        let ctx = self.ctx();
        self.spawn("canvases.edit", async move {
            ctx.backend.canvases_edit(file_id.clone(), op).await?;
            emit_canvas_html(&ctx.backend, &ctx.events, file_id).await;
            Ok(())
        });
    }

    pub(super) fn delete_canvas(&self, canvas_id: String) {
        let ctx = self.ctx();
        self.spawn("canvases.delete", async move {
            let result = map_canvas_result(
                &ctx.events,
                ctx.backend.canvases_delete(canvas_id.clone()).await,
            );
            ctx.events.emit(Event::CanvasDeleted { canvas_id, result });
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
        let ctx = self.ctx();
        self.spawn("canvases.access.set", async move {
            let summary = access_summary(
                Some(access_level.as_str()),
                channel_ids.len(),
                user_ids.len(),
            );
            let shared = channel_ids.clone();
            let api_res = per_access_side(channel_ids, user_ids, |channels, users| {
                ctx.backend
                    .canvases_access_set(canvas_id.clone(), access_level, channels, users)
            })
            .await;
            let result = map_canvas_result(&ctx.events, api_res.map(drop));
            if result.is_ok() {
                post_permalink_to(&ctx, post_permalink, &shared).await;
            }
            ctx.events.emit(Event::CanvasAccessUpdated {
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
        let ctx = self.ctx();
        self.spawn("canvases.access.delete", async move {
            let summary = access_summary(None, channel_ids.len(), user_ids.len());
            let api_res = per_access_side(channel_ids, user_ids, |channels, users| {
                ctx.backend
                    .canvases_access_delete(canvas_id.clone(), channels, users)
            })
            .await;
            let result = map_canvas_result(&ctx.events, api_res.map(drop));
            ctx.events.emit(Event::CanvasAccessUpdated {
                canvas_id,
                summary,
                result,
            });
            Ok(())
        });
    }

    pub(super) fn load_canvas_comments(&self, canvas_id: String) {
        let ctx = self.ctx();
        self.spawn("canvas.comments.list", async move {
            let result = map_canvas_result(
                &ctx.events,
                ctx.backend.canvas_comments_list(canvas_id.clone()).await,
            );
            ctx.events
                .emit(Event::CanvasCommentsLoaded { canvas_id, result });
            Ok(())
        });
    }

    pub(super) fn post_canvas_comment(
        &self,
        canvas_id: String,
        text: String,
        thread_ts: Option<Ts>,
    ) {
        let ctx = self.ctx();
        self.spawn("canvas.comment.post", async move {
            let result = map_canvas_result(
                &ctx.events,
                ctx.backend
                    .canvas_comment_post(canvas_id.clone(), text, thread_ts)
                    .await
                    .map(|_| ()),
            );
            ctx.events
                .emit(Event::CanvasCommentPosted { canvas_id, result });
            Ok(())
        });
    }

    pub(super) fn resolve_canvas_comment(&self, canvas_id: String, thread_ts: Ts, resolved: bool) {
        let ctx = self.ctx();
        let context = if resolved {
            "quip.thread.archive"
        } else {
            "quip.thread.unarchive"
        };
        self.spawn(context, async move {
            let channel_id = crate::api::canvas_comment_channel_id(&canvas_id);
            let res = if resolved {
                ctx.backend
                    .quip_thread_archive(channel_id, thread_ts.clone())
                    .await
            } else {
                ctx.backend
                    .quip_thread_unarchive(channel_id, thread_ts.clone())
                    .await
            };
            let result = map_canvas_result(&ctx.events, res);
            ctx.events.emit(Event::CanvasCommentResolved {
                canvas_id,
                thread_ts,
                resolved,
                result,
            });
            Ok(())
        });
    }

    pub(super) fn resolve_all_canvas_comments(&self, canvas_id: String) {
        let ctx = self.ctx();
        self.spawn("quip.thread.archiveAll", async move {
            let channel_id = crate::api::canvas_comment_channel_id(&canvas_id);
            let result = map_canvas_result(
                &ctx.events,
                ctx.backend.quip_thread_archive_all(channel_id).await,
            );
            ctx.events.emit(Event::CanvasCommentResolved {
                canvas_id,
                thread_ts: Ts::default(),
                resolved: true,
                result,
            });
            Ok(())
        });
    }

    pub(super) fn load_list(&self, file_id: String) {
        let ctx = self.ctx();
        self.spawn("lists.records.list", async move {
            emit_list(&ctx.backend, &ctx.events, file_id).await;
            Ok(())
        });
    }

    pub(super) fn create_list(
        &self,
        name: String,
        description: Option<String>,
        channel: Option<ChannelId>,
    ) {
        let ctx = self.ctx();
        self.spawn("lists.create", async move {
            let file_id = ctx
                .backend
                .lists_create(name.clone(), description, channel)
                .await?;
            ctx.events.emit(Event::ListCreated { file_id, name });
            Ok(())
        });
    }

    pub(super) fn create_list_item(&self, list_id: String, initial_fields: Vec<ListCellInput>) {
        let ctx = self.ctx();
        self.spawn("lists.records.create", async move {
            let _ = ctx
                .backend
                .lists_records_create(list_id.clone(), initial_fields)
                .await?;
            emit_list(&ctx.backend, &ctx.events, list_id).await;
            Ok(())
        });
    }

    pub(super) fn update_list_cell(
        &self,
        list_id: String,
        row_id: String,
        cells: Vec<ListCellInput>,
    ) {
        let ctx = self.ctx();
        self.spawn("lists.cells.update", async move {
            ctx.backend
                .lists_cells_update(list_id.clone(), row_id, cells)
                .await?;
            emit_list(&ctx.backend, &ctx.events, list_id).await;
            Ok(())
        });
    }

    pub(super) fn delete_list_item(&self, list_id: String, row_id: String) {
        let ctx = self.ctx();
        self.spawn("lists.records.delete", async move {
            ctx.backend
                .lists_records_delete(list_id.clone(), row_id)
                .await?;
            emit_list(&ctx.backend, &ctx.events, list_id).await;
            Ok(())
        });
    }

    pub(super) fn add_message_to_list(&self, list_id: String, channel: ChannelId, ts: Ts) {
        let ctx = self.ctx();
        self.spawn("lists.addToList", async move {
            let record_id = ctx
                .backend
                .lists_add_to_list(list_id.clone(), channel, ts)
                .await?;
            ctx.events
                .emit(Event::MessageAddedToList { list_id, record_id });
            Ok(())
        });
    }

    pub(super) fn archive_list_item(&self, list_id: String, row_id: String) {
        let ctx = self.ctx();
        self.spawn("lists.records.archive", async move {
            ctx.backend
                .lists_records_archive(list_id.clone(), row_id)
                .await?;
            emit_list(&ctx.backend, &ctx.events, list_id).await;
            Ok(())
        });
    }

    pub(super) fn set_list_access(
        &self,
        list_id: String,
        access_level: ListAccessLevel,
        channel_ids: Vec<ChannelId>,
        user_ids: Vec<UserId>,
        post_permalink: Option<String>,
    ) {
        let ctx = self.ctx();
        self.spawn("slackLists.access.set", async move {
            let summary = access_summary(
                Some(access_level.as_str()),
                channel_ids.len(),
                user_ids.len(),
            );
            let shared = channel_ids.clone();
            let api_res = per_access_side(channel_ids, user_ids, |channels, users| {
                ctx.backend
                    .lists_access_set(list_id.clone(), access_level, channels, users)
            })
            .await;
            let result = map_list_result(&ctx.events, api_res.map(drop));
            if result.is_ok() {
                post_permalink_to(&ctx, post_permalink, &shared).await;
            }
            ctx.events.emit(Event::ListAccessUpdated {
                list_id,
                summary,
                result,
            });
            Ok(())
        });
    }

    pub(super) fn delete_list_access(
        &self,
        list_id: String,
        channel_ids: Vec<ChannelId>,
        user_ids: Vec<UserId>,
    ) {
        let ctx = self.ctx();
        self.spawn("slackLists.access.delete", async move {
            let summary = access_summary(None, channel_ids.len(), user_ids.len());
            let api_res = per_access_side(channel_ids, user_ids, |channels, users| {
                ctx.backend
                    .lists_access_delete(list_id.clone(), channels, users)
            })
            .await;
            // Slack answers `ok` while reporting the ids it could not update.
            let result = map_list_result(&ctx.events, api_res).and_then(|reports| {
                let channels = reports
                    .iter()
                    .map(|r| r.failed_to_update_channel_ids.len())
                    .sum();
                let users = reports
                    .iter()
                    .map(|r| r.failed_to_update_user_ids.len())
                    .sum();
                access_targets(channels, users).map_or(Ok(()), |targets| {
                    Err(format!("Slack could not revoke access for {targets}."))
                })
            });
            ctx.events.emit(Event::ListAccessUpdated {
                list_id,
                summary,
                result,
            });
            Ok(())
        });
    }

    pub(super) fn fetch_list_record_comments(&self, list_id: String, record_id: String) {
        let ctx = self.ctx();
        self.spawn("lists.getThreadsForRecord", async move {
            let messages = match ctx
                .backend
                .lists_get_threads_for_record(list_id.clone(), record_id.clone())
                .await
            {
                Ok(messages) => messages,
                Err(e) if e.is_auth_expired() => return Err(e.into()),
                Err(e) => {
                    tracing::warn!(%list_id, %record_id, %e, "lists.getThreadsForRecord failed");
                    Vec::new()
                }
            };
            ctx.events.emit(Event::ListRecordComments {
                list_id,
                record_id,
                messages,
            });
            Ok(())
        });
    }

    pub(super) fn load_app_home(&self, channel: ChannelId) {
        let ctx = self.ctx();
        self.spawn("conversations.info(app_home)", async move {
            let home = match ctx.backend.load_app_home(channel.clone()).await {
                Ok(home) => Ok(Box::new(home)),
                Err(e) => {
                    emit_if_auth_expired(&ctx.events, &e);
                    Err(e.to_string())
                }
            };
            ctx.events.emit(Event::AppHomeLoaded { channel, home });
            Ok(())
        });
    }

    pub(super) fn open_app_home(&self, app_id: String, team_id: String) {
        let ctx = self.ctx();
        self.spawn("apps.home.open", async move {
            if let Some(channel) = ctx.backend.open_app_home(app_id, team_id).await? {
                ctx.events.emit(Event::ConversationOpened(channel));
            }
            Ok(())
        });
    }

    pub(super) fn trip_workflow_trigger(&self, url: String, context: serde_json::Value) {
        let ctx = self.ctx();
        self.spawn("workflows.triggers.trip", async move {
            let resp = ctx
                .backend
                .trip_workflow_trigger(url.clone(), context)
                .await?;
            ctx.events.emit(Event::WorkflowTripped {
                url,
                is_slow_workflow: resp.is_slow_workflow,
                contains_ai_steps: resp.contains_ai_steps,
            });
            Ok(())
        });
    }

    pub(super) fn fetch_workflow_trigger_info(&self, trigger_id: String) {
        let ctx = self.ctx();
        self.spawn("workflows.triggers.info", async move {
            let info = match ctx.backend.workflow_trigger_info(trigger_id.clone()).await {
                Ok(info) => Ok(Box::new(info)),
                Err(e) => {
                    emit_if_auth_expired(&ctx.events, &e);
                    Err(e.to_string())
                }
            };
            ctx.events
                .emit(Event::WorkflowTriggerInfoLoaded { trigger_id, info });
            Ok(())
        });
    }

    pub(super) fn submit_modal(
        &self,
        view_id: String,
        hash: Option<String>,
        state_values: serde_json::Value,
    ) {
        let ctx = self.ctx();
        self.spawn("views.submit", async move {
            let outcome = match ctx
                .backend
                .views_submit(view_id.clone(), hash, state_values)
                .await
            {
                Ok(resp) => Ok(resp),
                Err(e) => {
                    emit_if_auth_expired(&ctx.events, &e);
                    Err(e.to_string())
                }
            };
            ctx.events
                .emit(Event::ModalSubmitResult { view_id, outcome });
            Ok(())
        });
    }

    pub(super) fn close_modal(
        &self,
        view_id: String,
        root_view_id: Option<String>,
        hash: Option<String>,
    ) {
        let ctx = self.ctx();
        self.spawn("views.close", async move {
            ctx.backend.views_close(view_id, root_view_id, hash).await?;
            Ok(())
        });
    }

    pub(super) fn fetch_call_info(&self, call_id: String) {
        let ctx = self.ctx();
        self.spawn("calls.info", async move {
            let call = match ctx.backend.calls_info(call_id.clone()).await {
                Ok(call) => Ok(Box::new(call)),
                Err(e) => {
                    emit_if_auth_expired(&ctx.events, &e);
                    Err(e.to_string())
                }
            };
            ctx.events.emit(Event::CallInfoLoaded { call_id, call });
            Ok(())
        });
    }

    pub(super) fn block_suggestions(&self, params: BlockSuggestionsParams) {
        let ctx = self.ctx();
        self.spawn("blocks.suggestions", async move {
            let req = BlockSuggestionsRequest {
                bot: &params.bot,
                app_id: &params.app_id,
                team: &params.team,
                action_id: &params.action_id,
                block_id: &params.block_id,
                value: &params.value,
                container: &params.container,
            };
            match ctx.backend.block_suggestions(req).await {
                Ok(options) => {
                    ctx.events.emit(Event::BlockSuggestions {
                        key: params.key,
                        options,
                    });
                    Ok(())
                }
                Err(err) => {
                    // Always emit an empty suggestions result so UI loading state clears and
                    // users can retry instead of remaining permanently disabled.
                    ctx.events.emit(Event::BlockSuggestions {
                        key: params.key,
                        options: Vec::new(),
                    });
                    Err(err.into())
                }
            }
        });
    }

    pub(super) fn transcribe_media_file(
        &self,
        file_id: crate::types::FileId,
        local_path: std::path::PathBuf,
        provider: crate::huddle::TranscriptionProvider,
    ) {
        let ctx = self.ctx();
        self.spawn("media.transcribe", async move {
            let ws_check = ctx.workspace.clone();
            let fid_check = file_id.clone();
            if let Ok(Some(existing)) = with_store(&ctx.store, move |s| {
                s.file_transcript(&ws_check, &fid_check)
            })
            .await
                && !existing.text.trim().is_empty()
            {
                ctx.events.emit(Event::FileTranscriptReady {
                    file_id,
                    text: existing.text,
                    cues: existing.cues,
                });
                return Ok(());
            }

            let path_for_task = local_path.clone();
            let transcribed = tokio::task::spawn_blocking(move || {
                crate::huddle::transcribe_media_file_timed(&path_for_task, provider)
            })
            .await;

            match transcribed {
                Ok(Ok(timed)) => {
                    let trimmed = timed.text.trim().to_owned();
                    let cues = timed.cues;
                    let ws_save = ctx.workspace.clone();
                    let fid_save = file_id.clone();
                    let text_save = trimmed.clone();
                    let cues_save = cues.clone();
                    let source = timed.provider.source_tag().to_owned();
                    if let Err(err) = with_store(&ctx.store, move |s| {
                        s.upsert_file_transcript_with_cues(
                            &ws_save, &fid_save, &source, &text_save, &cues_save,
                        )
                    })
                    .await
                    {
                        ctx.events.emit(Event::FileTranscriptFailed {
                            file_id,
                            error: err.to_string(),
                        });
                        return Ok(());
                    }
                    ctx.events.emit(Event::FileTranscriptReady {
                        file_id,
                        text: trimmed,
                        cues,
                    });
                }
                Ok(Err(error)) => {
                    ctx.events
                        .emit(Event::FileTranscriptFailed { file_id, error });
                }
                Err(join_err) => {
                    ctx.events.emit(Event::FileTranscriptFailed {
                        file_id,
                        error: format!("transcription worker failed: {join_err}"),
                    });
                }
            }
            Ok(())
        });
    }
}

/// "2 channel(s) and 1 user(s)": who an access change covers (`None` for nobody).
fn access_targets(channels: usize, users: usize) -> Option<String> {
    match (channels, users) {
        (0, 0) => None,
        (channels, 0) => Some(format!("{channels} channel(s)")),
        (0, users) => Some(format!("{users} user(s)")),
        (channels, users) => Some(format!("{channels} channel(s) and {users} user(s)")),
    }
}

/// The share dialog's line for an access change: a grant of `level`, or a revoke when `None`.
fn access_summary(level: Option<&str>, channels: usize, users: usize) -> String {
    match (level, access_targets(channels, users)) {
        (Some(level), Some(targets)) => format!("Granted {level} access to {targets}"),
        (Some(level), None) => format!("Updated {level} access"),
        (None, Some(targets)) => format!("Revoked access for {targets}"),
        (None, None) => "Revoked access".to_owned(),
    }
}

/// Runs a canvas/List access call once per non-empty side, channels first: Slack takes
/// `channel_ids` XOR `user_ids` per call. Stops at the first failure.
async fn per_access_side<T, Fut>(
    channel_ids: Vec<ChannelId>,
    user_ids: Vec<UserId>,
    mut call: impl FnMut(Vec<ChannelId>, Vec<UserId>) -> Fut,
) -> api::Result<Vec<T>>
where
    Fut: Future<Output = api::Result<T>>,
{
    if channel_ids.is_empty() && user_ids.is_empty() {
        return Err(ApiError::Slack("invalid_parameters".to_owned()));
    }
    let mut answers = Vec::new();
    if !channel_ids.is_empty() {
        answers.push(call(channel_ids, Vec::new()).await?);
    }
    if !user_ids.is_empty() {
        answers.push(call(Vec::new(), user_ids).await?);
    }
    Ok(answers)
}

/// Posts a shared canvas's or List's `permalink` into each channel it was shared with. A failed
/// post is logged (an expired session reported) without failing the share.
async fn post_permalink_to<B: Backend>(
    ctx: &TaskCtx<B>,
    permalink: Option<String>,
    channels: &[ChannelId],
) {
    let Some(url) = permalink.filter(|u| !u.trim().is_empty()) else {
        return;
    };
    for channel in channels {
        if let Err(err) = ctx
            .backend
            .post_message(
                channel.clone(),
                MessageContent::plain(url.clone()),
                None,
                false,
                None,
            )
            .await
        {
            emit_if_auth_expired(&ctx.events, &err);
            tracing::warn!(%channel, %err, "failed to post a shared permalink");
        }
    }
}

fn emit_if_auth_expired(events: &Emitter, err: &ApiError) {
    if let ApiError::AuthExpired(code) = err {
        events.emit(Event::AuthExpired(code.clone()));
    }
}

fn map_canvas_result<T>(events: &Emitter, res: crate::api::Result<T>) -> Result<T, String> {
    match res {
        Ok(value) => Ok(value),
        Err(err) => {
            emit_if_auth_expired(events, &err);
            Err(format_canvas_api_error(&err))
        }
    }
}

async fn emit_canvas_html<B: Backend>(backend: &B, events: &Emitter, file_id: String) {
    match map_canvas_result(
        events,
        backend.fetch_canvas_with_meta(file_id.clone()).await,
    ) {
        Ok((meta, html)) => {
            let title = meta.title.clone();
            events.emit(Event::CanvasLoaded {
                file_id: file_id.clone(),
                title,
                html: Ok(html),
            });
            events.emit(Event::CanvasMetaLoaded { file_id, meta });
        }
        Err(err_msg) => {
            events.emit(Event::CanvasLoaded {
                file_id,
                title: "Canvas".to_owned(),
                html: Err(err_msg),
            });
        }
    }
}

fn map_list_result<T>(events: &Emitter, res: api::Result<T>) -> Result<T, String> {
    match res {
        Ok(value) => Ok(value),
        Err(err) => {
            emit_if_auth_expired(events, &err);
            Err(format_list_api_error(&err))
        }
    }
}

async fn emit_list<B: Backend>(backend: &B, events: &Emitter, file_id: String) {
    match map_list_result(events, backend.fetch_list_with_meta(file_id.clone()).await) {
        Ok((list, meta)) => {
            events.emit(Event::ListLoaded {
                file_id: file_id.clone(),
                list: Ok(Box::new(list)),
            });
            events.emit(Event::ListMetaLoaded {
                file_id,
                meta: Box::new(meta),
            });
        }
        Err(err) => {
            events.emit(Event::ListLoaded {
                file_id,
                list: Err(err),
            });
        }
    }
}

async fn reload_files_extras<B: Backend>(backend: &B, events: &Emitter) -> Result<(), CoreError> {
    let (favorites, collections, deleted) = tokio::join!(
        backend.files_favorites_list(),
        backend.files_collections_list(),
        backend.files_recently_deleted(),
    );
    for err in [
        favorites.as_ref().err(),
        collections.as_ref().err(),
        deleted.as_ref().err(),
    ]
    .into_iter()
    .flatten()
    {
        if let ApiError::AuthExpired(code) = err {
            return Err(ApiError::AuthExpired(code.clone()).into());
        }
    }
    events.emit(Event::FilesExtrasLoaded {
        favorites: favorites.unwrap_or_default(),
        collections: collections.unwrap_or_default(),
        deleted: deleted.unwrap_or_default(),
    });
    Ok(())
}
