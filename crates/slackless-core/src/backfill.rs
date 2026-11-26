//! Paged `conversations.*` fetches into the store: background history backfill walks
//! `conversations.history` (and optionally `conversations.replies` for threads) so a
//! conversation's history stays available locally even after Slack stops serving it, with
//! structured per-channel / per-workspace progress reporting and pause/resume controls.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;

use crate::Event;
use crate::api::{ApiError, Page};
use crate::backend::Backend;
use crate::client::{CoreError, Emitter, SharedStore, with_store};
use crate::store::Store;
use crate::types::{ChannelId, Message, TeamId, Ts};
use crate::unfurl::Unfurls;

/// Pause between history pages. `conversations.history` is Tier 3 (~50 requests/minute).
pub(crate) const PAGE_DELAY: Duration = Duration::from_millis(1200);
/// How many times one page waits out a rate limit (on top of the API client's own retries).
const MAX_RATE_LIMIT_WAITS: u32 = 5;
/// `local_state` key persisting known backfill spans per workspace so restarts skip completed
/// history walks.
pub const BACKFILL_SYNCED_KEY: &str = "backfill_synced_spans";

/// The contiguous span of a conversation's channel-level history known to be in the store.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Synced {
    pub newest: Ts,
    pub oldest: Ts,
    /// `oldest` is the beginning of the conversation.
    pub complete: bool,
}

pub(crate) type SyncedMap = Arc<Mutex<HashMap<ChannelId, Synced>>>;

/// Lifecycle state for a channel or workspace archive backfill job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillStatus {
    #[default]
    Idle,
    Running,
    Paused,
    Complete,
    Error,
}

impl BackfillStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Running => "Syncing",
            Self::Paused => "Paused",
            Self::Complete => "Complete",
            Self::Error => "Error",
        }
    }
}

/// Per-channel archive backfill progress snapshot.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChannelBackfillProgress {
    pub channel: ChannelId,
    pub channel_name: Option<String>,
    pub status: BackfillStatus,
    pub pages_fetched: u64,
    pub messages_stored: u64,
    pub threads_fetched: u64,
    pub replies_stored: u64,
    pub oldest_ts: Option<Ts>,
    pub newest_ts: Option<Ts>,
    pub complete: bool,
    pub last_error: Option<String>,
}

/// Workspace-wide archive backfill progress snapshot emitted via [`Event::BackfillProgress`].
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceBackfillProgress {
    pub workspace: TeamId,
    pub status: BackfillStatus,
    pub paused: bool,
    pub active_channel: Option<ChannelId>,
    pub total_channels: usize,
    pub completed_channels: usize,
    pub pages_fetched: u64,
    pub messages_stored: u64,
    pub threads_fetched: u64,
    pub replies_stored: u64,
    pub channels: Vec<ChannelBackfillProgress>,
}

/// Shared pause/resume and progress tracker for background archive backfill workers.
#[derive(Clone)]
pub(crate) struct BackfillControl {
    paused: Arc<AtomicBool>,
    notify: Arc<Notify>,
    progress: Arc<Mutex<WorkspaceBackfillProgress>>,
}

