//! The "New message" view (the workspace header's pen): Slack's compose - a "To:" field with
//! a suggestion dropdown over recent conversations and every person (Up/Down move, Enter or
//! a click picks), the message composer below, and a send that posts into the picked
//! conversation and navigates there. The recipient model and dropdown rows live in
//! [`crate::compose_targets`].
//!
//! A picked person with no existing DM is routed through the root, which opens one first
//! (`conversations.open`) and delivers the text after.
//!
//! `NewMessageView::new(Arc<Fixtures>, &mut Window, &mut App) -> Entity<NewMessageView>`
//! `NewMessageView::open / set_fixtures / set_enter_sends / insert_emoji`
//! emits `NewMessageEvent::Send { channel, text }` / `OpenDm { user, text }` /
//! `PickEmoji(point)` / `SlashCommand(Invocation)` / `Close`

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render, Styled as _,
    Subscription, Window, div, px,
};
use slackless_core::api::MessageContent;
use slackless_core::types::{ChannelId, UserId};

use crate::compose_targets::{Picked, suggestion_rows, suggestions};
use crate::composer::{Composer, ComposerEvent, PendingAttachment};
use crate::fixtures::Fixtures;
use crate::slash::{Invocation, SHRUG, SlashKind};

pub enum NewMessageEvent {
    Send {
        channel: ChannelId,
        content: MessageContent,
    },
    /// Send attached files (with optional message text) into an existing conversation.
    SendWithFiles {
        channel: ChannelId,
        content: Option<MessageContent>,
        files: Vec<PendingAttachment>,
    },
    /// Open (or reuse) a 1:1 DM, self-DM, or group DM with `users` and deliver `content` there.
    OpenDm {
        users: Vec<UserId>,
        content: MessageContent,
    },
    /// The composer's emoji button asked for the picker.
    PickEmoji(Point<Pixels>),
    /// A slash command; the root dispatches it.
    SlashCommand(Invocation),
    Close,
}

pub struct NewMessageView {
    pub(super) fixtures: Arc<Fixtures>,
    pub(super) to: Entity<TextareaState>,
    pub(super) query: String,
    pub(super) selected: usize,
    pub(super) picked: Vec<Picked>,
    pub(super) composer: Entity<Composer>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<NewMessageEvent> for NewMessageView {}

impl NewMessageView {
    pub fn new(
        fixtures: Arc<Fixtures>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<NewMessageView> {
        cx.new(|cx| {
            let to = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("#a-channel, @somebody, or somebody@example.com")
                    .auto_grow(1, 1)
            });
            let composer =
                Composer::new("Start a new message".into(), fixtures.clone(), window, cx);
            composer.update(cx, |composer, cx| composer.set_scheduling(false, cx));
            let subscriptions =
                vec![
                cx.subscribe_in(
                    &to,
                    window,
                    |this: &mut NewMessageView, to, event: &InputEvent, window, cx| match event {
                        InputEvent::Change => this.on_query_change(to, cx),
                        InputEvent::PressEnter { .. } => this.confirm(window, cx),
                        _ => {}
                    },
                ),
                cx.subscribe_in(
                    &composer,
                    window,
                    |this: &mut NewMessageView, _, event: &ComposerEvent, window, cx| match event {
                        ComposerEvent::Submit(content) => this.send(content.clone(), window, cx),
                        ComposerEvent::SubmitWithFiles { content, files } => {
                            if let [Picked {
                                recipient: crate::compose_targets::Recipient::Conversation(channel),
                                ..
                            }] = this.picked.as_slice()
                            {
                                cx.emit(NewMessageEvent::SendWithFiles {
                                    channel: channel.clone(),
                                    content: content.clone(),
                                    files: files.clone(),
                                });
                            }
                        }
                        ComposerEvent::OpenEmojiPicker(position) => {
                            cx.emit(NewMessageEvent::PickEmoji(*position));
                        }
                        ComposerEvent::SlashCommand(invocation) => {
                            // /shrug posts right away, like Slack's.
                            match invocation.command.map(|command| command.kind) {
                                Some(SlashKind::Shrug) => {
                                    let text = format!("{} {SHRUG}", invocation.args.trim());
                                    this.send(MessageContent::plain(text.trim_start()), window, cx);
                                }
                                _ => cx.emit(NewMessageEvent::SlashCommand(invocation.clone())),
                            }
                        }
                        // Scheduling is turned off here, and a new message has nothing to edit.
                        ComposerEvent::Schedule { .. }
                        | ComposerEvent::EditLast
                        | ComposerEvent::Typing
                        | ComposerEvent::Cancel => {}
                    },
                ),
            ];
            NewMessageView {
                fixtures,
                to,
                query: String::new(),
                selected: 0,
                picked: Vec::new(),
                composer,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Reset and focus the To field; call when the view opens.
    /// Show the view empty, or with `draft` in the composer (a forwarded message's link).
    pub fn open(&mut self, draft: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.query.clear();
        self.selected = 0;
        self.picked.clear();
        self.to.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus_handle(cx).focus(window, cx);
        });
        self.composer
            .update(cx, |composer, cx| composer.load_draft(draft, cx));
        cx.notify();
    }

    /// Live refresh: match the query against the conversations and people in `fixtures`.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        cx.notify();
    }

