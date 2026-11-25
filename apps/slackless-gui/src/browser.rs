//! Channel browser: every workspace channel (joined and not) with its name, topic and member
//! count, a filter field, and a Join affordance. The root mounts it inside a gpui-kit dialog
//! (`window.open_dialog`, gpui-kit-guide section 7).
//!
//! `ChannelBrowser::new(Arc<Fixtures>, &mut Window, &mut App) -> Entity<ChannelBrowser>`
//! emits `ChannelBrowserEvent::Join(ChannelId)` (already marked joined here) and
//! `ChannelBrowserEvent::Open(ChannelId)` (a row was clicked).

use std::collections::HashSet;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px,
};
use slackless_core::types::{ChannelId, Conversation, ConversationKind};

use crate::fixtures::Fixtures;

pub enum ChannelBrowserEvent {
    Join(ChannelId),
    Open(ChannelId),
}

pub struct ChannelBrowser {
    fixtures: Arc<Fixtures>,
    /// Channels joined from this browser during the session.
    joined: HashSet<ChannelId>,
    filter: Entity<TextareaState>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ChannelBrowserEvent> for ChannelBrowser {}

impl ChannelBrowser {
    pub fn new(
        fixtures: Arc<Fixtures>,
        window: &mut Window,
        cx: &mut gpui_kit::gpui::App,
    ) -> Entity<ChannelBrowser> {
        cx.new(|cx| {
            let filter = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Search for channels")
                    .auto_grow(1, 1)
            });
            let subscriptions = vec![cx.subscribe_in(
                &filter,
                window,
                |_: &mut ChannelBrowser, _, event: &InputEvent, _, cx| {
                    if let InputEvent::Change = event {
                        cx.notify();
                    }
                },
            )];
            ChannelBrowser {
                fixtures,
                joined: HashSet::new(),
                filter,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Live refresh: list the channels in `fixtures`, keeping the typed filter.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        cx.notify();
    }

    fn is_member(&self, conversation: &Conversation) -> bool {
        conversation.is_member || self.joined.contains(&conversation.id)
    }

    /// Every channel matching the filter (name or topic, case-insensitive), by name.
    fn visible(&self, cx: &Context<Self>) -> Vec<Conversation> {
        let needle = self.filter.read(cx).value().trim().to_lowercase();
        let mut channels: Vec<Conversation> = self
            .fixtures
            .channels
            .iter()
            .chain(&self.fixtures.directory)
            .filter(|c| {
                needle.is_empty()
                    || c.name
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&needle)
                    || c.topic
                        .as_ref()
                        .is_some_and(|t| t.value.to_lowercase().contains(&needle))
            })
            .cloned()
            .collect();
        channels.sort_by(|a, b| a.name.cmp(&b.name));
        channels
    }

    fn row(&self, conversation: &Conversation, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (foreground, muted, success, border, hover) = (
            theme.foreground,
            theme.muted_foreground,
            theme.success,
            theme.border,
            theme.accent,
        );
        let id = conversation.id.clone();
        let member = self.is_member(conversation);
        let icon = if conversation.kind() == ConversationKind::PrivateChannel {
            IconName::Lock
        } else {
            IconName::Hash
        };
        let members = conversation.num_members.unwrap_or(0);
        let topic = conversation
            .topic
            .as_ref()
            .map(|t| t.value.clone())
            .unwrap_or_default();
        let meta = if topic.is_empty() {
            format!("{members} members")
        } else {
            format!("{members} members · {topic}")
        };

        let action = if member {
            h_flex()
                .flex_none()
                .gap_1()
                .items_center()
                .text_xs()
                .text_color(success)
                .child(Icon::new(IconName::Check).text_color(success).size(px(14.)))
                .child("Joined")
                .into_any_element()
        } else {
            let join_id = id.clone();
            Button::new(SharedString::from(format!("join-{id}")))
                .primary()
                .label("Join")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.joined.insert(join_id.clone());
                    cx.emit(ChannelBrowserEvent::Join(join_id.clone()));
                    cx.notify();
                }))
                .into_any_element()
        };

        let open_id = id.clone();
        h_flex()
            .id(SharedString::from(format!("browse-{id}")))
            .items_center()
            .gap_3()
            .px_4()
            .py_2p5()
            .border_b_1()
            .border_color(border)
            .cursor_pointer()
            .hover(|row| row.bg(hover))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .text_color(foreground)
                            .child(Icon::new(icon).text_color(foreground).size(px(14.)))
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::BOLD)
                                    .child(conversation.name.clone().unwrap_or_default()),
                            ),
                    )
                    .child(div().truncate().text_xs().text_color(muted).child(meta)),
            )
            .child(action)
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(ChannelBrowserEvent::Open(open_id.clone()));
            }))
    }
}

impl Render for ChannelBrowser {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let channels = self.visible(cx);
        let count = channels.len();
        let rows: Vec<_> = channels.iter().map(|c| self.row(c, cx)).collect();

        v_flex()
            .w_full()
            .gap_2()
            .text_sm()
            .child(crate::preferences::search_input_box(&self.filter, cx).mx_4())
            .child(
                div()
                    .px_4()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("{count} channels")),
            )
            .child(
                v_flex()
                    .id("browse-list")
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .border_t_1()
                    .border_color(border)
                    .children(rows)
                    .when(count == 0, |list| {
                        list.child(
                            div()
                                .p_6()
                                .text_color(muted)
                                .child("No channels match your search."),
                        )
                    }),
            )
    }
}
