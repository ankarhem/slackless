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
mod client;
pub mod command;
pub mod diff;
pub mod domain;
pub mod event;
pub mod export;
mod fetcher;
mod notify;
pub mod paths;
pub mod privacy;
mod reducer;
pub mod search;
mod unfurl;

pub use backend::{
    Backend, ChannelsBackend, CollabBackend, FilesBackend, HuddleBackend, MessagingBackend,
    UsersBackend, WorkspaceBackend,
};
pub use backfill::{
    BACKFILL_SYNCED_KEY, BackfillStatus, ChannelBackfillProgress, Synced,
    WorkspaceBackfillProgress, decode_synced_spans, read_synced_spans, write_synced_spans,
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
    let (used, text) = huddle::transcribe_media_file(local_path, preferred)?;
    let trimmed = text.trim().to_owned();
    let updated = store
        .upsert_file_transcript(workspace, file_id, used.source_tag(), &trimmed)
        .map_err(|e| format!("failed to index transcript in local store: {e}"))?;
    Ok((used, trimmed, updated))
}