    /// Forward the Enter-to-send preference to the composer.
    pub fn set_enter_sends(&mut self, enter_sends: bool, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |composer, cx| composer.set_enter_sends(enter_sends, cx));
    }

    /// Insert an emoji pick (or /shrug) into the composer.
    pub fn insert_emoji(
        &mut self,
        choice: &crate::emoji::EmojiChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composer
            .update(cx, |composer, cx| composer.insert_emoji(choice, window, cx));
    }
}

impl Render for NewMessageView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, foreground, popover, secondary) = (
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.popover,
            theme.secondary,
        );
        let suggestions = suggestions(&self.fixtures, &self.picked, &self.query);
        let selected = self.selected.min(suggestions.len().saturating_sub(1));
        let picking = self.picked.is_empty()
            || (!self.query.is_empty() && !suggestions.is_empty())
            || (self
                .picked
                .iter()
                .all(|p| matches!(p.recipient, crate::compose_targets::Recipient::User(_)))
                && self.to.read(cx).focus_handle(cx).is_focused(_window));
        let rows = suggestion_rows(&self.fixtures, &suggestions, selected, picking, cx);
        let chips: Vec<_> = self
            .picked
            .iter()
            .enumerate()
            .map(|(ix, picked)| {
                h_flex()
                    .id(("compose-chip", ix))
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .bg(secondary)
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(foreground)
                    .child(picked.label.clone())
                    .child(
                        Button::new(("compose-chip-rm", ix))
                            .ghost()
                            .with_size(Size::XSmall)
                            .icon(IconName::X)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_picked(ix, cx);
                            })),
                    )
            })
            .collect();

        v_flex()
            .id("new-message")
            .size_full()
            .capture_key_down(cx.listener(Self::on_key))
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
                                Icon::new(IconName::SquarePen)
                                    .text_color(foreground)
                                    .size(px(16.)),
                            )
                            .child("New message"),
                    )
                    .child(
                        Button::new("compose-close")
                            .ghost()
                            .with_size(Size::Small)
                            .icon(IconName::X)
                            .tooltip("Close")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(NewMessageEvent::Close))),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .pt_2()
                    .child(
                        h_flex()
                            .flex_wrap()
                            .items_center()
                            .gap_1p5()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .child(div().flex_none().text_sm().text_color(muted).child("To:"))
                            .children(chips)
                            .child(
                                div().flex_1().min_w(px(140.)).child(
                                    Textarea::new(&self.to).appearance(false).bordered(false),
                                ),
                            ),
                    )
                    .when(picking && !suggestions.is_empty(), |column| {
                        column.child(
                            v_flex()
                                .mx_2()
                                .my_1()
                                .p_1()
                                .gap_0p5()
                                .rounded_md()
                                .border_1()
                                .border_color(border)
                                .bg(popover)
                                .children(rows),
                        )
                    })
                    .when(
                        picking && suggestions.is_empty() && !self.query.is_empty(),
                        |column| {
                            column.child(
                                div()
                                    .mx_2()
                                    .my_1()
                                    .px_3()
                                    .py_2()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("No matches. Pick a suggestion to send."),
                            )
                        },
                    ),
            )
            .child(self.composer.clone())
    }
}
