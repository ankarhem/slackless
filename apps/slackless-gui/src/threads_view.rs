//! The Threads view (the sidebar's "Threads" row): every followed thread as one card - the
//! channel + participants header above a rounded bordered card with the root message, a
//! clickable "Show N more replies" expander, the latest replies, and a docked reply composer
//! with "Also send to #channel" (click another card's bordered "Reply..." bar to move it).
//! Cards, their derivations and the card element live in [`crate::threads_cards`].
//!
//! Data: live mode asks the core for Slack's own list (`subscriptions.thread.getView`,
//! [`slackless_core::Command::FetchThreads`]) and shows store-derived cards until it lands -
//! and keeps them when the fetch fails. Fixture mode derives the cards from the bundled
//! workspace.
//!
//! `ThreadsView::new(Arc<Fixtures>, &mut Window, &mut App) -> Entity<ThreadsView>`
//! `ThreadsView::show_local / show_store / set_remote / set_card_replies / set_fixtures / set_enter_sends`
//! emits `ThreadsEvent::Open { channel, thread_ts }` / `OpenLink(LinkTarget)`
//! / `ExpandThread { channel, thread_ts }` / `Reply { .. }` / `PickEmoji(point)`
//! / `SlashCommand(Invocation)`

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Pixels, Point, Render,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use slackless_core::api::{MessageContent, ThreadsView as SlackThreads};
use slackless_core::types::{ChannelId, Message, Ts};

use crate::actions::{self, MessageAction};
use crate::composer::{self, AlsoSendTarget, Composer, ComposerEvent, PendingAttachment};
use crate::fixtures::Fixtures;
use crate::image_viewer::{self, ImageOrigin, ViewerImage};
use crate::messages::LinkTarget;
use crate::slash::{Invocation, SlashKind};
use crate::threads_cards::{
    ListItemThreadMeta, ThreadCard, card_view, list_item_thread_card, remote_cards, sort_cards,
};

/// Filter tabs in the Threads header allowing users to view all followed threads, channel/DM
/// message threads, or Slack List item threads (`F-COLLAB-250`, `F-COLLAB-251`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThreadsFilter {
    #[default]
    All,
    Messages,
    ListItems,
}

pub enum ThreadsEvent {
    /// A card's channel header was clicked: open the conversation with the thread panel on `thread_ts`.
    Open {
        channel: ChannelId,
        thread_ts: Ts,
    },
    /// A list item thread header was clicked: open the parent Slack List on `record_id`.
    OpenListItem {
        list_id: String,
        record_id: String,
    },
    /// A link inside a thread card message (@mention, #channel, URL) was activated.
    OpenLink(LinkTarget),
    /// The user clicked "Show N more replies" on a card: ask the root to load full replies.
    ExpandThread {
        channel: ChannelId,
        thread_ts: Ts,
    },
    /// The user replied; the card already shows the reply locally.
    Reply {
        channel: ChannelId,
        thread_ts: Ts,
        content: MessageContent,
        broadcast: bool,
    },
    /// The user replied with attached files (and optional message text).
    ReplyWithFiles {
        channel: ChannelId,
        thread_ts: Ts,
        content: Option<MessageContent>,
        files: Vec<PendingAttachment>,
    },
    /// A message-level action (reaction, pin, save, edit, delete, image, profile, Block Kit, etc.)
    /// was triggered on a root message or reply inside a thread card.
    MessageAction {
        channel: ChannelId,
        thread_ts: Ts,
        message: Box<Message>,
        action: MessageAction,
    },
    /// An inline edit inside a thread card was saved; the card already reflects the updated text.
    EditMessage {
        channel: ChannelId,
        ts: Ts,
        content: MessageContent,
    },
    /// User explicitly marked one thread card read up to `ts`.
    MarkRead {
        channel: ChannelId,
        thread_ts: Ts,
        ts: Ts,
    },
    /// User explicitly marked all unread thread cards read (`(channel, thread_ts, newest_ts)`).
    MarkAllRead(Vec<(ChannelId, Ts, Ts)>),
    /// The reply composer's emoji button asked for the picker.
    PickEmoji(Point<Pixels>),
    PickGif(Point<Pixels>),
    /// A slash command other than /shrug, which replies here; the root dispatches it.
    SlashCommand(Invocation),
}

