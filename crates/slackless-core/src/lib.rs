//! slackless orchestrator: owns the tokio runtime thread and wires api + rtm + store + auth into
//! a front-end-agnostic command/event API (ARCHITECTURE: core).
//!
//! GPUI is smol-based, so the networking stack (reqwest, tokio-tungstenite) runs on a dedicated
//! OS thread that owns a multi-thread tokio runtime ([`Client::start`]). Front-ends talk to it
//! over runtime-agnostic `futures::channel::mpsc` channels ([`ClientHandle`]): [`Command`]s in,
//! ordered [`Event`]s out, and high-frequency [`HuddleMediaEvent`]s on a separate bounded channel.
//!
//! The [`store::Store`] is the source of truth. The core keeps it current (hydrate, websocket
//! events via [`apply_event`], history backfill) and emits events saying what changed; front-ends
//! re-read the store through [`ClientHandle::store`].
//!
//! Patchability: the core models no Slack JSON itself. Responses and websocket events are parsed
//! by `slackless_types` / `slackless_api` (lenient fields, catch-all `Unknown` / `Other`
//! variants, method names and URLs centralized in `types::endpoints`).

pub use slackless_api as api;
pub use slackless_auth as auth;
pub use slackless_huddle as huddle;
pub use slackless_rtm as rtm;
pub use slackless_store as store;
pub use slackless_types as types;

mod backend;
pub mod backfill;
pub mod catchup;
mod client;
pub mod command;
pub mod diff;
pub mod domain;
pub mod embeddings;
pub mod event;
pub mod export;
mod fetcher;
mod notify;
pub mod outbox;
pub mod paths;
pub mod privacy;
mod reducer;
pub mod search;
pub mod semantic_index;
mod unfurl;

pub use backend::{
    Backend, ChannelsBackend, CollabBackend, FilesBackend, HuddleBackend, MessagingBackend,
    UsersBackend, WorkspaceBackend,
};
pub use backfill::{
    BACKFILL_SYNCED_KEY, BackfillStatus, ChannelBackfillProgress, Synced,
    WorkspaceBackfillProgress, decode_synced_spans, read_synced_spans, write_synced_spans,
};
pub use catchup::{
    CATCHUP_LAST_VISIT_KEY, CatchUpBriefing, CatchUpExcerpt, CatchUpMention, CatchUpThread,
    CatchUpTranscript, CatchUpUnreadChannel, CatchUpWindow, CatchUpWorkspaceSummary,
    DEFAULT_CATCHUP_LIMIT, MentionKind, TranscriptKind, WorkspaceCatchUpTarget,
    build_catchup_across_stores, build_catchup_disk_archives, build_workspace_catchup,
    format_catchup_human, read_catchup_last_visit, record_catchup_visit,
};
pub use client::{
    Client, ClientHandle, ClientHandles, Closed, CommandSender, EventReceiver, HuddleMediaReceiver,
    HuddleMediaSender, SharedStore,
};
pub use command::{
    ChannelCommand, CollabCommand, Command, HuddleCommand, HuddleControlCommand, MessagingCommand,
    WorkspaceCommand,
};
pub use diff::{DiffKind, DiffSegment, diff_words};
pub use embeddings::{
    Embedder, EmbeddingDownloadProgress, EmbeddingModel, EmbeddingModelStatus,
    HashProjectionEmbedder, OnnxEmbedder, TokenizedInput, WordPieceTokenizer,
    default_embedding_model_dir, delete_embedding_model, detect_embedding_model_setup,
    download_embedding_model, download_embedding_model_async,
};
pub use event::{
    Event, FailedMutation, HUDDLE_MEDIA_CHANNEL_CAPACITY, HuddleEvent, HuddleMediaEvent,
};
pub use export::{
    ExportArtifact, ExportFormat, ExportOptions, ExportTarget, ExportedChannelMeta, ExportedFile,
    ExportedFileTranscriptDoc, ExportedHuddleSegment, ExportedHuddleTranscriptDoc, ExportedMessage,
    ExportedMessagesDoc, ExportedReaction, export_channel, export_file_transcript,
    export_huddle_transcript, export_target, export_thread,
};
pub use fetcher::Fetcher;
pub use notify::{NotifyLevel, notify_levels};
pub use outbox::{OutboxPayload, decode_outbox_payload, is_transient_send_error};
pub use privacy::{
    SecretMatch, detect_secrets, strip_tracking_params, strip_tracking_params_in_text,
};
pub use reducer::apply_event;
pub use search::{
    SearchHitKind, UnifiedSearchHit, UnifiedSearchResults, UnifiedTranscriptHit,
    WorkspaceSearchTarget, discover_disk_archives, discover_local_archives, search_across_stores,
    search_all_local_workspaces, search_disk_archives, search_huddle_transcripts_across_stores,
    search_huddle_transcripts_in_disk_archives, search_single_store_all_workspaces,
};
pub use semantic_index::{
    SemanticIndexProgress, SemanticIndexReport, default_semantic_index_stats,
    hybrid_search_disk_archives, hybrid_search_huddle_transcripts_across_stores,
    hybrid_search_huddle_transcripts_in_disk_archives, hybrid_search_stores, index_disk_archives,
    index_store_documents, open_default_semantic_index, open_existing_semantic_index,
};

/// Transcribe a local video or audio file with `preferred` (or any ready local provider), store
/// the transcript under `(workspace, file_id)`, and re-index every carrying message in `message_fts`.
///
/// Returns `(used_provider, transcript_text, indexed_message_count)`.
pub fn transcribe_and_index_file(
    store: &mut store::Store,
    workspace: &types::TeamId,
    file_id: &types::FileId,
    local_path: &std::path::Path,
    preferred: huddle::TranscriptionProvider,
) -> std::result::Result<(huddle::TranscriptionProvider, String, usize), String> {
    let timed = huddle::transcribe_media_file_timed(local_path, preferred)?;
    let trimmed = timed.text.trim().to_owned();
    let updated = store
        .upsert_file_transcript_with_cues(
            workspace,
            file_id,
            timed.provider.source_tag(),
            &trimmed,
            &timed.cues,
        )
        .map_err(|e| format!("failed to index transcript in local store: {e}"))?;
    Ok((timed.provider, trimmed, updated))
}