impl BackfillControl {
    pub(crate) fn new(workspace: TeamId) -> Self {
        Self {
            paused: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
            progress: Arc::new(Mutex::new(WorkspaceBackfillProgress {
                workspace,
                ..Default::default()
            })),
        }
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// Suspend active backfill workers cleanly between API pages without losing progress.
    pub(crate) fn pause(&self) -> WorkspaceBackfillProgress {
        self.paused.store(true, Ordering::Release);
        let mut p = self.lock();
        p.paused = true;
        if p.status == BackfillStatus::Running {
            p.status = BackfillStatus::Paused;
        }
        for ch in &mut p.channels {
            if ch.status == BackfillStatus::Running {
                ch.status = BackfillStatus::Paused;
            }
        }
        p.clone()
    }

    /// Resume any paused backfill workers immediately.
    pub(crate) fn resume(&self) -> WorkspaceBackfillProgress {
        self.paused.store(false, Ordering::Release);
        let snap = {
            let mut p = self.lock();
            p.paused = false;
            if p.status == BackfillStatus::Paused {
                p.status = BackfillStatus::Running;
            }
            for ch in &mut p.channels {
                if ch.status == BackfillStatus::Paused {
                    ch.status = BackfillStatus::Running;
                }
            }
            p.clone()
        };
        self.notify.notify_waiters();
        snap
    }

    /// Park cleanly while paused; returns immediately when not paused.
    pub(crate) async fn wait_if_paused(
        &self,
        events: &Emitter,
        channel: Option<&ChannelId>,
        emit_progress: bool,
    ) {
        if !self.is_paused() {
            return;
        }
        let snap = {
            let mut p = self.lock();
            p.paused = true;
            if p.status == BackfillStatus::Running {
                p.status = BackfillStatus::Paused;
            }
            if let Some(ch_id) = channel
                && let Some(entry) = p.channels.iter_mut().find(|c| c.channel == *ch_id)
                && entry.status == BackfillStatus::Running
            {
                entry.status = BackfillStatus::Paused;
            }
            p.clone()
        };
        if emit_progress {
            events.emit(Event::BackfillProgress(snap));
        }

        while self.is_paused() {
            let notified = self.notify.notified();
            if !self.is_paused() {
                break;
            }
            notified.await;
        }

        let resumed = {
            let mut p = self.lock();
            p.paused = false;
            if p.status == BackfillStatus::Paused {
                p.status = BackfillStatus::Running;
            }
            if let Some(ch_id) = channel
                && let Some(entry) = p.channels.iter_mut().find(|c| c.channel == *ch_id)
                && entry.status == BackfillStatus::Paused
            {
                entry.status = BackfillStatus::Running;
            }
            p.clone()
        };
        if emit_progress {
            events.emit(Event::BackfillProgress(resumed));
        }
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> WorkspaceBackfillProgress {
        self.lock().clone()
    }

    /// Populate/refresh channel rows from persisted SQLite spans and counts while preserving any
    /// in-flight counters.
    pub(crate) fn sync_inventory(
        &self,
        channels: &[(ChannelId, Option<String>)],
        synced: &HashMap<ChannelId, Synced>,
        msg_counts: &HashMap<ChannelId, u64>,
        mark_running: bool,
    ) -> WorkspaceBackfillProgress {
        let mut p = self.lock();
        let existing: HashMap<ChannelId, ChannelBackfillProgress> = p
            .channels
            .drain(..)
            .map(|c| (c.channel.clone(), c))
            .collect();
        for (id, name) in channels {
            let span = synced.get(id);
            let stored_count = msg_counts.get(id).copied().unwrap_or(0);
            let mut entry = existing
                .get(id)
                .cloned()
                .unwrap_or_else(|| ChannelBackfillProgress {
                    channel: id.clone(),
                    channel_name: name.clone(),
                    status: if span.is_some_and(|s| s.complete) {
                        BackfillStatus::Complete
                    } else {
                        BackfillStatus::Idle
                    },
                    pages_fetched: 0,
                    messages_stored: stored_count,
                    threads_fetched: 0,
                    replies_stored: 0,
                    oldest_ts: span.map(|s| s.oldest.clone()),
                    newest_ts: span.map(|s| s.newest.clone()),
                    complete: span.is_some_and(|s| s.complete),
                    last_error: None,
                });
            if entry.channel_name.is_none() {
                entry.channel_name = name.clone();
            }
            if let Some(span) = span {
                entry.oldest_ts = Some(span.oldest.clone());
                entry.newest_ts = Some(span.newest.clone());
                entry.complete |= span.complete;
                if entry.complete && entry.status == BackfillStatus::Idle {
                    entry.status = BackfillStatus::Complete;
                }
            }
            if stored_count > entry.messages_stored {
                entry.messages_stored = stored_count;
            }
            p.channels.push(entry);
        }
        p.total_channels = p.channels.len();
        p.completed_channels = p.channels.iter().filter(|c| c.complete).count();
        if mark_running {
            p.status = if p.paused {
                BackfillStatus::Paused
            } else {
                BackfillStatus::Running
            };
        } else if p.status == BackfillStatus::Idle
            && p.total_channels > 0
            && p.completed_channels == p.total_channels
        {
            p.status = BackfillStatus::Complete;
        }
        p.clone()
    }

    pub(crate) fn update_channel(
        &self,
        channel: &ChannelId,
        channel_name: Option<&str>,
        f: impl FnOnce(&mut ChannelBackfillProgress),
    ) -> WorkspaceBackfillProgress {
        let mut p = self.lock();
        if let Some(entry) = p.channels.iter_mut().find(|c| c.channel == *channel) {
            if entry.channel_name.is_none()
                && let Some(name) = channel_name
            {
                entry.channel_name = Some(name.to_owned());
            }
            f(entry);
        } else {
            let mut entry = ChannelBackfillProgress {
                channel: channel.clone(),
                channel_name: channel_name.map(ToOwned::to_owned),
                status: BackfillStatus::Running,
                pages_fetched: 0,
                messages_stored: 0,
                threads_fetched: 0,
                replies_stored: 0,
                oldest_ts: None,
                newest_ts: None,
                complete: false,
                last_error: None,
            };
            f(&mut entry);
            p.channels.push(entry);
        }
        p.total_channels = p.channels.len();
        p.completed_channels = p.channels.iter().filter(|c| c.complete).count();
        p.pages_fetched = p.channels.iter().map(|c| c.pages_fetched).sum();
        p.messages_stored = p.channels.iter().map(|c| c.messages_stored).sum();
        p.threads_fetched = p.channels.iter().map(|c| c.threads_fetched).sum();
        p.replies_stored = p.channels.iter().map(|c| c.replies_stored).sum();
        p.clone()
    }

    pub(crate) fn set_active_channel(
        &self,
        channel: Option<ChannelId>,
    ) -> WorkspaceBackfillProgress {
        let mut p = self.lock();
        p.active_channel = channel;
        if p.active_channel.is_some() {
            p.status = if p.paused {
                BackfillStatus::Paused
            } else {
                BackfillStatus::Running
            };
        }
        p.clone()
    }

    pub(crate) fn finish_run(&self, had_error: bool) -> WorkspaceBackfillProgress {
        let mut p = self.lock();
        p.active_channel = None;
        p.completed_channels = p.channels.iter().filter(|c| c.complete).count();
        p.status = if had_error {
            BackfillStatus::Error
        } else if p.total_channels > 0 && p.completed_channels == p.total_channels {
            BackfillStatus::Complete
        } else {
            BackfillStatus::Idle
        };
        p.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, WorkspaceBackfillProgress> {
        self.progress.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Deserialize persisted [`Synced`] spans from `local_state`, returning an empty map if absent
/// or malformed.
pub fn decode_synced_spans(raw: Option<String>) -> HashMap<ChannelId, Synced> {
    raw.and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

/// Read persisted [`Synced`] backfill spans for `workspace` from SQLite.
pub fn read_synced_spans(store: &Store, workspace: &TeamId) -> HashMap<ChannelId, Synced> {
    decode_synced_spans(store.local(workspace, BACKFILL_SYNCED_KEY).ok().flatten())
}

/// Persist [`Synced`] backfill spans for `workspace` into SQLite `local_state`.
pub fn write_synced_spans(
    store: &mut Store,
    workspace: &TeamId,
    spans: &HashMap<ChannelId, Synced>,
) -> crate::store::Result<()> {
    let payload = serde_json::to_string(spans)?;
    store.set_local(workspace, BACKFILL_SYNCED_KEY, &payload)
}

/// One backfill worker configuration; [`Backfill::run`] walks one conversation, and
/// [`Backfill::run_workspace`] walks an entire workspace's channels sequentially. Cancel by
/// aborting its task: every page is committed before the next is fetched, so an aborted walk
/// resumes where it stopped on the next run.
#[derive(Clone)]
pub(crate) struct Backfill<B: Backend> {
    pub backend: B,
    pub store: SharedStore,
    pub workspace: TeamId,
    pub events: Emitter,
    pub synced: SyncedMap,
    pub page_delay: Duration,
    /// Names the foreign channels the pages' unfurls point at.
    pub(crate) unfurls: Unfurls<B>,
    /// Shared pause/resume & progress state reported via [`Event::BackfillProgress`].
    pub(crate) control: BackfillControl,
}

impl<B: Backend> Backfill<B> {
    /// Fetches the newest page first (what the user is looking at), then walks older pages.
    ///
    /// When an earlier walk already covered a span, only the gap above it is fetched, then the
    /// walk jumps below that span's oldest message and continues to the beginning.
    ///
    /// Implicit conversation-open backfills update internal progress state quietly (visible via
    /// `QueryBackfillProgress`) without flooding `Event::BackfillProgress` listeners.
    pub async fn run(self, channel: ChannelId) {
        self.run_inner(channel, None, false, false).await;
    }

    /// Explicit archive backfill for a single channel, optionally walking `conversations.replies`
    /// for every thread root encountered (`include_threads`) and emitting live
    /// [`Event::BackfillProgress`] updates.
    pub async fn run_with_options(
        self,
        channel: ChannelId,
        channel_name: Option<String>,
        include_threads: bool,
    ) {
        self.run_inner(channel, channel_name, include_threads, true)
            .await;
    }

    async fn run_inner(
        self,
        channel: ChannelId,
        channel_name: Option<String>,
        include_threads: bool,
        emit_progress: bool,
    ) {
        let _ = self
            .control
            .update_channel(&channel, channel_name.as_deref(), |c| {
                c.status = BackfillStatus::Running;
                c.last_error = None;
            });
        let snap = self.control.set_active_channel(Some(channel.clone()));
        if emit_progress {
            self.events.emit(Event::BackfillProgress(snap));
        }

        match self
            .walk(
                &channel,
                channel_name.as_deref(),
                include_threads,
                emit_progress,
            )
            .await
        {
            Ok(()) => {
                let done = self.control.finish_run(false);
                if emit_progress {
                    self.events.emit(Event::BackfillProgress(done));
                }
            }
            Err(e) => {
                let msg = e.to_string();
                let _ = self
                    .control
                    .update_channel(&channel, channel_name.as_deref(), |c| {
                        c.status = BackfillStatus::Error;
                        c.last_error = Some(msg);
                    });
                let done = self.control.finish_run(true);
                if emit_progress {
                    self.events.emit(Event::BackfillProgress(done));
                }
                self.events.report(&format!("backfill {channel}"), e);
            }
        }
    }

    /// Walk every conversation in `channels` sequentially into SQLite, reporting live per-channel
    /// and workspace-wide progress via [`Event::BackfillProgress`] and respecting pause/resume.
    pub async fn run_workspace(
        self,
        channels: Vec<(ChannelId, Option<String>)>,
        include_threads: bool,
    ) {
        let ws = self.workspace.clone();
        let msg_counts = with_store(&self.store, move |s| s.channel_message_counts(&ws))
            .await
            .unwrap_or_default();
        let spans = self.lock().clone();
        let initial = self
            .control
            .sync_inventory(&channels, &spans, &msg_counts, true);
        self.events.emit(Event::BackfillProgress(initial));

        let mut had_error = false;
        for (channel, name) in channels {
            self.control
                .wait_if_paused(&self.events, Some(&channel), true)
                .await;
            let active = self.control.set_active_channel(Some(channel.clone()));
            self.events.emit(Event::BackfillProgress(active));
            let started = self.control.update_channel(&channel, name.as_deref(), |c| {
                c.status = BackfillStatus::Running;
                c.last_error = None;
            });
            self.events.emit(Event::BackfillProgress(started));

            match self
                .walk(&channel, name.as_deref(), include_threads, true)
                .await
            {
                Ok(()) => {}
                Err(CoreError::Api(ApiError::AuthExpired(code))) => {
                    let _ = self.control.update_channel(&channel, name.as_deref(), |c| {
                        c.status = BackfillStatus::Error;
                        c.last_error = Some(code.clone());
                    });
                    let done = self.control.finish_run(true);
                    self.events.emit(Event::BackfillProgress(done));
                    self.events.emit(Event::AuthExpired(code));
                    return;
                }
                Err(CoreError::Cancelled) => {
                    let done = self.control.finish_run(false);
                    self.events.emit(Event::BackfillProgress(done));
                    return;
                }
                Err(e) => {
                    had_error = true;
                    let msg = e.to_string();
                    let err_snap = self.control.update_channel(&channel, name.as_deref(), |c| {
                        c.status = BackfillStatus::Error;
                        c.last_error = Some(msg);
                    });
                    self.events.emit(Event::BackfillProgress(err_snap));
                    self.events.report(&format!("backfill {channel}"), e);
                }
            }
        }

        let done = self.control.finish_run(had_error);
        self.events.emit(Event::BackfillProgress(done));
    }

    async fn walk(
        &self,
        channel: &ChannelId,
        channel_name: Option<&str>,
        include_threads: bool,
        emit_progress: bool,
    ) -> Result<(), CoreError> {
        let mut catching_up = self.lock().get(channel).cloned();
        let mut catch_up_newest: Option<Ts> = None;
        let mut latest: Option<Ts> = None;
        let mut cursor: Option<String> = None;
        let mut first = true;
        loop {
            self.control
                .wait_if_paused(&self.events, Some(channel), emit_progress)
                .await;
            if !std::mem::take(&mut first) {
                tokio::time::sleep(self.page_delay).await;
                self.control
                    .wait_if_paused(&self.events, Some(channel), emit_progress)
                    .await;
            }
            let page = self.fetch(channel, &latest, &cursor).await?;
            let next = page
                .next_cursor
                .clone()
                .filter(|c| Some(c) != cursor.as_ref());
            let newest = page.items.iter().map(|m| &m.ts).max().cloned();
            let oldest = page.items.iter().map(|m| &m.ts).min().cloned();
            let overlaps = catching_up
                .as_ref()
                .is_some_and(|prior| page.items.iter().any(|m| m.ts <= prior.newest));
            let page_count = page.items.len() as u64;
            let thread_roots: Vec<Ts> = if include_threads {
                page.items
                    .iter()
                    .filter(|m| m.reply_count.unwrap_or(0) > 0)
                    .map(|m| m.ts.clone())
                    .collect()
            } else {
                Vec::new()
            };
            self.store_page(channel, page.items).await?;
            if !self.events.emit(Event::StoreChanged {
                channel: channel.clone(),
            }) {
                return Ok(());
            }

            let mut page_threads = 0u64;
            let mut page_replies = 0u64;
            for thread_ts in thread_roots {
                self.control
                    .wait_if_paused(&self.events, Some(channel), emit_progress)
                    .await;
                tokio::time::sleep(self.page_delay).await;
                self.control
                    .wait_if_paused(&self.events, Some(channel), emit_progress)
                    .await;
                let stored_replies = self
                    .fetch_thread_counted(channel.clone(), thread_ts)
                    .await?;
                page_threads += 1;
                page_replies += stored_replies;
            }

            if let Some(prior) = &catching_up {
                catch_up_newest = catch_up_newest.or(newest);
                if !overlaps && next.is_some() {
                    let progress = self.control.update_channel(channel, channel_name, |c| {
                        c.pages_fetched += 1;
                        c.messages_stored += page_count;
                        c.threads_fetched += page_threads;
                        c.replies_stored += page_replies;
                        if let Some(n) = &catch_up_newest
                            && c.newest_ts.as_ref().is_none_or(|cur| n > cur)
                        {
                            c.newest_ts = Some(n.clone());
                        }
                    });
                    if emit_progress {
                        self.events.emit(Event::BackfillProgress(progress));
                    }
                    cursor = next;
                    continue;
                }
                // Contiguous with the earlier span now (or history ended above it).
                let mut span = prior.clone();
                if let Some(n) = catch_up_newest.take().filter(|n| *n > span.newest) {
                    span.newest = n;
                }
                span.complete |= next.is_none() && !overlaps;
                catching_up = None;
                self.lock().insert(channel.clone(), span.clone());
                self.persist_synced().await;
                let progress = self.control.update_channel(channel, channel_name, |c| {
                    c.pages_fetched += 1;
                    c.messages_stored += page_count;
                    c.threads_fetched += page_threads;
                    c.replies_stored += page_replies;
                    c.newest_ts = Some(span.newest.clone());
                    c.oldest_ts = Some(span.oldest.clone());
                    c.complete = span.complete;
                    if span.complete {
                        c.status = BackfillStatus::Complete;
                    }
                });
                if emit_progress {
                    self.events.emit(Event::BackfillProgress(progress));
                }
                if span.complete {
                    self.events.emit(Event::HistoryComplete {
                        channel: channel.clone(),
                    });
                    return Ok(());
                }
                latest = Some(span.oldest);
                cursor = None;
                continue;
            }

            let span_snapshot = {
                let mut synced = self.lock();
                match (synced.get_mut(channel), newest, oldest) {
                    (Some(span), _, Some(oldest)) if oldest < span.oldest => span.oldest = oldest,
                    (None, Some(newest), Some(oldest)) => {
                        synced.insert(
                            channel.clone(),
                            Synced {
                                newest,
                                oldest,
                                complete: false,
                            },
                        );
                    }
                    _ => {}
                }
                if next.is_none()
                    && let Some(span) = synced.get_mut(channel)
                {
                    span.complete = true;
                }
                synced.get(channel).cloned()
            };
            self.persist_synced().await;
            let progress = self.control.update_channel(channel, channel_name, |c| {
                c.pages_fetched += 1;
                c.messages_stored += page_count;
                c.threads_fetched += page_threads;
                c.replies_stored += page_replies;
                if let Some(span) = &span_snapshot {
                    c.newest_ts = Some(span.newest.clone());
                    c.oldest_ts = Some(span.oldest.clone());
                    c.complete = span.complete;
                }
                if next.is_none() {
                    c.complete = true;
                    c.status = BackfillStatus::Complete;
                }
            });
            if emit_progress {
                self.events.emit(Event::BackfillProgress(progress));
            }
            match next {
                Some(next) => cursor = Some(next),
                None => {
                    self.events.emit(Event::HistoryComplete {
                        channel: channel.clone(),
                    });
                    return Ok(());
                }
            }
        }
    }

    async fn persist_synced(&self) {
        let Ok(payload) = serde_json::to_string(&*self.lock()) else {
            return;
        };
        let ws = self.workspace.clone();
        if let Err(err) = with_store(&self.store, move |s| {
            s.set_local(&ws, BACKFILL_SYNCED_KEY, &payload)
        })
        .await
        {
            tracing::warn!(%err, "failed to persist backfill synced spans");
        }
    }

    async fn fetch(
        &self,
        channel: &ChannelId,
        latest: &Option<Ts>,
        cursor: &Option<String>,
    ) -> Result<Page<Message>, ApiError> {
        wait_out_rate_limits(|| {
            self.backend
                .history_page(channel.clone(), latest.clone(), cursor.clone())
        })
        .await
    }

    /// Fetches one thread's full history (`conversations.replies`) into the store: the root
    /// followed by every reply. Every page is committed and announced before the next is
    /// fetched, so a cancelled fetch keeps what it stored. Read-only on Slack: it never moves
    /// the user's read cursor.
    pub(crate) async fn fetch_thread(
        &self,
        channel: ChannelId,
        thread_ts: Ts,
    ) -> Result<(), CoreError> {
        self.fetch_thread_counted(channel, thread_ts).await?;
        Ok(())
    }

    async fn fetch_thread_counted(
        &self,
        channel: ChannelId,
        thread_ts: Ts,
    ) -> Result<u64, CoreError> {
        let mut cursor: Option<String> = None;
        let mut first = true;
        let mut stored = 0u64;
        loop {
            if !std::mem::take(&mut first) {
                tokio::time::sleep(self.page_delay).await;
            }
            let page = wait_out_rate_limits(|| {
                self.backend
                    .replies_page(channel.clone(), thread_ts.clone(), cursor.clone())
            })
            .await?;
            let next = page
                .next_cursor
                .filter(|next| Some(next) != cursor.as_ref());
            let replies_in_page = page.items.iter().filter(|m| m.ts != thread_ts).count() as u64;
            stored += replies_in_page;
            self.store_page(&channel, page.items).await?;
            if !self.events.emit(Event::StoreChanged {
                channel: channel.clone(),
            }) {
                return Ok(stored);
            }
            match next {
                Some(next) => cursor = Some(next),
                None => return Ok(stored),
            }
        }
    }

    async fn store_page(&self, channel: &ChannelId, items: Vec<Message>) -> Result<(), CoreError> {
        self.unfurls.resolve(items.iter());
        let (ws, channel) = (self.workspace.clone(), channel.clone());
        with_store(&self.store, move |store| {
            store.upsert_messages(&ws, &channel, &items)
        })
        .await?;
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ChannelId, Synced>> {
        self.synced.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Runs `fetch` until it stops hitting a rate limit, waiting Slack's `retry_after` each time
/// (on top of the API client's own retries; at most [`MAX_RATE_LIMIT_WAITS`] waits).
pub(crate) async fn wait_out_rate_limits<T, F, Fut>(mut fetch: F) -> Result<T, ApiError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ApiError>>,
{
    let mut waits = 0;
    loop {
        match fetch().await {
            Err(ApiError::RateLimited { retry_after }) if waits < MAX_RATE_LIMIT_WAITS => {
                waits += 1;
                tokio::time::sleep(retry_after).await;
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::tests::{FakeBackend, fake_core_parts};
    use crate::store::Store;
    use futures::StreamExt;

    fn backfill(
        fake: &FakeBackend,
    ) -> (
        Backfill<FakeBackend>,
        futures::channel::mpsc::UnboundedReceiver<Event>,
    ) {
        let (store, events, rx) = fake_core_parts();
        let backfill = Backfill {
            backend: fake.clone(),
            store: Arc::clone(&store),
            workspace: TeamId::from("T1"),
            events: events.clone(),
            synced: SyncedMap::default(),
            page_delay: PAGE_DELAY,
            unfurls: Unfurls::new(
                fake.clone(),
                store,
                TeamId::from("T1"),
                events,
                crate::client::TaskTracker::default(),
            ),
            control: BackfillControl::new(TeamId::from("T1")),
        };
        (backfill, rx)
    }

    fn stored_ts(store: &SharedStore) -> Vec<String> {
        let store: std::sync::MutexGuard<'_, Store> = store.lock().unwrap();
        store
            .channel_history(&TeamId::from("T1"), &ChannelId::from("C1"), None, 1000)
            .unwrap()
            .into_iter()
            .map(|m| m.message.ts.0)
            .collect()
    }

    fn drain(rx: &mut futures::channel::mpsc::UnboundedReceiver<Event>) -> Vec<Event> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    #[tokio::test(start_paused = true)]
    async fn walks_every_page_into_the_store() {
        let fake = FakeBackend::with_history(1..=7);
        let (bf, mut rx) = backfill(&fake);
        bf.clone().run(ChannelId::from("C1")).await;

        assert_eq!(
            stored_ts(&bf.store),
            (1..=7).map(|i| format!("{i}.0")).collect::<Vec<_>>()
        );
        assert_eq!(fake.calls().len(), 4, "pages of 2");
        let events = drain(&mut rx);
        assert!(events.iter().any(|e| {
            matches!(
                e,
                Event::StoreChanged { channel } if *channel == ChannelId::from("C1")
            )
        }));
        assert!(events.iter().any(|e| {
            matches!(
                e,
                Event::HistoryComplete { channel } if *channel == ChannelId::from("C1")
            )
        }));
        let span = bf.synced.lock().unwrap()[&ChannelId::from("C1")].clone();
        assert_eq!(
            span,
            Synced {
                newest: Ts::from("7.0"),
                oldest: Ts::from("1.0"),
                complete: true
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn waits_out_rate_limits() {
        let fake = FakeBackend::with_history(1..=3);
        fake.rate_limit_next(2);
        let (bf, mut rx) = backfill(&fake);
        bf.clone().run(ChannelId::from("C1")).await;
        assert_eq!(stored_ts(&bf.store).len(), 3);
        assert!(drain(&mut rx).iter().all(|e| !matches!(e, Event::Error(_))));
    }

    #[tokio::test(start_paused = true)]
    async fn rerun_fetches_only_the_gap_above_a_complete_span() {
        let fake = FakeBackend::with_history(1..=7);
        let (bf, _rx) = backfill(&fake);
        bf.clone().run(ChannelId::from("C1")).await;
        fake.push_history(8..=10);
        fake.clear_calls();

        bf.clone().run(ChannelId::from("C1")).await;
        assert_eq!(stored_ts(&bf.store).len(), 10);
        // Page 1: 10, 9. Page 2: 8, 7 -> overlaps the stored span, stop.
        assert_eq!(fake.calls().len(), 2);
        assert_eq!(
            bf.synced.lock().unwrap()[&ChannelId::from("C1")].newest,
            Ts::from("10.0")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn interrupted_walk_resumes_below_the_known_span() {
        let fake = FakeBackend::with_history(1..=9);
        let (bf, _rx) = backfill(&fake);
        // An earlier walk stored 6..=9 and was cancelled.
        {
            let mut store = bf.store.lock().unwrap();
            for m in fake.messages().iter().filter(|m| m.ts >= Ts::from("6.0")) {
                store
                    .upsert_message(&bf.workspace, &ChannelId::from("C1"), m)
                    .unwrap();
            }
        }
        bf.synced.lock().unwrap().insert(
            ChannelId::from("C1"),
            Synced {
                newest: Ts::from("9.0"),
                oldest: Ts::from("6.0"),
                complete: false,
            },
        );

        bf.clone().run(ChannelId::from("C1")).await;
        assert_eq!(stored_ts(&bf.store).len(), 9);
        let calls = fake.calls();
        // Newest page overlaps immediately, then the walk jumps below 6.0.
        assert_eq!(calls[0].0, None);
        assert_eq!(calls[1], (Some(Ts::from("6.0")), None));
        assert!(bf.synced.lock().unwrap()[&ChannelId::from("C1")].complete);
    }

    #[tokio::test(start_paused = true)]
    async fn aborting_the_task_cancels_the_walk() {
        let fake = FakeBackend::with_history(1..=40);
        let (bf, mut events) = backfill(&fake);
        let task = tokio::spawn(bf.clone().run(ChannelId::from("C1")));
        // Paused time: the walk parks in its first inter-page delay once page 1 is stored.
        loop {
            match events.next().await {
                Some(Event::StoreChanged { .. }) => break,
                Some(_) => continue,
                None => panic!("expected StoreChanged"),
            }
        }
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(stored_ts(&bf.store).len() < 40);
    }

    #[tokio::test(start_paused = true)]
    async fn history_pages_name_the_channels_their_unfurls_point_at() {
        use crate::types::{Attachment, Conversation};

        let fake = FakeBackend::default();
        fake.set_info(Conversation {
            name: Some("third-line".into()),
            is_archived: true,
            ..Default::default()
        });
        fake.push_history(1..=1);
        fake.push_message(Message {
            ts: Ts::from("2.0"),
            text: "look at this".into(),
            attachments: vec![Attachment {
                is_msg_unfurl: true,
                channel_id: Some(ChannelId::from("CEXT")),
                from_url: Some("https://acme.slack.com/archives/CEXT/p1".into()),
                ..Default::default()
            }],
            ..Default::default()
        });
        let (bf, mut rx) = backfill(&fake);
        bf.clone().run(ChannelId::from("C1")).await;

        loop {
            match tokio::time::timeout(Duration::from_secs(5), rx.next()).await {
                Ok(Some(Event::Conversations(ids))) if ids.contains(&ChannelId::from("CEXT")) => {
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => panic!("the unfurl conversation is never announced"),
            }
        }
        let stored = bf
            .store
            .lock()
            .unwrap()
            .conversation(&bf.workspace, &ChannelId::from("CEXT"))
            .unwrap()
            .expect("the foreign channel lands in the store");
        assert_eq!(stored.name.as_deref(), Some("third-line"));
        assert!(stored.is_archived, "the archived flag survives");
        assert!(
            !stored.is_member,
            "a named reference never joins the sidebar"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stuck_cursor_guard_terminates_walk_and_thread_fetch() {
        let fake = FakeBackend::with_history(1..=6);
        fake.push_thread("6.0", 1..=6);
        fake.set_stuck_cursor(true);
        let (bf, mut rx) = backfill(&fake);

        bf.clone().run(ChannelId::from("C1")).await;
        assert_eq!(fake.calls().len(), 2);
        assert!(drain(&mut rx).iter().any(|e| {
            matches!(
                e,
                Event::HistoryComplete { channel } if *channel == ChannelId::from("C1")
            )
        }));

        bf.fetch_thread(ChannelId::from("C1"), Ts::from("6.0"))
            .await
            .unwrap();
        assert_eq!(fake.reply_calls().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limit_exhaustion_after_five_waits_reports_error() {
        let fake = FakeBackend::with_history(1..=3);
        fake.rate_limit_next(6);
        let (bf, mut rx) = backfill(&fake);

        bf.clone().run(ChannelId::from("C1")).await;
        assert_eq!(fake.calls().len(), 6, "initial try + 5 waits");
        assert!(stored_ts(&bf.store).is_empty());
        assert!(
            drain(&mut rx)
                .iter()
                .any(|e| matches!(e, Event::Error(msg) if msg.contains("backfill C1")))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn multi_page_catchup_over_incomplete_span_bridges_gap_and_finishes() {
        let fake = FakeBackend::with_history(1..=12);
        let (bf, mut rx) = backfill(&fake);
        // Pre-seed incomplete middle span 5..=6.
        {
            let mut store = bf.store.lock().unwrap();
            for m in fake
                .messages()
                .iter()
                .filter(|m| m.ts >= Ts::from("5.0") && m.ts <= Ts::from("6.0"))
            {
                store
                    .upsert_message(&bf.workspace, &ChannelId::from("C1"), m)
                    .unwrap();
            }
        }
        bf.synced.lock().unwrap().insert(
            ChannelId::from("C1"),
            Synced {
                newest: Ts::from("6.0"),
                oldest: Ts::from("5.0"),
                complete: false,
            },
        );

        bf.clone().run(ChannelId::from("C1")).await;
        assert_eq!(stored_ts(&bf.store).len(), 12);
        let calls = fake.calls();
        assert!(
            calls
                .iter()
                .any(|(latest, _)| *latest == Some(Ts::from("5.0")))
        );
        let span = bf.synced.lock().unwrap()[&ChannelId::from("C1")].clone();
        assert_eq!(
            span,
            Synced {
                newest: Ts::from("12.0"),
                oldest: Ts::from("1.0"),
                complete: true,
            }
        );
        assert!(drain(&mut rx).iter().any(|e| {
            matches!(
                e,
                Event::HistoryComplete { channel } if *channel == ChannelId::from("C1")
            )
        }));
    }

    #[tokio::test(start_paused = true)]
    async fn empty_first_page_completes_cleanly() {
        let fake = FakeBackend::default();
        let (bf, mut rx) = backfill(&fake);

        bf.clone().run(ChannelId::from("C1")).await;
        assert_eq!(fake.calls().len(), 1);
        assert!(stored_ts(&bf.store).is_empty());
        let events = drain(&mut rx);
        assert!(events.contains(&Event::StoreChanged {
            channel: ChannelId::from("C1")
        }));
        assert!(events.contains(&Event::HistoryComplete {
            channel: ChannelId::from("C1")
        }));
    }

    #[tokio::test(start_paused = true)]
    async fn receiver_drop_exits_walk_and_thread_fetch_early() {
        let fake = FakeBackend::with_history(1..=10);
        fake.push_thread("10.0", 1..=10);
        let (bf, rx) = backfill(&fake);
        drop(rx);

        bf.clone().run(ChannelId::from("C1")).await;
        assert_eq!(
            fake.calls().len(),
            1,
            "walk stops after first page once receiver drops"
        );

        bf.fetch_thread(ChannelId::from("C1"), Ts::from("10.0"))
            .await
            .unwrap();
        assert_eq!(
            fake.reply_calls().len(),
            1,
            "thread fetch stops after first page once receiver drops"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn synced_spans_persist_in_local_state_across_restarts() {
        let fake = FakeBackend::with_history(1..=7);
        let (bf, _rx) = backfill(&fake);
        bf.clone().run(ChannelId::from("C1")).await;
        assert_eq!(fake.calls().len(), 4);

        let persisted_raw = bf
            .store
            .lock()
            .expect("store lock")
            .local(&bf.workspace, BACKFILL_SYNCED_KEY)
            .expect("local read");
        let restored = decode_synced_spans(persisted_raw);
        assert_eq!(
            restored.get(&ChannelId::from("C1")),
            Some(&Synced {
                newest: Ts::from("7.0"),
                oldest: Ts::from("1.0"),
                complete: true,
            })
        );

        fake.push_history(8..=8);
        fake.clear_calls();
        let (events2, _rx2) = {
            let (tx, rx) = futures::channel::mpsc::unbounded();
            (Emitter(tx), rx)
        };
        let bf2 = Backfill {
            backend: fake.clone(),
            store: Arc::clone(&bf.store),
            workspace: bf.workspace.clone(),
            events: events2.clone(),
            synced: Arc::new(Mutex::new(restored)),
            page_delay: PAGE_DELAY,
            unfurls: Unfurls::new(
                fake.clone(),
                Arc::clone(&bf.store),
                bf.workspace.clone(),
                events2,
                crate::client::TaskTracker::default(),
            ),
            control: BackfillControl::new(bf.workspace.clone()),
        };
        bf2.run(ChannelId::from("C1")).await;
        assert_eq!(fake.calls().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn backfill_with_threads_walks_replies_and_reports_progress() {
        let fake = FakeBackend::with_history(1..=2);
        fake.push_message(Message {
            ts: Ts::from("3.0"),
            text: "thread root".into(),
            reply_count: Some(2),
            ..Default::default()
        });
        fake.push_thread("3.0", 4..=5);
        let (bf, mut rx) = backfill(&fake);

        bf.clone()
            .run_with_options(ChannelId::from("C1"), Some("general".into()), true)
            .await;

        // Thread replies landed in SQLite alongside channel history.
        let replies = bf
            .store
            .lock()
            .unwrap()
            .thread_replies(
                &TeamId::from("T1"),
                &ChannelId::from("C1"),
                &Ts::from("3.0"),
            )
            .unwrap();
        assert_eq!(replies.len(), 3, "root + 2 replies");

        let progress = bf.control.snapshot();
        assert_eq!(progress.status, BackfillStatus::Complete);
        assert_eq!(progress.threads_fetched, 1);
        assert_eq!(progress.replies_stored, 2);
        assert_eq!(progress.completed_channels, 1);
        assert!(drain(&mut rx).iter().any(|e| {
            matches!(
                e,
                Event::BackfillProgress(p) if p.status == BackfillStatus::Complete && p.threads_fetched == 1
            )
        }));
    }

    #[tokio::test(start_paused = true)]
    async fn pause_and_resume_suspends_and_completes_workspace_backfill() {
        let fake = FakeBackend::with_history(1..=6);
        let (bf, mut rx) = backfill(&fake);
        let control = bf.control.clone();
        control.pause();
        assert!(control.is_paused());

        let task = tokio::spawn(
            bf.clone()
                .run_workspace(vec![(ChannelId::from("C1"), Some("general".into()))], false),
        );

        // Wait until the worker emits its Paused progress event.
        loop {
            match tokio::time::timeout(Duration::from_secs(5), rx.next()).await {
                Ok(Some(Event::BackfillProgress(p))) if p.status == BackfillStatus::Paused => {
                    assert!(p.paused);
                    break;
                }
                Ok(Some(_)) => continue,
                other => panic!("expected Paused BackfillProgress, got {other:?}"),
            }
        }
        assert!(fake.calls().is_empty(), "must not fetch pages while paused");

        control.resume();
        task.await.unwrap();
        let snap = control.snapshot();
        assert_eq!(snap.status, BackfillStatus::Complete);
        assert!(!snap.paused);
        assert_eq!(snap.completed_channels, 1);
        assert_eq!(stored_ts(&bf.store).len(), 6);
    }
}
