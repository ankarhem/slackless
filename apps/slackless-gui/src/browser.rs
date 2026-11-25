//! Channel browser: every workspace channel (joined and not) with its name, topic and member
//! count, a filter field, and a Join affordance. The root mounts it inside a gpui-kit dialog
//! (`window.open_dialog`, gpui-kit-guide section 7).
//!
//! `ChannelBrowser::new(Arc<Fixtures>, &mut Window, &mut App) -> Entity<ChannelBrowser>`
//! emits `ChannelBrowserEvent::Join(ChannelId)` (already marked joined here) and
//! `ChannelBrowserEvent::Open(ChannelId)` (a row was clicked).

use std::collections::HashSet;
use std::sync::Arc;

use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px,
};
use slackless_core::types::{ChannelId, Conversation};

use crate::directory_rows::{ChannelRowSpec, filter_channels, render_channel_row};
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
        let needle = self.filter.read(cx).value();
        filter_channels(&self.fixtures, &needle)
    }

    fn row(&self, conversation: &Conversation, cx: &mut Context<Self>) -> AnyElement {
        let id = conversation.id.clone();
        let join_id = id.clone();
        let open_id = id.clone();
        render_channel_row(
            conversation,
            ChannelRowSpec {
                row_id: SharedString::from(format!("browse-{id}")).into(),
                join_id: SharedString::from(format!("join-{id}")),
                is_member: self.is_member(conversation),
                inline_icon: true,
                compact_join: false,
            },
            cx.listener(move |this, _, _, cx| {
                this.joined.insert(join_id.clone());
                cx.emit(ChannelBrowserEvent::Join(join_id.clone()));
                cx.notify();
            }),
            cx.listener(move |_, _, _, cx| {
                cx.emit(ChannelBrowserEvent::Open(open_id.clone()));
            }),
            cx,
        )
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
            .child(crate::ui::forms::search_input_box(&self.filter, cx).mx_4())
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
