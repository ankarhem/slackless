//! The Threads view (the sidebar's "Threads" row): every followed thread as one card - the
//! conversation, the root message and the latest replies - with a single reply composer
//! docked to the card being answered (click a card's reply bar to move it). Cards, their
//! derivations and the card element live in [`crate::threads_cards`].
//!
//! Data: live mode asks the core for Slack's own list (`subscriptions.thread.getView`,
//! [`slackless_core::Command::FetchThreads`]) and shows store-derived cards until it lands -
//! and keeps them when the fetch fails. Fixture mode derives the cards from the bundled
//! workspace.
//!
//! `ThreadsView::new(Arc<Fixtures>, &mut Window, &mut App) -> Entity<ThreadsView>`
//! `ThreadsView::show_local / show_store / set_remote / set_fixtures / set_enter_sends`
//! emits `ThreadsEvent::Open { channel, thread_ts }` / `Reply { .. }` / `PickEmoji(point)`
//! / `SlashCommand(Invocation)` / `Close`

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Point, Render, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px,
};
use slackless_core::api::{MessageContent, ThreadsView as SlackThreads};
use slackless_core::types::{ChannelId, Ts};

use crate::composer::{Composer, ComposerEvent, PendingAttachment};
use crate::fixtures::Fixtures;
use crate::slash::{Invocation, SHRUG, SlashKind};
use crate::threads_cards::{ThreadCard, card_view, remote_cards};

pub enum ThreadsEvent {
    /// A card was clicked: open the conversation with the thread panel on `thread_ts`.
    Open {
        channel: slackless_core::types::ChannelId,
        thread_ts: slackless_core::types::Ts,
    },
    /// The user replied; the card already shows the reply locally.
    Reply {
        channel: slackless_core::types::ChannelId,
        thread_ts: slackless_core::types::Ts,
        content: MessageContent,
    },
    /// The user replied with attached files (and optional message text).
    ReplyWithFiles {
        channel: slackless_core::types::ChannelId,
        thread_ts: slackless_core::types::Ts,
        content: Option<MessageContent>,
        files: Vec<PendingAttachment>,
    },
    /// The reply composer's emoji button asked for the picker.
    PickEmoji(Point<Pixels>),
    PickGif(Point<Pixels>),
    /// A slash command other than /shrug, which replies here; the root dispatches it.
    SlashCommand(Invocation),
    Close,
}

pub struct ThreadsView {
    fixtures: Arc<Fixtures>,
    cards: Vec<ThreadCard>,
    /// Slack's own list landed; later store derivations must not overwrite it.
    remote: bool,
    /// The card the shared reply composer is docked to.
    active: Option<usize>,
    composer: Entity<Composer>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ThreadsEvent> for ThreadsView {}

impl ThreadsView {
    pub fn new(fixtures: Arc<Fixtures>, window: &mut Window, cx: &mut App) -> Entity<ThreadsView> {
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
                composer,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Show `cards` (the fixture derivation) and drop any earlier remote list; call when the
    /// view opens.
    pub fn show_local(&mut self, cards: Vec<ThreadCard>, cx: &mut Context<Self>) {
        self.remote = false;
        self.cards = cards;
        self.active = None;
        cx.notify();
    }

    /// Show `cards` (the store derivation) unless Slack's own list already landed.
    pub fn show_store(&mut self, cards: Vec<ThreadCard>, cx: &mut Context<Self>) {
        if self.remote {
            return;
        }
        self.cards = cards;
        self.active = None;
        cx.notify();
    }

    /// Replace the cards with Slack's own followed threads (`subscriptions.thread.getView`).
    pub fn set_remote(&mut self, view: SlackThreads, cx: &mut Context<Self>) {
        self.remote = true;
        self.cards = remote_cards(view);
        if let Some(active) = self.active {
            self.active = (active < self.cards.len()).then_some(active);
        }
        cx.notify();
    }

    /// Live refresh: keep the cards, draw them from `fixtures`.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
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
        // The local echo shows the same blocks the reply posts with.
        let mut message = self.fixtures.local_message(
            &card.channel,
            content.text.clone(),
            Some(card.thread_ts.clone()),
        );
        message.blocks = content.blocks.clone();
        card.replies.push(message);
        card.reply_total += 1;
        cx.emit(ThreadsEvent::Reply {
            channel: card.channel.clone(),
            thread_ts: card.thread_ts.clone(),
            content,
        });
        cx.notify();
    }

    /// Dock the reply composer to card `ix`.
    pub(super) fn activate(
        &mut self,
        ix: Option<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active = ix;
        cx.notify();
    }
}

impl Render for ThreadsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, foreground) = (theme.border, theme.muted_foreground, theme.foreground);
        let unread: u32 = self.cards.iter().map(|card| card.unread).sum();
        let cards: Vec<(usize, ThreadCard)> = self.cards.iter().cloned().enumerate().collect();
        let mut list = v_flex()
            .id("threads-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        for (ix, card) in &cards {
            list = list.child(card_view(
                &self.fixtures,
                *ix,
                card,
                self.active == Some(*ix),
                &self.composer,
                cx,
            ));
        }
        let list = list.when(cards.is_empty(), |list| {
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
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .h(px(46.))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .font_weight(FontWeight::BOLD)
                            .child(
                                Icon::new(IconName::MessageSquareText)
                                    .text_color(foreground)
                                    .size(px(16.)),
                            )
                            .child("Threads")
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
                        Button::new("threads-close")
                            .ghost()
                            .with_size(Size::Small)
                            .icon(IconName::X)
                            .tooltip("Close")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ThreadsEvent::Close))),
                    ),
            )
            .child(div().h(px(1.)).border_b_1().border_color(border))
            .child(list)
    }
}
