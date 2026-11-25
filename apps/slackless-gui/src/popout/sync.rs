//! Store reload, revision watching, fixture seeding, and huddle thread synchronization for
//! popped-out secondary windows.

use gpui_kit::gpui::Context;
use slackless_core::MessagingCommand;
use slackless_core::types::Message;

use crate::fixtures::Fixtures;

use super::{HuddleDrawer, PopoutKind, PopoutWindow};

impl PopoutWindow {
    pub(super) fn reload_from_store(&mut self, cx: &mut Context<Self>) {
        match &self.kind {
            PopoutKind::Activity { source, .. } => {
                if let Some(feed) = source.upgrade() {
                    self.activity_rows = feed.read(cx).snapshot_for_popout();
                    cx.notify();
                }
                return;
            }
            PopoutKind::Later { source, .. } => {
                if let Some(later) = source.upgrade() {
                    self.later_items = later.read(cx).items_for_popout();
                    cx.notify();
                }
                return;
            }
            PopoutKind::Files { source, .. } => {
                if let Some(page) = source.upgrade() {
                    self.files_items = page.read(cx).snapshot_for_popout();
                    cx.notify();
                }
                return;
            }
            PopoutKind::Huddle { source, .. } => {
                if let Some(state_entity) = source.upgrade() {
                    self.huddle_state = Some(state_entity.read(cx).view_state());
                }
                if self.huddle_drawer == Some(HuddleDrawer::Thread) {
                    self.refresh_huddle_thread_async(cx);
                    self.maybe_fetch_huddle_thread();
                    cx.notify();
                }
                return;
            }
            PopoutKind::Canvas { .. } => return,
            PopoutKind::Conversation { .. } | PopoutKind::Thread { .. } => {}
        }
        if let (Some(store), Some(team)) = (self.store.clone(), self.team.clone()) {
            let query = match &self.kind {
                PopoutKind::Conversation { channel, .. } => (channel.clone(), None),
                PopoutKind::Thread {
                    channel, thread_ts, ..
                } => (channel.clone(), Some(thread_ts.clone())),
                _ => return,
            };
            self.reload_gen = self.reload_gen.wrapping_add(1);
            let generation = self.reload_gen;
            cx.spawn(async move |this, cx| {
                let fresh = cx
                    .background_executor()
                    .spawn(async move {
                        let Ok(guard) = store.lock() else {
                            return None;
                        };
                        let rows = match query.1 {
                            Some(thread_ts) => guard.thread_replies(&team, &query.0, &thread_ts),
                            None => guard.channel_history(&team, &query.0, None, 200),
                        };
                        rows.ok().map(|list| {
                            list.into_iter()
                                .filter(|r| !r.deleted)
                                .map(|r| r.message)
                                .collect::<Vec<_>>()
                        })
                    })
                    .await;
                if let Some(fresh) = fresh {
                    let _ = this.update(cx, |win, cx| {
                        if win.reload_gen != generation {
                            return;
                        }
                        win.apply_refreshed_messages(fresh, cx);
                    });
                }
            })
            .detach();
            return;
        }
        let fresh = load_fixture_messages(&self.kind, &self.fixtures);
        self.apply_refreshed_messages(fresh, cx);
    }

    pub(super) fn apply_refreshed_messages(&mut self, fresh: Vec<Message>, cx: &mut Context<Self>) {
        // Compare full message equality so remote edits, reactions, and pins always refresh.
        if fresh == self.messages {
            return;
        }
        let appended = fresh.len() > self.messages.len()
            || fresh.last().map(|m| &m.ts) != self.messages.last().map(|m| &m.ts);
        self.messages = fresh;
        if appended {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    pub(super) fn maybe_fetch_huddle_thread(&mut self) {
        if self.huddle_drawer != Some(HuddleDrawer::Thread) {
            return;
        }
        let Some(state) = &self.huddle_state else {
            return;
        };
        let Some(root_ts) = self.resolve_huddle_thread_root(&state.session) else {
            return;
        };
        if self.huddle_thread_fetched.as_ref() == Some(&root_ts) {
            return;
        }
        self.huddle_thread_fetched = Some(root_ts.clone());
        (self.send)(
            MessagingCommand::OpenThread {
                channel: state.session.channel.clone(),
                thread_ts: root_ts,
            }
            .into(),
        );
    }
}

pub(super) fn load_fixture_messages(kind: &PopoutKind, fixtures: &Fixtures) -> Vec<Message> {
    match kind {
        PopoutKind::Conversation { channel, .. } => {
            fixtures.messages.get(channel).cloned().unwrap_or_default()
        }
        PopoutKind::Thread {
            channel, thread_ts, ..
        } => fixtures
            .threads
            .get(&(channel.clone(), thread_ts.clone()))
            .cloned()
            .unwrap_or_default(),
        PopoutKind::Activity { .. }
        | PopoutKind::Later { .. }
        | PopoutKind::Canvas { .. }
        | PopoutKind::Files { .. }
        | PopoutKind::Huddle { .. } => Vec::new(),
    }
}
