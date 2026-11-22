//! Cmd+K quick switcher (mounted in a gpui-kit dialog by the root): a filter field over every
//! joined channel and DM. Typing filters by name (prefix matches first), Up/Down move the
//! selection, Enter or a click opens the conversation.
//!
//! `QuickSwitcher::new(Arc<Fixtures>, &mut Window, &mut App) -> Entity<QuickSwitcher>`
//! `QuickSwitcher::open(&mut self, &mut Window, &mut Context<Self>)` (reset + focus)
//! emits `SwitcherEvent::Open(ChannelId)`

use std::collections::HashSet;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use slackless_core::types::{ChannelId, ConversationKind, UserId};

use crate::avatars::{conversation_avatar, user_chip};
use crate::fixtures::Fixtures;

/// Most rows listed at once.
const MAX_ROWS: usize = 12;

pub enum SwitcherEvent {
    Open(ChannelId),
    /// A colleague with no open DM yet; the root opens or creates the DM.
    OpenUser(UserId),
}

enum Target {
    Channel(ChannelId),
    User(UserId),
}

pub struct QuickSwitcher {
    fixtures: Arc<Fixtures>,
    input: Entity<TextareaState>,
    query: String,
    selected: usize,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SwitcherEvent> for QuickSwitcher {}

impl QuickSwitcher {
    pub fn new(
        fixtures: Arc<Fixtures>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<QuickSwitcher> {
        cx.new(|cx| {
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Where would you like to go?")
                    .auto_grow(1, 1)
            });
            let subscriptions = vec![cx.subscribe_in(
                &input,
                window,
                |this: &mut QuickSwitcher, input, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => {
                        this.query = input.read(cx).value().trim().to_lowercase();
                        this.selected = 0;
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.confirm(cx),
                    _ => {}
                },
            )];
            QuickSwitcher {
                fixtures,
                input,
                query: String::new(),
                selected: 0,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Live refresh: match the query against the conversations in `fixtures`.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        cx.notify();
    }

    /// Clear the filter and focus the field; call when the dialog opens.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.query.clear();
        self.selected = 0;
        self.input.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }

    /// Conversations and colleagues matching the filter: prefix matches before substring
    /// matches, existing conversations ahead of people without an open DM.
    fn matches(&self) -> Vec<(Target, String)> {
        let with_dm: HashSet<&UserId> = self
            .fixtures
            .dms
            .iter()
            .filter_map(|dm| dm.user.as_ref())
            .collect();
        let candidates = self
            .fixtures
            .dms
            .iter()
            .chain(&self.fixtures.channels)
            .map(|c| {
                (
                    Target::Channel(c.id.clone()),
                    self.fixtures.conversation_label(c),
                )
            })
            .chain(
                self.fixtures
                    .users
                    .iter()
                    .filter(|u| !u.deleted && !with_dm.contains(&u.id))
                    .map(|u| {
                        let name = if u.id == self.fixtures.self_user {
                            format!("{} (you)", self.fixtures.name_of(u))
                        } else {
                            self.fixtures.name_of(u).to_owned()
                        };
                        (Target::User(u.id.clone()), name)
                    }),
            );
        crate::search_menu::rank_matches(&self.query, candidates, MAX_ROWS)
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        if let Some((target, _)) = self.matches().into_iter().nth(self.selected) {
            match target {
                Target::Channel(channel) => cx.emit(SwitcherEvent::Open(channel)),
                Target::User(user) => cx.emit(SwitcherEvent::OpenUser(user)),
            }
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.matches().len();
        match event.keystroke.key.as_str() {
            "down" if count > 0 => self.selected = (self.selected + 1) % count,
            "up" if count > 0 => self.selected = (self.selected + count - 1) % count,
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for QuickSwitcher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, hover, active, active_fg, popover) = (
            theme.border,
            theme.muted_foreground,
            theme.accent,
            theme.sidebar_accent,
            theme.sidebar_accent_foreground,
            theme.popover,
        );
        let matches = self.matches();
        let selected = self.selected.min(matches.len().saturating_sub(1));

        let rows: Vec<_> = matches
            .iter()
            .enumerate()
            .map(|(ix, (target, label))| {
                let on = ix == selected;
                let (leading, kind) = match target {
                    Target::Channel(channel) => {
                        let conversation = self.fixtures.conversation(channel);
                        let icon = match conversation.map(|c| c.kind()) {
                            Some(ConversationKind::PrivateChannel) => div()
                                .size(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::Lock)
                                        .text_color(if on { active_fg } else { muted })
                                        .size(px(14.)),
                                ),
                            Some(ConversationKind::PublicChannel) => div()
                                .size(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::Hash)
                                        .text_color(if on { active_fg } else { muted })
                                        .size(px(14.)),
                                ),
                            _ => conversation
                                .map(|c| {
                                    conversation_avatar(
                                        &self.fixtures,
                                        c,
                                        20.,
                                        if on { active } else { popover },
                                        cx,
                                    )
                                })
                                .unwrap_or_else(div),
                        };
                        let kind = match conversation.map(|c| c.kind()) {
                            Some(ConversationKind::Im) => "Direct message",
                            Some(ConversationKind::Mpim) => "Group DM",
                            _ => "Channel",
                        };
                        (icon, kind)
                    }
                    Target::User(user) => {
                        let kind = if *user == self.fixtures.self_user {
                            "Direct message"
                        } else {
                            "Person"
                        };
                        (user_chip(&self.fixtures, user, 20.), kind)
                    }
                };
                let target_clone = match target {
                    Target::Channel(c) => Target::Channel(c.clone()),
                    Target::User(u) => Target::User(u.clone()),
                };
                h_flex()
                    .id(("switcher-row", ix))
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .when(on, |row| row.bg(active).text_color(active_fg))
                    .when(!on, |row| row.hover(|el| el.bg(hover)))
                    .child(leading)
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(label.trim_start_matches("# ").to_owned()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if on { active_fg } else { muted })
                            .child(kind),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| match &target_clone {
                        Target::Channel(channel) => cx.emit(SwitcherEvent::Open(channel.clone())),
                        Target::User(user) => cx.emit(SwitcherEvent::OpenUser(user.clone())),
                    }))
            })
            .collect();

        let empty = rows.is_empty().then(|| {
            div()
                .px_3()
                .py_4()
                .text_sm()
                .text_color(muted)
                .child("No conversations match.")
        });

        v_flex()
            .id("quick-switcher")
            .w_full()
            .gap_2()
            .capture_key_down(cx.listener(Self::on_key))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .child(Icon::new(IconName::Search).text_color(muted).size(px(16.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Textarea::new(&self.input).appearance(false).bordered(false)),
                    ),
            )
            .child(v_flex().gap_0p5().children(rows).children(empty))
            .child(div().pt_1().text_xs().text_color(muted).child(
                "\u{2191}\u{2193} to navigate \u{b7} \u{21B5} to open \u{b7} esc to dismiss",
            ))
    }
}