pub struct ThreadsView {
    fixtures: Arc<Fixtures>,
    cards: Vec<ThreadCard>,
    /// Slack's own list landed; later store derivations must not overwrite it.
    remote: bool,
    /// True on the very first live open before `subscriptions.thread.getView` has answered.
    loading: bool,
    /// The card the shared reply composer is docked to (defaults to card 0 when present).
    active: Option<usize>,
    /// Cards whose "Show N more replies" link has been expanded in-place.
    expanded: HashSet<(ChannelId, Ts)>,
    /// Saved-for-later message timestamps synced from `LaterView`.
    saved: HashSet<Ts>,
    /// Active inline message edit `(channel, ts, textarea)`.
    editing: Option<(ChannelId, Ts, Entity<TextareaState>)>,
    /// Filter pills in the Threads header (`All`, `Messages`, `List items`).
    filter: ThreadsFilter,
    composer: Entity<Composer>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ThreadsEvent> for ThreadsView {}

impl ThreadsView {
    pub fn new(fixtures: Arc<Fixtures>, window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let composer = Composer::new("Reply...".into(), fixtures.clone(), window, cx);
            composer.update(cx, |composer, cx| composer.set_scheduling(false, cx));
            let subscriptions = vec![cx.subscribe(
                &composer,
                |this: &mut ThreadsView, _, event: &ComposerEvent, cx| match event {
                    ComposerEvent::Submit(content) => this.reply(content.clone(), cx),
                    ComposerEvent::SubmitWithFiles { content, files } => {
                        if let Some((channel, thread_ts)) = this.active_thread() {
                            cx.emit(ThreadsEvent::ReplyWithFiles {
                                channel,
                                thread_ts,
                                content: content.clone(),
                                files: files.clone(),
                            });
                        }
                    }
                    ComposerEvent::OpenEmojiPicker(position) => {
                        cx.emit(ThreadsEvent::PickEmoji(*position));
                    }
                    ComposerEvent::OpenGifPicker(position) => {
                        cx.emit(ThreadsEvent::PickGif(*position));
                    }
                    ComposerEvent::SlashCommand(invocation) => {
                        match invocation.command.map(|command| command.kind) {
                            Some(SlashKind::Shrug) => {
                                let text = crate::composer::format_shrug(&invocation.args);
                                this.reply(MessageContent::plain(text), cx);
                            }
                            _ => cx.emit(ThreadsEvent::SlashCommand(invocation.clone())),
                        }
                    }
                    // Scheduling is turned off in the reply composer, and a card has no single
                    // "last message" for Up to edit.
                    ComposerEvent::Schedule { .. }
                    | ComposerEvent::ScheduleCustom(_)
                    | ComposerEvent::EditLast
                    | ComposerEvent::Typing
                    | ComposerEvent::Cancel => {}
                },
            )];
            ThreadsView {
                fixtures,
                cards: Vec::new(),
                remote: false,
                loading: false,
                active: None,
                expanded: HashSet::new(),
                saved: HashSet::new(),
                editing: None,
                filter: ThreadsFilter::All,
                composer,
                _subscriptions: subscriptions,
            }
        })
    }

    fn sync_also_send(&self, cx: &mut Context<Self>) {
        let target = self
            .active
            .and_then(|ix| self.cards.get(ix))
            .filter(|card| card.list_item.is_none())
            .and_then(|card| AlsoSendTarget::for_channel(&self.fixtures, &card.channel));
        self.composer
            .update(cx, |composer, cx| composer.set_also_send(target, cx));
    }

    /// Show `cards` (the fixture derivation) and drop any earlier remote list; call when the
    /// view opens in fixture mode.
    pub fn show_local(&mut self, cards: Vec<ThreadCard>, cx: &mut Context<Self>) {
        self.remote = false;
        self.loading = false;
        self.cards = cards;
        self.expanded.clear();
        self.active = (!self.cards.is_empty()).then_some(0);
        self.sync_also_send(cx);
        cx.notify();
    }

    /// Live mode open: keep already-fetched remote cards on screen while refreshing silently in
    /// the background (zero flash). Before the first remote response arrives, show a loading
    /// state rather than flashing an empty view or stale store-derived cards.
    pub fn begin_remote_load(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        if !self.remote {
            self.loading = true;
        }
        self.sync_also_send(cx);
        cx.notify();
    }

    /// Drop cached threads when switching to another workspace.
    pub fn clear_for_workspace(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        self.remote = false;
        self.loading = false;
        self.cards.clear();
        self.expanded.clear();
        self.editing = None;
        self.filter = ThreadsFilter::All;
        self.active = None;
        self.sync_also_send(cx);
        cx.notify();
    }

    /// Switch the header filter (`All` / `Messages` / `List items`).
    pub fn set_filter(&mut self, filter: ThreadsFilter, cx: &mut Context<Self>) {
        if self.filter != filter {
            self.filter = filter;
            cx.notify();
        }
    }

    /// Merge or insert a Slack List item comment thread fetched via `lists.getThreadsForRecord`
    /// (`F-COLLAB-250`, `F-COLLAB-251`) so it appears cleanly alongside message threads.
    pub fn upsert_list_item_thread(
        &mut self,
        meta: ListItemThreadMeta,
        messages: Vec<Message>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(card) = list_item_thread_card(meta, messages) else {
            return false;
        };
        if let Some(existing) = self.cards.iter_mut().find(|c| {
            c.list_item.as_ref().is_some_and(|m| {
                card.list_item
                    .as_ref()
                    .is_some_and(|n| m.list_id == n.list_id && m.record_id == n.record_id)
            })
        }) {
            *existing = card;
        } else {
            self.cards.push(card);
        }
        sort_cards(&mut self.cards);
        self.active = self
            .active
            .filter(|&ix| ix < self.cards.len())
            .or_else(|| (!self.cards.is_empty()).then_some(0));
        self.sync_also_send(cx);
        cx.notify();
        true
    }

    /// Replace the set of saved-for-later message timestamps (synced from `LaterView`).
    pub fn replace_saved(&mut self, saved: HashSet<Ts>, cx: &mut Context<Self>) {
        if self.saved != saved {
            self.saved = saved;
            cx.notify();
        }
    }

    /// Update a single message's saved marker.
    pub fn set_saved(&mut self, ts: &Ts, saved: bool, cx: &mut Context<Self>) {
        let changed = if saved {
            self.saved.insert(ts.clone())
        } else {
            self.saved.remove(ts)
        };
        if changed {
            cx.notify();
        }
    }

    fn find_message_mut(&mut self, channel: &ChannelId, ts: &Ts) -> Option<&mut Message> {
        for card in &mut self.cards {
            if card.channel != *channel {
                continue;
            }
            if card.root.ts == *ts {
                return Some(&mut card.root);
            }
            if let Some(reply) = card.replies.iter_mut().find(|m| m.ts == *ts) {
                return Some(reply);
            }
        }
        None
    }

    /// Toggle a reaction on a thread root or reply; returns `Some(added)` when the message exists.
    pub fn toggle_reaction(
        &mut self,
        channel: &ChannelId,
        ts: &Ts,
        name: &str,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        let me = self.fixtures.self_user.clone();
        let msg = self.find_message_mut(channel, ts)?;
        let add = actions::toggle_reaction(msg, &me, name);
        cx.notify();
        Some(add)
    }

    /// Add a reaction on a thread root or reply if not already reacted by the signed-in user.
    pub fn add_reaction(
        &mut self,
        channel: &ChannelId,
        ts: &Ts,
        name: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let me = self.fixtures.self_user.clone();
        let Some(msg) = self.find_message_mut(channel, ts) else {
            return false;
        };
        if actions::add_reaction(msg, &me, name) {
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Toggle a message's pinned state in its channel; returns `Some(pinned)` if found.
    pub fn toggle_pin(
        &mut self,
        channel: &ChannelId,
        ts: &Ts,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        let msg = self.find_message_mut(channel, ts)?;
        let pinned = actions::toggle_pin(msg, channel);
        cx.notify();
        Some(pinned)
    }

    /// Remove a deleted or dismissed message from its thread card.
    pub fn remove_message(&mut self, channel: &ChannelId, ts: &Ts, cx: &mut Context<Self>) {
        let mut changed = false;
        if let Some(card_ix) = self
            .cards
            .iter()
            .position(|c| c.channel == *channel && c.root.ts == *ts)
        {
            self.cards.remove(card_ix);
            self.active = self
                .active
                .filter(|&ix| ix < self.cards.len())
                .or_else(|| (!self.cards.is_empty()).then_some(0));
            changed = true;
        } else {
            for card in &mut self.cards {
                if card.channel != *channel {
                    continue;
                }
                let before = card.replies.len();
                card.replies.retain(|m| m.ts != *ts);
                if card.replies.len() < before {
                    card.reply_total = card.reply_total.saturating_sub(1);
                    changed = true;
                }
            }
        }
        if self
            .editing
            .as_ref()
            .is_some_and(|(ch, edit_ts, _)| ch == channel && edit_ts == ts)
        {
            self.editing = None;
            changed = true;
        }
        if changed {
            self.sync_also_send(cx);
            cx.notify();
        }
    }

    /// Begin inline editing of `message` inside its thread card.
    pub fn start_edit(
        &mut self,
        channel: ChannelId,
        message: &Message,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial = self.fixtures.plain_text(message);
        let input = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder("Edit message…")
                .auto_grow(1, 6);
            state.set_value(initial, window, cx);
            state
        });
        input.read(cx).focus_handle(cx).focus(window, cx);
        self.editing = Some((channel, message.ts.clone(), input));
        cx.notify();
    }

    /// Commit the active inline edit and emit [`ThreadsEvent::EditMessage`].
    pub(super) fn save_edit(&mut self, cx: &mut Context<Self>) {
        let Some((channel, ts, input)) = self.editing.take() else {
            return;
        };
        let text = input.read(cx).value().trim().to_owned();
        if text.is_empty() {
            cx.notify();
            return;
        }
        let content = composer::content_of(&text, &self.fixtures);
        let me = self.fixtures.self_user.clone();
        if let Some(msg) = self.find_message_mut(&channel, &ts) {
            actions::apply_edit(msg, &me, content.clone());
        }
        cx.emit(ThreadsEvent::EditMessage {
            channel,
            ts,
            content,
        });
        cx.notify();
    }

    /// Cancel any active inline edit.
    pub(super) fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            cx.notify();
        }
    }

    /// Dock and focus the reply composer on the card for `(channel, thread_ts)`.
    pub fn activate_thread(
        &mut self,
        channel: &ChannelId,
        thread_ts: &Ts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self
            .cards
            .iter()
            .position(|c| c.channel == *channel && c.thread_ts == *thread_ts)
        {
            self.activate(Some(ix), window, cx);
        }
    }

    /// Collect all images across a thread card (root + replies) and locate `(ts, origin)`.
    pub fn card_images(
        &self,
        channel: &ChannelId,
        thread_ts: &Ts,
        ts: &Ts,
        origin: &ImageOrigin,
    ) -> Option<(Rc<Vec<ViewerImage>>, usize)> {
        let card = self
            .cards
            .iter()
            .find(|c| c.channel == *channel && c.thread_ts == *thread_ts)?;
        image_viewer::collect_viewer_images(
            &self.fixtures,
            std::iter::once(&card.root).chain(&card.replies),
            ts,
            origin,
        )
    }

    /// Show `cards` (the store derivation fallback) unless Slack's own list already landed.
    pub fn show_store(&mut self, cards: Vec<ThreadCard>, cx: &mut Context<Self>) {
        if self.remote {
            return;
        }
        self.loading = false;
        self.cards = cards;
        self.active = (!self.cards.is_empty()).then_some(0);
        self.sync_also_send(cx);
        cx.notify();
    }

    /// Replace the cards with Slack's own followed threads (`subscriptions.thread.getView`),
    /// preserving any list item comment threads fetched via `lists.getThreadsForRecord` that
    /// Slack's subscription endpoint did not include directly.
    pub fn set_remote(&mut self, view: SlackThreads, cx: &mut Context<Self>) {
        self.remote = true;
        self.loading = false;
        let preserved_list_cards: Vec<ThreadCard> = self
            .cards
            .iter()
            .filter(|c| c.list_item.is_some())
            .cloned()
            .collect();
        let mut next = remote_cards(view);
        for card in preserved_list_cards {
            let dup = card.list_item.as_ref().is_some_and(|meta| {
                next.iter().any(|c| {
                    c.list_item
                        .as_ref()
                        .is_some_and(|m| m.list_id == meta.list_id && m.record_id == meta.record_id)
                })
            });
            if !dup {
                next.push(card);
            }
        }
        sort_cards(&mut next);
        self.cards = next;
        self.active = self
            .active
            .filter(|&ix| ix < self.cards.len())
            .or_else(|| (!self.cards.is_empty()).then_some(0));
        self.sync_also_send(cx);
        cx.notify();
    }

    /// Fill an expanded card with its full replies (fetched from the store or Slack).
    pub fn set_card_replies(
        &mut self,
        channel: &ChannelId,
        thread_ts: &Ts,
        replies: Vec<Message>,
        cx: &mut Context<Self>,
    ) {
        if let Some(card) = self
            .cards
            .iter_mut()
            .find(|c| &c.channel == channel && &c.thread_ts == thread_ts)
            && !replies.is_empty()
        {
            card.reply_total = card.reply_total.max(replies.len() as u32);
            card.replies = replies;
            cx.notify();
        }
    }

    /// Live refresh: keep the cards, draw them from `fixtures`.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        self.sync_also_send(cx);
        cx.notify();
    }

    /// Forward the Enter-to-send preference to the reply composer.
    pub fn set_enter_sends(&mut self, enter_sends: bool, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |composer, cx| composer.set_enter_sends(enter_sends, cx));
    }

    /// Insert an emoji pick into the reply composer.
    pub fn insert_emoji(
        &mut self,
        choice: &crate::emoji::EmojiChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composer
            .update(cx, |composer, cx| composer.insert_emoji(choice, window, cx));
    }

    /// Stage a GIF pick inside the reply composer.
    pub fn stage_gif(
        &mut self,
        gif: slackless_core::api::GifItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composer
            .update(cx, |composer, cx| composer.stage_gif(gif, window, cx));
    }

    /// The thread the reply composer is docked to, where a slash command typed there runs.
    pub fn active_thread(&self) -> Option<(ChannelId, Ts)> {
        let card = self.cards.get(self.active?)?;
        Some((card.channel.clone(), card.thread_ts.clone()))
    }

    fn reply(&mut self, content: MessageContent, cx: &mut Context<Self>) {
        let Some(ix) = self.active else {
            return;
        };
        let Some(card) = self.cards.get_mut(ix) else {
            return;
        };
        let broadcast = self
            .composer
            .update(cx, |composer, cx| composer.take_also_send(cx));
        // The local echo shows the same blocks the reply posts with.
        let mut message = self.fixtures.local_message(
            &card.channel,
            content.text.clone(),
            Some(card.thread_ts.clone()),
        );
        message.blocks = content.blocks.clone();
        message.attachments = content.attachments.clone();
        card.replies.push(message);
        card.reply_total += 1;
        card.unread = 0;
        card.last_read = None;
        cx.emit(ThreadsEvent::Reply {
            channel: card.channel.clone(),
            thread_ts: card.thread_ts.clone(),
            content,
            broadcast,
        });
        cx.notify();
    }

    /// Dock the reply composer to card `ix` and focus it.
    pub(super) fn activate(
        &mut self,
        ix: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active = ix;
        self.sync_also_send(cx);
        if ix.is_some() {
            self.composer
                .update(cx, |composer, cx| composer.focus_input(window, cx));
        }
        cx.notify();
    }

    /// Expanded `(channel, thread_ts)` pairs matching `channel`, for live store refreshes.
    pub fn expanded_in(&self, channel: &ChannelId) -> Vec<Ts> {
        self.expanded
            .iter()
            .filter(|(ch, _)| ch == channel)
            .map(|(_, ts)| ts.clone())
            .collect()
    }

    /// Expand hidden replies on `(channel, thread_ts)` in-place.
    pub(super) fn expand(&mut self, channel: ChannelId, thread_ts: Ts, cx: &mut Context<Self>) {
        self.expanded.insert((channel.clone(), thread_ts.clone()));
        cx.emit(ThreadsEvent::ExpandThread { channel, thread_ts });
        cx.notify();
    }

    /// True once Slack's `subscriptions.thread.getView` list has been loaded for this workspace.
    pub fn is_remote(&self) -> bool {
        self.remote
    }

    /// True if any displayed thread card still has unread replies.
    pub fn has_unreads(&self) -> bool {
        self.cards.iter().any(|c| c.unread > 0)
    }

    /// True if the specific thread card currently shows unread replies.
    pub fn is_thread_unread(&self, channel: &ChannelId, thread_ts: &Ts) -> bool {
        self.cards
            .iter()
            .any(|c| c.channel == *channel && c.thread_ts == *thread_ts && c.unread > 0)
    }

    /// `(channel, thread_ts, newest_ts)` for every card currently carrying unread replies.
    #[cfg(test)]
    pub fn unread_targets(&self) -> Vec<(ChannelId, Ts, Ts)> {
        self.cards
            .iter()
            .filter(|c| c.unread > 0)
            .map(|c| (c.channel.clone(), c.thread_ts.clone(), c.newest_ts()))
            .collect()
    }

    /// The read cursor immediately before `ts` inside `(channel, thread_ts)` (for "Mark unread").
    pub fn cursor_before(&self, channel: &ChannelId, thread_ts: &Ts, ts: &Ts) -> Ts {
        self.cards
            .iter()
            .find(|c| c.channel == *channel && c.thread_ts == *thread_ts)
            .map(|c| c.cursor_before(ts))
            .unwrap_or_else(|| crate::threads_cards::ts_just_before(ts))
    }

    /// Apply a thread read cursor (`last_read`) to `(channel, thread_ts)`: clears `unread` when
    /// caught up to `newest_ts()`, or recounts unread replies when moved earlier (Mark unread).
    pub fn mark_thread_read(
        &mut self,
        channel: &ChannelId,
        thread_ts: &Ts,
        last_read: Option<&Ts>,
        clear_divider: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(card) = self
            .cards
            .iter_mut()
            .find(|c| c.channel == *channel && c.thread_ts == *thread_ts)
        else {
            return false;
        };
        let newest = card.newest_ts();
        let new_unread = match last_read {
            None => 0,
            Some(cursor) if *cursor >= newest => 0,
            Some(cursor) => {
                let count = card.replies.iter().filter(|m| m.ts > *cursor).count() as u32;
                count.max(1)
            }
        };
        let mut changed = card.unread != new_unread;
        card.unread = new_unread;
        if let Some(cursor) = last_read {
            card.root.last_read = Some(cursor.clone());
            if new_unread > 0 {
                card.last_read = Some(cursor.clone());
                changed = true;
            } else if clear_divider && card.last_read.take().is_some() {
                changed = true;
            }
        } else if clear_divider && card.last_read.take().is_some() {
            changed = true;
        }
        if changed {
            cx.notify();
        }
        changed
    }

    /// Clear local unread badges across all cards and return every `(channel, thread_ts, newest_ts)`
    /// that was unread. When `clear_dividers` is false (auto-mark on opening Threads), each card
    /// keeps its pre-mark `last_read` so the `"New"` divider stays visible during this visit.
    pub fn clear_all_unreads(
        &mut self,
        clear_dividers: bool,
        cx: &mut Context<Self>,
    ) -> Vec<(ChannelId, Ts, Ts)> {
        let mut targets = Vec::new();
        let mut changed = false;
        for card in &mut self.cards {
            if card.unread > 0 {
                let newest = card.newest_ts();
                targets.push((card.channel.clone(), card.thread_ts.clone(), newest.clone()));
                card.unread = 0;
                card.root.last_read = Some(newest);
                changed = true;
            }
            if clear_dividers && card.last_read.take().is_some() {
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
        targets
    }

    /// Explicit per-card "Mark as read" click.
    pub(super) fn mark_card_read(
        &mut self,
        channel: ChannelId,
        thread_ts: Ts,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = self
            .cards
            .iter_mut()
            .find(|c| c.channel == channel && c.thread_ts == thread_ts)
        else {
            return;
        };
        let ts = card.newest_ts();
        card.unread = 0;
        card.last_read = None;
        card.root.last_read = Some(ts.clone());
        cx.emit(ThreadsEvent::MarkRead {
            channel,
            thread_ts,
            ts,
        });
        cx.notify();
    }

    /// Explicit header "Mark all read" click.
    pub(super) fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        let targets = self.clear_all_unreads(true, cx);
        if !targets.is_empty() {
            cx.emit(ThreadsEvent::MarkAllRead(targets));
        }
    }
}

