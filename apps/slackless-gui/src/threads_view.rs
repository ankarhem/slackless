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
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Point, Render, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px,
};
use slackless_core::api::{MessageContent, ThreadsView as SlackThreads};
use slackless_core::types::{ChannelId, ConversationKind, Message, Ts};

use crate::composer::{AlsoSendTarget, Composer, ComposerEvent, PendingAttachment};
use crate::fixtures::Fixtures;
use crate::messages::LinkTarget;
use crate::slash::{Invocation, SHRUG, SlashKind};
use crate::threads_cards::{ThreadCard, card_view, remote_cards};

pub enum ThreadsEvent {
    /// A card's channel header was clicked: open the conversation with the thread panel on `thread_ts`.
    Open {
        channel: ChannelId,
        thread_ts: Ts,
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
    /// The card the shared reply composer is docked to (defaults to card 0 when present).
    active: Option<usize>,
    /// Cards whose "Show N more replies" link has been expanded in-place.
    expanded: HashSet<(ChannelId, Ts)>,
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
                            Some(SlashKind::Shrug) if invocation.args.is_empty() => {
                                this.reply(MessageContent::plain(SHRUG), cx);
                            }
                            Some(SlashKind::Shrug) => {
                                let text = format!("{} {SHRUG}", invocation.args);
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
                active: None,
                expanded: HashSet::new(),
                composer,
                _subscriptions: subscriptions,
            }
        })
    }

    fn sync_also_send(&self, cx: &mut Context<Self>) {
        let target = self
            .active
            .and_then(|ix| self.cards.get(ix))
            .and_then(|card| {
                self.fixtures
                    .conversation(&card.channel)
                    .map(|conversation| match conversation.kind() {
                        ConversationKind::Im | ConversationKind::Mpim => {
                            AlsoSendTarget::DirectMessage
                        }
                        kind => AlsoSendTarget::Channel {
                            name: conversation.name.clone().unwrap_or_default().into(),
                            private: kind == ConversationKind::PrivateChannel,
                        },
                    })
            });
        self.composer
            .update(cx, |composer, cx| composer.set_also_send(target, cx));
    }

    /// Show `cards` (the fixture derivation) and drop any earlier remote list; call when the
    /// view opens.
    pub fn show_local(&mut self, cards: Vec<ThreadCard>, cx: &mut Context<Self>) {
        self.remote = false;
        self.cards = cards;
        self.expanded.clear();
        self.active = (!self.cards.is_empty()).then_some(0);
        self.sync_also_send(cx);
        cx.notify();
    }

    /// Show `cards` (the store derivation) unless Slack's own list already landed.
    pub fn show_store(&mut self, cards: Vec<ThreadCard>, cx: &mut Context<Self>) {
        if self.remote {
            return;
        }
        self.cards = cards;
        self.active = (!self.cards.is_empty()).then_some(0);
        self.sync_also_send(cx);
        cx.notify();
    }

    /// Replace the cards with Slack's own followed threads (`subscriptions.thread.getView`).
    pub fn set_remote(&mut self, view: SlackThreads, cx: &mut Context<Self>) {
        self.remote = true;
        self.cards = remote_cards(view);
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
        {
            if !replies.is_empty() {
                card.reply_total = card.reply_total.max(replies.len() as u32);
                card.replies = replies;
                cx.notify();
            }
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
}

impl Render for ThreadsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, background) = (theme.border, theme.muted_foreground, theme.background);
        let unread: u32 = self.cards.iter().map(|card| card.unread).sum();
        let cards: Vec<(usize, ThreadCard)> = self.cards.iter().cloned().enumerate().collect();
        let mut inner = v_flex().w_full().px_5().py_4().gap_6();
        for (ix, card) in &cards {
            let expanded = self
                .expanded
                .contains(&(card.channel.clone(), card.thread_ts.clone()));
            inner = inner.child(card_view(
                &self.fixtures,
                *ix,
                card,
                self.active == Some(*ix),
                expanded,
                &self.composer,
                cx,
            ));
        }
        let inner = inner.when(cards.is_empty(), |list| {
            list.child(
                v_flex()
                    .items_center()
                    .gap_2()
                    .py_10()
                    .text_sm()
                    .text_color(muted)
                    .child(
                        Icon::new(IconName::MessageSquareText)
                            .text_color(muted)
                            .size(px(28.)),
                    )
                    .child("No followed threads yet. Reply in one to follow it."),
            )
        });

        v_flex()
            .size_full()
            .bg(background)
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .px_5()
                    .h(px(49.))
                    .gap_2()
                    .border_b_1()
                    .border_color(border)
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
