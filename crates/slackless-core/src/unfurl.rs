//! Names for the conversations shared-message unfurls point at but the store does not know.
//!
//! A shared-message unfurl (`attachments[].is_msg_unfurl`) carries the `channel_id` of the
//! conversation the shared message lives in. That conversation is usually foreign to the
//! signed-in user (an archived channel of another workspace), so nothing else ever stores it;
//! without a row the UI cannot render "Thread in <name>". Storing messages collects the ids,
//! and this resolver asks Slack for each once per session in the background.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use crate::Event;
use crate::api::ApiError;
use crate::backend::Backend;
use crate::backfill::{PAGE_DELAY, wait_out_rate_limits};
use crate::client::{CoreError, Emitter, SharedStore, TaskTracker, with_store};
use crate::types::event::MessageEvent;
use crate::types::{ChannelId, Message, SlackEvent, TeamId};

/// Resolves unfurled conversation ids through `conversations.info`, once per session.
///
/// Ids are claimed the moment they are collected, whether the lookup then succeeds, fails
/// (`channel_not_found`, no access) or turns out unnecessary: a failure is a result, so it is
/// never retried while the session lives.
#[derive(Clone)]
pub(crate) struct Unfurls<B: Backend> {
    backend: B,
    store: SharedStore,
    workspace: TeamId,
    events: Emitter,
    /// Ids already claimed this session (resolved, skipped or failed).
    asked: Arc<Mutex<HashSet<ChannelId>>>,
    tasks: TaskTracker,
}

impl<B: Backend> Unfurls<B> {
    /// A resolver sharing `asked` with every clone handed to the core's other workers.
    pub(crate) fn new(
        backend: B,
        store: SharedStore,
        workspace: TeamId,
        events: Emitter,
        tasks: TaskTracker,
    ) -> Self {
        Self {
            backend,
            store,
            workspace,
            events,
            asked: Arc::new(Mutex::new(HashSet::new())),
            tasks,
        }
    }