impl Render for ThreadsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, background, secondary, foreground) = (
            theme.border,
            theme.muted_foreground,
            theme.background,
            theme.secondary,
            theme.foreground,
        );
        let unread: u32 = self.cards.iter().map(|card| card.unread).sum();
        let list_count = self.cards.iter().filter(|c| c.list_item.is_some()).count();
        let msg_count = self.cards.len().saturating_sub(list_count);
        let filter = self.filter;
        let cards: Vec<(usize, ThreadCard)> = self
            .cards
            .iter()
            .cloned()
            .enumerate()
            .filter(|(_, card)| match filter {
                ThreadsFilter::All => true,
                ThreadsFilter::Messages => card.list_item.is_none(),
                ThreadsFilter::ListItems => card.list_item.is_some(),
            })
            .collect();
        let mut inner = v_flex().w_full().px_5().py_4().gap_6();
        let editing = self.editing.clone();
        for (ix, card) in &cards {
            let expanded = self
                .expanded
                .contains(&(card.channel.clone(), card.thread_ts.clone()));
            inner = inner.child(card_view(
                crate::threads_cards::ThreadCardRenderArgs {
                    fixtures: &self.fixtures,
                    ix: *ix,
                    card,
                    active: self.active == Some(*ix),
                    expanded,
                    saved_set: &self.saved,
                    editing: editing.as_ref(),
                    composer: &self.composer,
                },
                cx,
            ));
        }
        let loading = self.loading;
        let inner = inner.when(cards.is_empty(), |list| {
            list.child(
                v_flex()
                    .items_center()
                    .gap_2()
                    .py_10()
                    .text_sm()
                    .text_color(muted)
                    .child(
                        Icon::new(if filter == ThreadsFilter::ListItems {
                            IconName::ListChecks
                        } else {
                            IconName::MessageSquareText
                        })
                        .text_color(muted)
                        .size(px(28.)),
                    )
                    .child(if loading {
                        "Loading threads…"
                    } else if filter == ThreadsFilter::ListItems {
                        "No list item threads yet. Open a list item and comment to see it here."
                    } else {
                        "No followed threads yet. Reply in one to follow it."
                    }),
            )
        });
        let filter_pill = |id: &'static str, label: String, target: ThreadsFilter| {
            let active = filter == target;
            div()
                .id(id)
                .px_2()
                .py(px(3.))
                .rounded_sm()
                .text_xs()
                .cursor_pointer()
                .when(active, |el| {
                    el.bg(secondary)
                        .text_color(foreground)
                        .font_weight(FontWeight::SEMIBOLD)
                })
                .when(!active, |el| el.text_color(muted))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.set_filter(target, cx)))
        };

        v_flex()
            .size_full()
            .bg(background)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.editing.is_some() {
                    this.cancel_edit(cx);
                }
            }))
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px_5()
                    .h(px(49.))
                    .gap_2()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::BOLD)
                                    .child("Threads"),
                            )
                            .when(unread > 0, |title| {
                                title.child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::NORMAL)
                                        .text_color(muted)
                                        .child(format!("{unread} unread")),
                                )
                            })
                            .when(list_count > 0, |title| {
                                title.child(
                                    h_flex()
                                        .ml_2()
                                        .p(px(2.))
                                        .gap_0p5()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(border)
                                        .child(filter_pill(
                                            "threads-filter-all",
                                            format!("All ({})", self.cards.len()),
                                            ThreadsFilter::All,
                                        ))
                                        .child(filter_pill(
                                            "threads-filter-messages",
                                            format!("Messages ({msg_count})"),
                                            ThreadsFilter::Messages,
                                        ))
                                        .child(filter_pill(
                                            "threads-filter-lists",
                                            format!("List items ({list_count})"),
                                            ThreadsFilter::ListItems,
                                        )),
                                )
                            }),
                    )
                    .when(unread > 0, |bar| {
                        bar.child(
                            Button::new("threads-mark-all-read")
                                .outline()
                                .with_size(Size::Small)
                                .label("Mark all read")
                                .on_click(cx.listener(|this, _, _, cx| this.mark_all_read(cx))),
                        )
                    }),
            )
            .child(
                div()
                    .id("threads-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(inner),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui::{self, TestAppContext};
    use slackless_core::api::ThreadSubscription;

    #[gpui::test]
    fn begin_remote_load_keeps_cached_remote_threads_without_flashing_empty(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let fixtures = Arc::new(crate::fixtures::load());
        let view_cell = std::rc::Rc::new(std::cell::RefCell::new(None));
        let cell = view_cell.clone();
        let (_root, cx) = cx.add_window_view(|window, cx| {
            let view = ThreadsView::new(fixtures.clone(), window, cx);
            *cell.borrow_mut() = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = view_cell.borrow_mut().take().unwrap();

        cx.update(|_, cx| {
            // First live open: loading = true, no stale/empty flash.
            view.update(cx, |v, cx| v.begin_remote_load(fixtures.clone(), cx));
            assert!(view.read(cx).loading);
            assert!(view.read(cx).cards.is_empty());

            // Remote response lands.
            let remote = SlackThreads {
                threads: vec![ThreadSubscription {
                    root: Message {
                        channel: Some(ChannelId::from("C0ENG")),
                        ts: Ts::from("1760000001.000100"),
                        text: "hello".into(),
                        reply_count: Some(1),
                        ..Default::default()
                    },
                    latest_replies: vec![],
                    unread_replies: vec![],
                }],
                has_more: false,
                total_unread_replies: 0,
            };
            view.update(cx, |v, cx| v.set_remote(remote, cx));
            assert!(!view.read(cx).loading);
            assert_eq!(view.read(cx).cards.len(), 1);

            // Re-opening Threads in live mode must keep the fetched remote cards on screen.
            view.update(cx, |v, cx| v.begin_remote_load(fixtures.clone(), cx));
            assert!(!view.read(cx).loading);
            assert_eq!(view.read(cx).cards.len(), 1);

            // Switching workspaces drops the previous workspace's threads cleanly.
            view.update(cx, |v, cx| v.clear_for_workspace(fixtures.clone(), cx));
            assert!(!view.read(cx).remote);
            assert!(view.read(cx).cards.is_empty());
        });
    }

    #[gpui::test]
    fn threads_view_message_actions_mutate_state(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let fixtures = Arc::new(crate::fixtures::load());
        let cards = crate::threads_cards::local_cards(&fixtures);
        assert!(!cards.is_empty());

        let view_cell = std::rc::Rc::new(std::cell::RefCell::new(None));
        let cell = view_cell.clone();
        let (_root, cx) = cx.add_window_view(|window, cx| {
            let view = ThreadsView::new(fixtures.clone(), window, cx);
            *cell.borrow_mut() = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = view_cell.borrow_mut().take().unwrap();
        let edited: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let edited_cb = edited.clone();
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event, _| {
                if let ThreadsEvent::EditMessage { content, .. } = event {
                    edited_cb.lock().unwrap().push(content.text.clone());
                }
            })
            .detach();
        });

        cx.update(|window, cx| {
            view.update(cx, |v, cx| v.show_local(cards.clone(), cx));
            let card = view.read(cx).cards[0].clone();
            let ch = card.channel.clone();
            let root_ts = card.root.ts.clone();
            let reply = card.replies[0].clone();
            let reply_ts = reply.ts.clone();

            // Reactions on root and reply
            let added = view.update(cx, |v, cx| v.add_reaction(&ch, &root_ts, "rocket", cx));
            assert!(added);
            assert!(!view.update(cx, |v, cx| v.add_reaction(&ch, &root_ts, "rocket", cx)));
            assert_eq!(
                view.update(cx, |v, cx| v.toggle_reaction(&ch, &root_ts, "rocket", cx)),
                Some(false)
            );
            assert_eq!(
                view.update(cx, |v, cx| v.toggle_reaction(&ch, &reply_ts, "eyes", cx)),
                Some(true)
            );

            // Pinning and saving
            assert_eq!(
                view.update(cx, |v, cx| v.toggle_pin(&ch, &reply_ts, cx)),
                Some(true)
            );
            assert_eq!(
                view.update(cx, |v, cx| v.toggle_pin(&ch, &reply_ts, cx)),
                Some(false)
            );
            view.update(cx, |v, cx| {
                v.replace_saved(HashSet::from([root_ts.clone()]), cx);
            });
            assert!(view.read(cx).saved.contains(&root_ts));
            view.update(cx, |v, cx| v.set_saved(&root_ts, false, cx));
            assert!(!view.read(cx).saved.contains(&root_ts));

            // Inline editing
            view.update(cx, |v, cx| {
                v.start_edit(ch.clone(), &reply, window, cx);
                let input = v.editing.as_ref().unwrap().2.clone();
                input.update(cx, |s, cx| {
                    s.set_value("Edited reply text".to_owned(), window, cx)
                });
                v.save_edit(cx);
            });
            assert!(view.read(cx).editing.is_none());
            assert_eq!(
                view.read(cx).cards[0]
                    .replies
                    .iter()
                    .find(|m| m.ts == reply_ts)
                    .unwrap()
                    .text,
                "Edited reply text"
            );

            // Deleting a reply
            let before_replies = view.read(cx).cards[0].replies.len();
            view.update(cx, |v, cx| v.remove_message(&ch, &reply_ts, cx));
            assert_eq!(view.read(cx).cards[0].replies.len(), before_replies - 1);
        });
        assert_eq!(*edited.lock().unwrap(), vec!["Edited reply text"]);
    }

    #[gpui::test]
    fn threads_view_marks_individual_and_all_threads_read_and_unread(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let fixtures = Arc::new(crate::fixtures::load());
        let view_cell = std::rc::Rc::new(std::cell::RefCell::new(None));
        let cell = view_cell.clone();
        let (_root, cx) = cx.add_window_view(|window, cx| {
            let view = ThreadsView::new(fixtures.clone(), window, cx);
            *cell.borrow_mut() = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = view_cell.borrow_mut().take().unwrap();
        let marked: Arc<std::sync::Mutex<Vec<(ChannelId, Ts, Ts)>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let marked_cb = marked.clone();
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event, _| match event {
                ThreadsEvent::MarkRead {
                    channel,
                    thread_ts,
                    ts,
                } => {
                    marked_cb.lock().unwrap().push((
                        channel.clone(),
                        thread_ts.clone(),
                        ts.clone(),
                    ));
                }
                ThreadsEvent::MarkAllRead(targets) => {
                    marked_cb.lock().unwrap().extend(targets.iter().cloned());
                }
                _ => {}
            })
            .detach();
        });

        let remote = SlackThreads {
            threads: vec![
                ThreadSubscription {
                    root: Message {
                        channel: Some(ChannelId::from("C0ENG")),
                        ts: Ts::from("1760000001.000100"),
                        text: "thread 1".into(),
                        reply_count: Some(2),
                        last_read: Some(Ts::from("1760000001.000200")),
                        ..Default::default()
                    },
                    latest_replies: vec![Message {
                        ts: Ts::from("1760000001.000200"),
                        text: "older".into(),
                        ..Default::default()
                    }],
                    unread_replies: vec![Message {
                        ts: Ts::from("1760000001.000300"),
                        text: "new reply".into(),
                        ..Default::default()
                    }],
                },
                ThreadSubscription {
                    root: Message {
                        channel: Some(ChannelId::from("C0GENERAL")),
                        ts: Ts::from("1760000002.000100"),
                        text: "thread 2".into(),
                        reply_count: Some(1),
                        ..Default::default()
                    },
                    latest_replies: vec![],
                    unread_replies: vec![Message {
                        ts: Ts::from("1760000002.000200"),
                        text: "unread 2".into(),
                        ..Default::default()
                    }],
                },
            ],
            has_more: false,
            total_unread_replies: 2,
        };

        cx.update(|_, cx| {
            view.update(cx, |v, cx| v.set_remote(remote, cx));
            assert!(view.read(cx).has_unreads());
            assert_eq!(view.read(cx).unread_targets().len(), 2);

            // Explicit per-card mark read clears card 1 and emits MarkRead with newest reply ts.
            view.update(cx, |v, cx| {
                v.mark_card_read(ChannelId::from("C0ENG"), Ts::from("1760000001.000100"), cx);
            });
            assert!(
                !view
                    .read(cx)
                    .is_thread_unread(&ChannelId::from("C0ENG"), &Ts::from("1760000001.000100"))
            );
            assert!(view.read(cx).has_unreads());

            // Explicit Mark all read clears remaining unread cards.
            view.update(cx, |v, cx| v.mark_all_read(cx));
            assert!(!view.read(cx).has_unreads());

            // Moving cursor backward (Mark unread) restores unread count.
            view.update(cx, |v, cx| {
                v.mark_thread_read(
                    &ChannelId::from("C0ENG"),
                    &Ts::from("1760000001.000100"),
                    Some(&Ts::from("1760000001.000200")),
                    false,
                    cx,
                );
            });
            assert!(
                view.read(cx)
                    .is_thread_unread(&ChannelId::from("C0ENG"), &Ts::from("1760000001.000100"))
            );
        });

        assert_eq!(
            *marked.lock().unwrap(),
            vec![
                (
                    ChannelId::from("C0ENG"),
                    Ts::from("1760000001.000100"),
                    Ts::from("1760000001.000300"),
                ),
                (
                    ChannelId::from("C0GENERAL"),
                    Ts::from("1760000002.000100"),
                    Ts::from("1760000002.000200"),
                ),
            ]
        );
    }

    #[gpui::test]
    fn threads_view_upserts_preserves_and_filters_list_item_threads(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let fixtures = Arc::new(crate::fixtures::load());
        let view_cell = std::rc::Rc::new(std::cell::RefCell::new(None));
        let cell = view_cell.clone();
        let (_root, cx) = cx.add_window_view(|window, cx| {
            let view = ThreadsView::new(fixtures.clone(), window, cx);
            *cell.borrow_mut() = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = view_cell.borrow_mut().take().unwrap();

        cx.update(|_, cx| {
            let meta = ListItemThreadMeta {
                list_id: "F0LIST1".into(),
                list_title: "Launch tasks".into(),
                record_id: "Rec42".into(),
                item_title: "Ship v2".into(),
            };
            let inserted = view.update(cx, |v, cx| {
                v.upsert_list_item_thread(
                    meta.clone(),
                    vec![
                        Message {
                            ts: Ts::from("1760000010.000100"),
                            text: "First list comment".into(),
                            ..Default::default()
                        },
                        Message {
                            ts: Ts::from("1760000011.000100"),
                            text: "Second list comment".into(),
                            ..Default::default()
                        },
                    ],
                    cx,
                )
            });
            assert!(inserted);
            assert_eq!(view.read(cx).cards.len(), 1);
            assert_eq!(view.read(cx).cards[0].list_item.as_ref(), Some(&meta));

            // Subsequent remote message-thread refresh preserves fetched list item threads.
            let remote = SlackThreads {
                threads: vec![ThreadSubscription {
                    root: Message {
                        channel: Some(ChannelId::from("C0ENG")),
                        ts: Ts::from("1760000001.000100"),
                        text: "Channel thread".into(),
                        reply_count: Some(1),
                        ..Default::default()
                    },
                    latest_replies: vec![],
                    unread_replies: vec![],
                }],
                has_more: false,
                total_unread_replies: 0,
            };
            view.update(cx, |v, cx| v.set_remote(remote, cx));
            assert_eq!(view.read(cx).cards.len(), 2);
            assert!(view.read(cx).cards.iter().any(|c| c.list_item.is_some()));
            assert!(view.read(cx).cards.iter().any(|c| c.list_item.is_none()));

            // Filter toggles cleanly between All, Messages, and ListItems.
            view.update(cx, |v, cx| v.set_filter(ThreadsFilter::ListItems, cx));
            assert_eq!(view.read(cx).filter, ThreadsFilter::ListItems);
            view.update(cx, |v, cx| v.set_filter(ThreadsFilter::Messages, cx));
            assert_eq!(view.read(cx).filter, ThreadsFilter::Messages);
        });
    }
}
