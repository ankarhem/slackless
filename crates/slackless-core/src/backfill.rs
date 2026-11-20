//! Paged `conversations.*` fetches into the store: background history backfill walks
//! `conversations.history` so a conversation's history stays available locally even after
//! Slack stops serving it, and thread fetches walk `conversations.replies` for one thread's
//! full history on demand.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::Event;
use crate::api::{ApiError, Page};
use crate::backend::Backend;
use crate::client::{CoreError, Emitter, SharedStore, with_store};
use crate::types::{ChannelId, Message, TeamId, Ts};
use crate::unfurl::Unfurls;

/// Pause between history pages. `conversations.history` is Tier 3 (~50 requests/minute).
pub(crate) const PAGE_DELAY: Duration = Duration::from_millis(1200);
/// How many times one page waits out a rate limit (on top of the API client's own retries).
const MAX_RATE_LIMIT_WAITS: u32 = 5;

/// The contiguous span of a conversation's channel-level history known to be in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Synced {
    pub newest: Ts,
    pub oldest: Ts,
    /// `oldest` is the beginning of the conversation.
    pub complete: bool,
}

pub(crate) type SyncedMap = Arc<Mutex<HashMap<ChannelId, Synced>>>;

/// One backfill worker configuration; [`Backfill::run`] walks one conversation. Cancel by
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
}

impl<B: Backend> Backfill<B> {
    /// Fetches the newest page first (what the user is looking at), then walks older pages.
    ///
    /// When an earlier walk already covered a span, only the gap above it is fetched, then the
    /// walk jumps below that span's oldest message and continues to the beginning.
    pub async fn run(self, channel: ChannelId) {
        if let Err(e) = self.walk(&channel).await {
            self.events.report(&format!("backfill {channel}"), e);
        }
    }

    async fn walk(&self, channel: &ChannelId) -> Result<(), CoreError> {
        let mut catching_up = self.lock().get(channel).cloned();
        let mut catch_up_newest: Option<Ts> = None;
        let mut latest: Option<Ts> = None;
        let mut cursor: Option<String> = None;
        let mut first = true;
        loop {
            if !std::mem::take(&mut first) {
                tokio::time::sleep(self.page_delay).await;
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
            self.store_page(channel, page.items).await?;
            if !self.events.emit(Event::StoreChanged {
                channel: channel.clone(),
            }) {
                return Ok(());
            }

            if let Some(prior) = &catching_up {
                catch_up_newest = catch_up_newest.or(newest);
                if !overlaps && next.is_some() {
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

            {
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
                if next.is_none() {
                    if let Some(span) = synced.get_mut(channel) {
                        span.complete = true;
                    }
                }
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
        let mut cursor: Option<String> = None;
        loop {
            let page = wait_out_rate_limits(|| {
                self.backend
                    .replies_page(channel.clone(), thread_ts.clone(), cursor.clone())
            })
            .await?;
            let next = page
                .next_cursor
                .filter(|next| Some(next) != cursor.as_ref());
            self.unfurls.resolve(page.items.iter());
            let (ws, target) = (self.workspace.clone(), channel.clone());
            with_store(&self.store, move |store| {
                page.items
                    .iter()
                    .try_for_each(|message| store.upsert_message(&ws, &target, message))
            })
            .await?;
            if !self.events.emit(Event::StoreChanged {
                channel: channel.clone(),
            }) {
                return Ok(());
            }
            match next {
                Some(next) => cursor = Some(next),
                None => return Ok(()),
            }
        }
    }

    async fn store_page(&self, channel: &ChannelId, items: Vec<Message>) -> Result<(), CoreError> {
        self.unfurls.resolve(items.iter());
        let (ws, channel) = (self.workspace.clone(), channel.clone());
        with_store(&self.store, move |store| {
            items
                .iter()
                .try_for_each(|m| store.upsert_message(&ws, &channel, m))
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
            unfurls: Unfurls::new(fake.clone(), store, TeamId::from("T1"), events),
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
        assert_eq!(
            events.first(),
            Some(&Event::StoreChanged {
                channel: ChannelId::from("C1")
            })
        );
        assert_eq!(
            events.last(),
            Some(&Event::HistoryComplete {
                channel: ChannelId::from("C1")
            })
        );
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
        assert!(matches!(
            events.next().await,
            Some(Event::StoreChanged { .. })
        ));
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

        // The walk stored its page; the spawned unfurl resolution then announces the foreign
        // conversation it looked up.
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
}