    /// Collects the unfurled ids of `messages`, claims the unseen ones and resolves them in
    /// the background; the caller never waits.
    pub(crate) fn resolve<'a>(&self, messages: impl IntoIterator<Item = &'a Message>) {
        if let Some(claimed) = self.claim(messages) {
            let this = self.clone();
            self.tasks.spawn(async move {
                if let Err(e) = this.walk(claimed).await {
                    this.events.report("unfurl conversations.info", e);
                }
            });
        }
    }

    /// As [`Unfurls::resolve`], awaited: tests drive it deterministically.
    #[cfg(test)]
    pub(crate) async fn resolve_now<'a>(
        &self,
        messages: impl IntoIterator<Item = &'a Message>,
    ) -> Result<(), CoreError> {
        match self.claim(messages) {
            Some(claimed) => self.walk(claimed).await,
            None => Ok(()),
        }
    }

    /// The not-yet-asked unfurled ids of `messages`, freshly claimed; `None` when there is
    /// nothing new to do.
    fn claim<'a>(&self, messages: impl IntoIterator<Item = &'a Message>) -> Option<Vec<ChannelId>> {
        let mut targets: Vec<ChannelId> = Vec::new();
        for message in messages {
            for attachment in &message.attachments {
                if attachment.is_msg_unfurl
                    && let Some(id) = &attachment.channel_id
                    && !targets.contains(id)
                {
                    targets.push(id.clone());
                }
            }
        }
        let mut asked = self.asked.lock().unwrap_or_else(PoisonError::into_inner);
        let claimed: Vec<ChannelId> = targets
            .into_iter()
            .filter(|id| asked.insert(id.clone()))
            .collect();
        (!claimed.is_empty()).then_some(claimed)
    }

    /// One `conversations.info` per id not yet in the store, paced at the Tier-3 rate; the
    /// result is stored as a non-member conversation so it can be named but never joins the
    /// sidebar. Slack-level failures (`channel_not_found`, ...) are final and silent: no name
    /// exists, and the claim already remembers that. Transient errors or cancellation release
    /// any unattempted/transiently-failed ids from `asked` so they are never permanently poisoned.
    async fn walk(&self, ids: Vec<ChannelId>) -> Result<(), CoreError> {
        struct UnfinishedGuard {
            asked: Arc<Mutex<HashSet<ChannelId>>>,
            ids: Vec<ChannelId>,
            completed: usize,
        }
        impl Drop for UnfinishedGuard {
            fn drop(&mut self) {
                if self.completed < self.ids.len() {
                    let mut asked = self.asked.lock().unwrap_or_else(PoisonError::into_inner);
                    for id in &self.ids[self.completed..] {
                        asked.remove(id);
                    }
                }
            }
        }

        let mut guard = UnfinishedGuard {
            asked: Arc::clone(&self.asked),
            ids,
            completed: 0,
        };
        for n in 0..guard.ids.len() {
            if n > 0 {
                tokio::time::sleep(PAGE_DELAY).await;
            }
            let id = guard.ids[n].clone();
            let (ws, target) = (self.workspace.clone(), id.clone());
            let stored = with_store(&self.store, move |s| s.conversation(&ws, &target)).await?;
            if stored.is_some() {
                guard.completed = n + 1;
                continue;
            }
            match wait_out_rate_limits(|| self.backend.conversation_info(id.clone())).await {
                Ok(mut conversation) => {
                    conversation.is_member = false;
                    let (ws, conversation) = (self.workspace.clone(), conversation);
                    with_store(&self.store, move |s| {
                        s.upsert_conversation(&ws, &conversation)
                    })
                    .await?;
                    self.events.emit(Event::Conversations(vec![id]));
                    guard.completed = n + 1;
                }
                Err(ApiError::Slack(_)) => {
                    guard.completed = n + 1;
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}

/// The messages a websocket event carries (new, edited or thread-root): the only payloads an
/// unfurl can arrive in.
pub(crate) fn event_messages(event: &SlackEvent) -> Vec<&Message> {
    match event {
        SlackEvent::Message(ev) => match ev {
            MessageEvent::Message(m) => vec![m.as_ref()],
            MessageEvent::Changed(c) => vec![c.message.as_ref()],
            MessageEvent::Replied(r) => r.message.as_deref().into_iter().collect(),
            MessageEvent::Deleted(_) => Vec::new(),
        },
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::tests::{FakeBackend, fake_core_parts};
    use crate::types::{Attachment, Conversation, Ts};

    /// A message sharing one thread of `channel`.
    fn unfurling(ts: &str, channel: &str) -> Message {
        Message {
            ts: Ts::from(ts),
            text: "look at this".into(),
            attachments: vec![Attachment {
                is_msg_unfurl: true,
                channel_id: Some(ChannelId::from(channel)),
                from_url: Some(format!("https://acme.slack.com/archives/{channel}/p1")),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn unfurls(
        fake: &FakeBackend,
        store: SharedStore,
        events: Emitter,
    ) -> (Unfurls<FakeBackend>, SharedStore) {
        let (store, events) = (store, events);
        (
            Unfurls {
                backend: fake.clone(),
                store: Arc::clone(&store),
                workspace: TeamId::from("T1"),
                events,
                asked: Arc::new(Mutex::new(HashSet::new())),
                tasks: TaskTracker::default(),
            },
            store,
        )
    }
    #[tokio::test]
    async fn resolves_foreign_channels_once_as_non_members() {
        let fake = FakeBackend::default();
        fake.set_info(Conversation {
            name: Some("third-line".into()),
            is_archived: true,
            ..Default::default()
        });
        let (store, events, _rx) = fake_core_parts();
        let (unfurls, store) = unfurls(&fake, store, events);
        let ws = TeamId::from("T1");
        // A conversation the store already knows never needs asking.
        store
            .lock()
            .unwrap()
            .upsert_conversation(
                &ws,
                &Conversation {
                    id: ChannelId::from("CKNOWN"),
                    name: Some("known".into()),
                    ..Default::default()
                },
            )
            .unwrap();

        let shared = unfurling("5.0", "CEXT1");
        let plain_unfurling = unfurling("6.0", "CKNOWN");
        unfurls
            .resolve_now([&shared, &plain_unfurling])
            .await
            .unwrap();

        assert_eq!(
            fake.info_calls(),
            vec![ChannelId::from("CEXT1")],
            "only the foreign, unknown channel is asked for"
        );
        let stored = store
            .lock()
            .unwrap()
            .conversation(&ws, &ChannelId::from("CEXT1"))
            .unwrap()
            .expect("the foreign channel lands in the store");
        assert_eq!(stored.name.as_deref(), Some("third-line"));
        assert!(stored.is_archived, "the info answer is kept verbatim");
        assert!(
            !stored.is_member,
            "a named reference is never a sidebar row"
        );

        // The same unfurl arrives again (a replayed history page, a refresh): asked once.
        unfurls.resolve_now([&shared]).await.unwrap();
        assert_eq!(fake.info_calls().len(), 1);
    }

    #[tokio::test]
    async fn failed_lookups_are_remembered_not_retried() {
        let fake = FakeBackend::default();
        fake.fail_info();
        let (store, events, _rx) = fake_core_parts();
        let (unfurls, store) = unfurls(&fake, store, events);

        let gone = unfurling("5.0", "CGONE");
        unfurls.resolve_now([&gone]).await.unwrap();
        assert_eq!(fake.info_calls(), vec![ChannelId::from("CGONE")]);

        // The next refresh replays the same message: no second ask, even now that lookups
        // would succeed.
        fake.set_info(Conversation {
            name: Some("too late".into()),
            ..Default::default()
        });
        unfurls.resolve_now([&gone]).await.unwrap();
        assert_eq!(fake.info_calls().len(), 1, "the failure is remembered");
        assert!(
            store
                .lock()
                .unwrap()
                .conversation(&TeamId::from("T1"), &ChannelId::from("CGONE"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn event_messages_covers_the_unfurl_carrying_events() {
        let event: SlackEvent = serde_json::from_str(
            r#"{"type":"message","channel":"C1","user":"U1","ts":"5.0","text":"x",
                "attachments":[{"is_msg_unfurl":true,"channel_id":"CEXT"}]}"#,
        )
        .unwrap();
        assert_eq!(event_messages(&event).len(), 1);
        let changed: SlackEvent = serde_json::from_str(
            r#"{"type":"message","subtype":"message_changed","channel":"C1",
                "message":{"ts":"5.0","text":"edited",
                    "attachments":[{"is_msg_unfurl":true,"channel_id":"CEXT"}]},
                "event_ts":"6.0"}"#,
        )
        .unwrap();
        assert_eq!(event_messages(&changed).len(), 1);
        let replied: SlackEvent = serde_json::from_str(
            r#"{"type":"message","subtype":"message_replied","channel":"C1",
                "message":{"ts":"5.0","text":"root"}}"#,
        )
        .unwrap();
        assert_eq!(event_messages(&replied).len(), 1);
        let replied_empty: SlackEvent = serde_json::from_str(
            r#"{"type":"message","subtype":"message_replied","channel":"C1"}"#,
        )
        .unwrap();
        assert!(event_messages(&replied_empty).is_empty());
        let deleted: SlackEvent = serde_json::from_str(
            r#"{"type":"message","subtype":"message_deleted","channel":"C1","deleted_ts":"5.0"}"#,
        )
        .unwrap();
        assert!(event_messages(&deleted).is_empty());
        let other: SlackEvent =
            serde_json::from_str(r#"{"type":"channel_rename","channel":{"id":"C1"}}"#).unwrap();
        assert!(event_messages(&other).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn transient_partial_failure_does_not_poison_unattempted_ids() {
        let fake = FakeBackend::default();
        fake.set_info(Conversation {
            name: Some("foreign".into()),
            ..Default::default()
        });
        fake.fail_info_transient_for(ChannelId::from("CEXT1"));
        let (store, events, _rx) = fake_core_parts();
        let (unfurls, store) = unfurls(&fake, store, events);

        let m1 = unfurling("5.0", "CEXT1");
        let m2 = unfurling("6.0", "CEXT2");
        assert!(unfurls.resolve_now([&m1, &m2]).await.is_err());
        let calls = fake.info_calls();
        assert!(
            !calls.is_empty() && calls.iter().all(|c| *c == ChannelId::from("CEXT1")),
            "rate-limit retries stay on CEXT1 and exit before attempting CEXT2: {calls:?}"
        );

        // Once the transient outage clears, retrying resolves BOTH CEXT1 and CEXT2.
        fake.clear_fail_info_transient();
        unfurls.resolve_now([&m1, &m2]).await.unwrap();
        let ws = TeamId::from("T1");
        assert!(
            store
                .lock()
                .unwrap()
                .conversation(&ws, &ChannelId::from("CEXT1"))
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .lock()
                .unwrap()
                .conversation(&ws, &ChannelId::from("CEXT2"))
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn ignores_non_msg_unfurls_and_deduplicates_within_batch() {
        let fake = FakeBackend::default();
        let (store, events, _rx) = fake_core_parts();
        let (unfurls, _store) = unfurls(&fake, store, events);

        let non_unfurl = Message {
            ts: Ts::from("1.0"),
            attachments: vec![Attachment {
                is_msg_unfurl: false,
                channel_id: Some(ChannelId::from("CIGNORED")),
                ..Default::default()
            }],
            ..Default::default()
        };
        let dup1 = unfurling("2.0", "CDUP");
        let dup2 = unfurling("3.0", "CDUP");
        unfurls
            .resolve_now([&non_unfurl, &dup1, &dup2])
            .await
            .unwrap();
        assert_eq!(fake.info_calls(), vec![ChannelId::from("CDUP")]);
    }
}
