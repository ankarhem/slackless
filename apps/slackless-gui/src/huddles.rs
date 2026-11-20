//! The Huddles page (the sidebar's "Huddles" row): recent huddles, each opening its
//! conversation, and Slack's empty state when there are none. Fixture mode lists the bundled
//! sample workspace's live huddle; live mode derives recent huddles from the stored
//! `huddle_thread` trail messages (every huddle leaves one in its channel; its `room` object
//! says who joined and whether it ended - see docs/research/api-huddles.md). Audio/video
//! stay out of scope here, as in [`crate::huddle`]: opening the conversation is the working
//! action.
//!
//! `HuddlesView::new(Arc<Fixtures>, &mut App) -> Entity<HuddlesView>`
//! `HuddlesView::show_local(&mut self, &Fixtures, &mut Context<Self>)` (fixture mode)
//! `HuddlesView::set_huddles(&mut self, Vec<RecentHuddle>, &mut Context<Self>)` (live scan)
//! emits `HuddlesEvent::Open(ChannelId)` / `HuddlesEvent::Close`
//! `store_huddles(&Store, &TeamId) -> Vec<RecentHuddle>` runs the live scan off the UI thread.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Window,
    div, px,
};
use slackless_core::store::Store;
use slackless_core::types::{ChannelId, Message, MessageSubtype, TeamId, Ts, UserId};

use crate::blockkit::serde_json::Value;
use crate::fixtures::Fixtures;
use crate::huddle;
use crate::messages::list_stamp;

/// Trail messages scanned per conversation in the live derivation.
const SCAN_LIMIT: usize = 200;

/// One recent huddle: where it ran and whether it is still going.
#[derive(Clone)]
pub struct RecentHuddle {
    pub channel: ChannelId,
    /// When the huddle started (its trail message ts).
    pub ts: Ts,
    pub participants: Vec<UserId>,
    pub live: bool,
}

pub enum HuddlesEvent {
    /// Open the huddle's conversation.
    Open(ChannelId),
    Close,
}

pub struct HuddlesView {
    fixtures: Arc<Fixtures>,
    huddles: Vec<RecentHuddle>,
}

impl EventEmitter<HuddlesEvent> for HuddlesView {}

impl HuddlesView {
    pub fn new(fixtures: Arc<Fixtures>, cx: &mut App) -> Entity<HuddlesView> {
        cx.new(|_| HuddlesView {
            fixtures,
            huddles: Vec::new(),
        })
    }

    /// Live refresh: keep the rows, label them from `fixtures`.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        cx.notify();
    }

    /// Fixture mode: the bundled sample workspace's live huddle.
    pub fn show_local(&mut self, fixtures: &Fixtures, cx: &mut Context<Self>) {
        self.huddles = huddle::fixture(fixtures)
            .map(|huddle| RecentHuddle {
                channel: huddle.channel,
                ts: Ts::default(),
                participants: huddle.participants,
                live: true,
            })
            .into_iter()
            .collect();
        cx.notify();
    }

    /// Live mode: the store scan's recent huddles.
    pub fn set_huddles(&mut self, huddles: Vec<RecentHuddle>, cx: &mut Context<Self>) {
        self.huddles = huddles;
        cx.notify();
    }

    fn row(&self, ix: usize, huddle: &RecentHuddle, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, foreground, success, hover) = (
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.success,
            theme.accent,
        );
        let label = self
            .fixtures
            .conversation(&huddle.channel)
            .map(|c| self.fixtures.conversation_label(c))
            .unwrap_or_else(|| huddle.channel.to_string());
        let names: Vec<&str> = huddle
            .participants
            .iter()
            .map(|user| self.fixtures.user_name(user))
            .collect();
        let meta = if names.is_empty() {
            format!("{} · {}", state_word(huddle.live), list_stamp(&huddle.ts))
        } else {
            format!(
                "{} · {} · {}",
                state_word(huddle.live),
                names.join(", "),
                list_stamp(&huddle.ts)
            )
        };
        let open = huddle.channel.clone();
        h_flex()
            .id(("huddle-row", ix))
            .items_center()
            .gap_3()
            .px_4()
            .py_2p5()
            .border_b_1()
            .border_color(border)
            .cursor_pointer()
            .hover(|row| row.bg(hover))
            .child(
                Icon::new(IconName::Headphones)
                    .text_color(if huddle.live { success } else { muted })
                    .size(px(18.)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex().gap_2().child(
                            div()
                                .truncate()
                                .font_weight(FontWeight::BOLD)
                                .text_color(foreground)
                                .child(label.trim_start_matches("# ").to_owned()),
                        ),
                    )
                    .child(div().truncate().text_xs().text_color(muted).child(meta)),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(if huddle.live { success } else { muted })
                    .child(state_word(huddle.live)),
            )
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(HuddlesEvent::Open(open.clone()))))
    }
}

impl Render for HuddlesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, foreground) = (theme.border, theme.muted_foreground, theme.foreground);
        let rows: Vec<_> = self
            .huddles
            .iter()
            .enumerate()
            .map(|(ix, huddle)| self.row(ix, huddle, cx))
            .collect();

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
                                Icon::new(IconName::Headphones)
                                    .text_color(foreground)
                                    .size(px(16.)),
                            )
                            .child("Huddles"),
                    )
                    .child(
                        Button::new("huddles-close")
                            .ghost()
                            .with_size(Size::Small)
                            .icon(IconName::X)
                            .tooltip("Close")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddlesEvent::Close))),
                    ),
            )
            .child(div().h(px(1.)).border_b_1().border_color(border))
            .child(
                v_flex()
                    .id("huddles-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .when(self.huddles.is_empty(), |list| {
                        list.child(
                            v_flex()
                                .items_center()
                                .gap_2()
                                .py_10()
                                .text_sm()
                                .text_color(muted)
                                .child(
                                    Icon::new(IconName::Headphones)
                                        .text_color(muted)
                                        .size(px(28.)),
                                )
                                .child("No recent huddles.")
                                .child(
                                    div().text_xs().child(
                                        "Start one from a conversation's headphones button.",
                                    ),
                                ),
                        )
                    }),
            )
    }
}

/// The live/ended word of a row's meta line.
fn state_word(live: bool) -> &'static str {
    if live { "Live now" } else { "Ended" }
}

/// The `room` object of a `huddle_thread` trail message.
fn room_of(message: &Message) -> Option<&Value> {
    message.extra.get("room")
}

/// Recent huddles from the stored `huddle_thread` trail messages, newest first.
pub fn store_huddles(store: &Store, team: &TeamId) -> Vec<RecentHuddle> {
    let Ok(conversations) = store.conversations(team) else {
        return Vec::new();
    };
    let mut huddles = Vec::new();
    for conversation in &conversations {
        let Ok(rows) = store.channel_history(team, &conversation.id, None, SCAN_LIMIT) else {
            continue;
        };
        for row in rows {
            let message = &row.message;
            if row.deleted || message.subtype != Some(MessageSubtype::HuddleThread) {
                continue;
            }
            let room = room_of(message);
            let live = room
                .and_then(|room| room.get("has_ended"))
                .and_then(Value::as_bool)
                .is_some_and(|ended| !ended);
            let participants = room
                .and_then(|room| room.get("participants"))
                .and_then(Value::as_array)
                .map_or_else(Vec::new, |ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(UserId::from)
                        .collect()
                });
            huddles.push(RecentHuddle {
                channel: conversation.id.clone(),
                ts: message.ts.clone(),
                participants,
                live,
            });
        }
    }
    huddles.sort_by_key(|huddle| std::cmp::Reverse(huddle.ts.clone()));
    huddles
}
