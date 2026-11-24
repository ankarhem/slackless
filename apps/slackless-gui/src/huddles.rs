//! The Huddles page (the sidebar's "Huddles" row): active live huddle cards, quick-start
//! suggestion, filterable recent huddles in Slack's grouped card layout, and empty state.
//!
//! `HuddlesView::new(Arc<Fixtures>, &mut App) -> Entity<HuddlesView>`
//! `HuddlesView::show_local(&mut self, &Fixtures, &mut Context<Self>)` (fixture mode)
//! `HuddlesView::set_huddles(&mut self, Vec<RecentHuddle>, &mut Context<Self>)` (live scan)
//! emits `HuddlesEvent::Open(ChannelId)` / `Join(ChannelId)` / `NewHuddle` / `OpenThread` / `Close`
//! `store_huddles(&Store, &TeamId) -> Vec<RecentHuddle>` runs the live scan off the UI thread.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Window,
    div, linear_color_stop, linear_gradient, px, rgb,
};
use slackless_core::store::Store;
use slackless_core::types::{
    ChannelId, ConversationKind, Message, MessageSubtype, TeamId, Ts, UserId,
};

use crate::avatars::user_chip;
use crate::blockkit::serde_json::Value;
use crate::fixtures::Fixtures;
use crate::huddle;
use crate::messages::list_stamp;

fn format_huddle_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        let m = secs / 60;
        let s = secs % 60;
        if s == 0 {
            format!("{m}m")
        } else {
            format!("{m}m {s}s")
        }
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h {m}m")
        }
    }
}

/// Trail messages scanned per conversation in the live derivation.
const SCAN_LIMIT: usize = 200;

/// One recent or live huddle: where it ran, who joined, duration, and thread replies.
#[derive(Clone)]
pub struct RecentHuddle {
    pub channel: ChannelId,
    /// When the huddle started (its trail message ts).
    pub ts: Ts,
    pub participants: Vec<UserId>,
    pub live: bool,
    pub duration_secs: Option<u64>,
    pub reply_count: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HuddleFilter {
    All,
    Channels,
    Dms,
}

pub enum HuddlesEvent {
    /// Open the huddle's conversation.
    Open(ChannelId),
    /// Join an active huddle or start a huddle in a channel/DM immediately.
    Join(ChannelId),
    /// Start a new huddle from the "+ New Huddle" header action.
    NewHuddle,
    /// Open the huddle's dedicated thread (`F-COLLAB-014`).
    OpenThread(ChannelId, Ts),
    Close,
}

pub struct HuddlesView {
    fixtures: Arc<Fixtures>,
    huddles: Vec<RecentHuddle>,
    filter: HuddleFilter,
    loading: bool,
}

impl EventEmitter<HuddlesEvent> for HuddlesView {}

impl HuddlesView {
    pub fn new(fixtures: Arc<Fixtures>, cx: &mut App) -> Entity<HuddlesView> {
        cx.new(|_| HuddlesView {
            fixtures,
            huddles: Vec::new(),
            filter: HuddleFilter::All,
            loading: false,
        })
    }

    /// Live refresh: keep the rows, label them from `fixtures`.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        cx.notify();
    }

    /// Live mode open: keep already-loaded rows visible while refreshing from SQLite, showing
    /// a loading state only on the very first empty visit.
    pub fn begin_live_load(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        if self.huddles.is_empty() {
            self.loading = true;
        }
        cx.notify();
    }

    /// Fixture mode: the bundled sample workspace's live huddle.
    pub fn show_local(&mut self, fixtures: &Fixtures, cx: &mut Context<Self>) {
        self.loading = false;
        self.huddles = huddle::fixture(fixtures)
            .map(|huddle| {
                let ts = fixtures
                    .messages
                    .get(&huddle.channel)
                    .and_then(|msgs| msgs.last())
                    .map(|m| m.ts.clone())
                    .unwrap_or_default();
                RecentHuddle {
                    channel: huddle.channel,
                    ts,
                    participants: huddle.participants,
                    live: true,
                    duration_secs: None,
                    reply_count: 0,
                }
            })
            .into_iter()
            .collect();
        cx.notify();
    }

    /// Live mode: the store scan's recent huddles.
    pub fn set_huddles(&mut self, huddles: Vec<RecentHuddle>, cx: &mut Context<Self>) {
        self.loading = false;
        self.huddles = huddles;
        cx.notify();
    }

    fn is_dm(&self, channel: &ChannelId) -> bool {
        self.fixtures
            .conversation(channel)
            .is_some_and(|c| matches!(c.kind(), ConversationKind::Im | ConversationKind::Mpim))
    }

    fn conversation_heading(&self, channel: &ChannelId) -> (Option<IconName>, String) {
        let Some(conv) = self.fixtures.conversation(channel) else {
            return (None, channel.to_string());
        };
        let label = self.fixtures.conversation_label(conv);
        match conv.kind() {
            ConversationKind::PrivateChannel => (
                Some(IconName::Lock),
                label.trim_start_matches("# ").to_owned(),
            ),
            ConversationKind::PublicChannel => {
                (None, format!("# {}", label.trim_start_matches("# ")))
            }
            ConversationKind::Im | ConversationKind::Mpim => (None, label),
        }
    }

    /// Frequent recent DM huddle partner for Slack's quick-start card ("Start a huddle with …?").
    fn suggested_peer(&self) -> Option<(ChannelId, UserId, usize)> {
        let mut counts: HashMap<ChannelId, usize> = HashMap::new();
        for h in &self.huddles {
            if !h.live && self.is_dm(&h.channel) {
                *counts.entry(h.channel.clone()).or_default() += 1;
            }
        }
        let (ch, freq) = counts.into_iter().max_by_key(|(_, n)| *n)?;
        let conv = self.fixtures.conversation(&ch)?;
        let peer = conv.user.clone().or_else(|| {
            self.fixtures
                .peers(conv)
                .into_iter()
                .find(|u| **u != self.fixtures.self_user)
                .cloned()
        })?;
        Some((ch, peer, freq))
    }

    fn render_live_card(
        &self,
        ix: usize,
        huddle: &RecentHuddle,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let dark = theme.mode.is_dark();
        let (bg, border, fg, muted) = (
            theme.background,
            theme.border,
            theme.foreground,
            theme.muted_foreground,
        );
        let banner_bg = if dark {
            linear_gradient(
                135.,
                linear_color_stop(Hsla::from(rgb(0x1E3A34)), 0.),
                linear_color_stop(Hsla::from(rgb(0x284359)), 1.),
            )
        } else {
            linear_gradient(
                135.,
                linear_color_stop(Hsla::from(rgb(0x9BC7C9)), 0.),
                linear_color_stop(Hsla::from(rgb(0xD5E2B8)), 1.),
            )
        };
        let (lock_icon, title) = self.conversation_heading(&huddle.channel);
        let people_label = match huddle.participants.len() {
            0 => "Live now".to_owned(),
            1 => "1 person".to_owned(),
            n => format!("{n} people"),
        };
        let elapsed = list_stamp(&huddle.ts);
        let join_ch = huddle.channel.clone();
        let open_ch = huddle.channel.clone();

        v_flex()
            .id(("live-huddle-card", ix))
            .w(px(276.))
            .rounded_xl()
            .overflow_hidden()
            .border_1()
            .border_color(border)
            .bg(bg)
            .shadow_xs()
            .child(
                div()
                    .id(("live-huddle-banner", ix))
                    .relative()
                    .w_full()
                    .h(px(78.))
                    .rounded_tl(px(11.))
                    .rounded_tr(px(11.))
                    .overflow_hidden()
                    .bg(banner_bg)
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(HuddlesEvent::Open(open_ch.clone()));
                    }))
                    .when(!elapsed.is_empty(), |el| {
                        el.child(
                            div()
                                .absolute()
                                .left_2()
                                .top_2()
                                .px_2()
                                .py_0p5()
                                .rounded_full()
                                .bg(Hsla::from(rgb(0x14171C)).opacity(0.75))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(Hsla::from(rgb(0xFFFFFF)))
                                .child(elapsed),
                        )
                    })
                    .child(
                        h_flex()
                            .size_full()
                            .items_center()
                            .justify_center()
                            .children(huddle.participants.iter().take(4).enumerate().map(
                                |(pix, uid)| {
                                    div()
                                        .when(pix > 0, |el| el.ml(px(-6.)))
                                        .rounded_lg()
                                        .overflow_hidden()
                                        .border_2()
                                        .border_color(bg)
                                        .child(user_chip(&self.fixtures, uid, 36.))
                                },
                            )),
                    ),
            )
            .child(
                v_flex()
                    .p_3()
                    .gap_2p5()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .when_some(lock_icon, |row, ic| {
                                        row.child(Icon::new(ic).size(px(13.)).text_color(fg))
                                    })
                                    .child(
                                        div()
                                            .truncate()
                                            .text_sm()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(fg)
                                            .child(title),
                                    ),
                            )
                            .child(div().text_xs().text_color(muted).child(people_label)),
                    )
                    .child(
                        Button::new(("live-huddle-join", ix))
                            .primary()
                            .w_full()
                            .with_size(Size::Small)
                            .label("Join Huddle")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(HuddlesEvent::Join(join_ch.clone()));
                            })),
                    ),
            )
    }

    fn render_quick_start_card(
        &self,
        channel: ChannelId,
        peer: UserId,
        count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (muted_bg, border, fg, muted, hover) = (
            theme.muted,
            theme.border,
            theme.foreground,
            theme.muted_foreground,
            theme.accent,
        );
        let peer_name = self.fixtures.user_name(&peer).to_owned();
        let subtitle = if count == 1 {
            format!("You huddled with {peer_name} recently")
        } else {
            format!("You huddled with {peer_name} {count} times recently")
        };
        v_flex()
            .id("huddle-quick-start-card")
            .w(px(276.))
            .min_h(px(176.))
            .p_4()
            .gap_3()
            .items_center()
            .justify_center()
            .rounded_xl()
            .border_1()
            .border_color(border)
            .bg(muted_bg)
            .hover(move |el| el.bg(hover))
            .cursor_pointer()
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(HuddlesEvent::Join(channel.clone()));
            }))
            .child(
                h_flex()
                    .items_center()
                    .child(
                        div()
                            .rounded_lg()
                            .overflow_hidden()
                            .border_2()
                            .border_color(muted_bg)
                            .child(user_chip(&self.fixtures, &self.fixtures.self_user, 36.)),
                    )
                    .child(
                        div()
                            .ml(px(-6.))
                            .rounded_lg()
                            .overflow_hidden()
                            .border_2()
                            .border_color(muted_bg)
                            .child(user_chip(&self.fixtures, &peer, 36.)),
                    ),
            )
            .child(
                v_flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(fg)
                            .text_center()
                            .child(format!("Start a huddle with {peer_name}?")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .text_center()
                            .child(subtitle),
                    ),
            )
    }

    fn recent_row(
        &self,
        ix: usize,
        is_last: bool,
        huddle: &RecentHuddle,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, muted_bg, fg, link, hover, bg) = (
            theme.border,
            theme.muted_foreground,
            theme.muted,
            theme.foreground,
            theme.link,
            theme.accent,
            theme.background,
        );
        let (lock_icon, title) = self.conversation_heading(&huddle.channel);
        let stamp = list_stamp(&huddle.ts);
        let duration = huddle.duration_secs.map(format_huddle_duration);
        let mut meta_parts: Vec<String> = Vec::new();
        if !stamp.is_empty() {
            meta_parts.push(stamp);
        }
        if let Some(d) = duration {
            meta_parts.push(d);
        }
        let meta = meta_parts.join(" · ");

        let open = huddle.channel.clone();
        let thread_target =
            (!huddle.ts.as_str().is_empty()).then(|| (huddle.channel.clone(), huddle.ts.clone()));
        let visible_avatars: Vec<&UserId> = huddle.participants.iter().take(3).collect();
        let overflow = huddle.participants.len().saturating_sub(3);

        h_flex()
            .id(("huddle-row", ix))
            .items_center()
            .gap_3()
            .px_4()
            .py_3()
            .when(!is_last, |row| row.border_b_1().border_color(border))
            .cursor_pointer()
            .hover(move |row| row.bg(hover))
            .child(
                div()
                    .size(px(36.))
                    .flex_none()
                    .rounded_lg()
                    .bg(muted_bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::Headphones)
                            .text_color(fg.opacity(0.8))
                            .size(px(17.)),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .when_some(lock_icon, |row, ic| {
                                row.child(Icon::new(ic).size(px(13.)).text_color(fg))
                            })
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(fg)
                                    .child(title),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .text_color(muted)
                            .when(!meta.is_empty(), |row| row.child(meta))
                            .when(huddle.reply_count > 0 && thread_target.is_some(), |row| {
                                let (ch, ts) = thread_target.clone().unwrap();
                                let replies_text = if huddle.reply_count == 1 {
                                    "1 reply".to_owned()
                                } else {
                                    format!("{} replies", huddle.reply_count)
                                };
                                row.child("·").child(
                                    div()
                                        .id(("huddle-row-replies", ix))
                                        .text_color(link)
                                        .font_weight(FontWeight::MEDIUM)
                                        .hover(|el| el.underline())
                                        .child(replies_text)
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.stop_propagation();
                                            cx.emit(HuddlesEvent::OpenThread(
                                                ch.clone(),
                                                ts.clone(),
                                            ));
                                        })),
                                )
                            }),
                    ),
            )
            .when(!visible_avatars.is_empty(), |row| {
                row.child(
                    h_flex()
                        .flex_none()
                        .items_center()
                        .children(visible_avatars.into_iter().enumerate().map(|(aix, uid)| {
                            div()
                                .when(aix > 0, |el| el.ml(px(-4.)))
                                .rounded_md()
                                .overflow_hidden()
                                .border_1()
                                .border_color(bg)
                                .child(user_chip(&self.fixtures, uid, 24.))
                        }))
                        .when(overflow > 0, |el| {
                            el.child(
                                div()
                                    .ml(px(-4.))
                                    .size(px(24.))
                                    .rounded_md()
                                    .bg(theme.secondary)
                                    .border_1()
                                    .border_color(bg)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg)
                                    .child(format!("+{overflow}")),
                            )
                        }),
                )
            })
            .when_some(thread_target, |row, (ch, ts)| {
                row.child(
                    Button::new(("huddle-thread", ix))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::EllipsisVertical)
                        .tooltip("Open huddle thread")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.stop_propagation();
                            cx.emit(HuddlesEvent::OpenThread(ch.clone(), ts.clone()));
                        })),
                )
            })
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(HuddlesEvent::Open(open.clone()))))
    }
}

impl Render for HuddlesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, border, muted, fg, secondary) = (
            theme.background,
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.secondary,
        );
        let live_huddles: Vec<&RecentHuddle> = self.huddles.iter().filter(|h| h.live).collect();
        let recent_huddles: Vec<&RecentHuddle> = self
            .huddles
            .iter()
            .filter(|h| !h.live)
            .filter(|h| match self.filter {
                HuddleFilter::All => true,
                HuddleFilter::Channels => !self.is_dm(&h.channel),
                HuddleFilter::Dms => self.is_dm(&h.channel),
            })
            .collect();
        let suggested = self.suggested_peer();

        let recent_count = recent_huddles.len();
        let recent_rows: Vec<_> = recent_huddles
            .into_iter()
            .enumerate()
            .map(|(ix, huddle)| self.recent_row(ix, ix + 1 == recent_count, huddle, cx))
            .collect();
        let live_cards: Vec<_> = live_huddles
            .into_iter()
            .enumerate()
            .map(|(ix, huddle)| self.render_live_card(ix, huddle, cx))
            .collect();
        let quick_start =
            suggested.map(|(ch, peer, count)| self.render_quick_start_card(ch, peer, count, cx));
        let has_hero = !live_cards.is_empty() || quick_start.is_some();

        let filter_pill =
            |id: &'static str, label: &'static str, kind: HuddleFilter, cx: &mut Context<Self>| {
                let active = self.filter == kind;
                div()
                    .id(id)
                    .px_2p5()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(if active { fg.opacity(0.45) } else { border })
                    .bg(if active { secondary } else { bg })
                    .cursor_pointer()
                    .text_xs()
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(fg)
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = kind;
                        cx.notify();
                    }))
            };
        let filter_all = filter_pill("huddle-filter-all", "All huddles", HuddleFilter::All, cx);
        let filter_channels = filter_pill(
            "huddle-filter-channels",
            "Channels",
            HuddleFilter::Channels,
            cx,
        );
        let filter_dms = filter_pill(
            "huddle-filter-dms",
            "Direct messages",
            HuddleFilter::Dms,
            cx,
        );

        v_flex()
            .size_full()
            .bg(bg)
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px_6()
                    .h(px(52.))
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(fg)
                            .child("Huddles"),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("huddles-new")
                                    .outline()
                                    .with_size(Size::Small)
                                    .icon(IconName::Plus)
                                    .label("New Huddle")
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(HuddlesEvent::NewHuddle)),
                                    ),
                            )
                            .child(
                                Button::new("huddles-close")
                                    .ghost()
                                    .with_size(Size::Small)
                                    .icon(IconName::X)
                                    .tooltip("Close")
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(HuddlesEvent::Close)),
                                    ),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .id("huddles-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .items_center()
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(px(860.))
                            .px_6()
                            .py_5()
                            .gap_5()
                            .when(has_hero, |col| {
                                col.child(
                                    h_flex()
                                        .flex_wrap()
                                        .gap_4()
                                        .children(live_cards)
                                        .children(quick_start),
                                )
                            })
                            .when(!self.huddles.is_empty(), |col| {
                                col.child(
                                    v_flex()
                                        .gap_3()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(fg)
                                                .child("Recent huddles"),
                                        )
                                        .child(
                                            h_flex()
                                                .items_center()
                                                .gap_2()
                                                .child(filter_all)
                                                .child(filter_channels)
                                                .child(filter_dms),
                                        )
                                        .child(if recent_rows.is_empty() {
                                            div()
                                                .p_6()
                                                .rounded_xl()
                                                .border_1()
                                                .border_color(border)
                                                .text_xs()
                                                .text_color(muted)
                                                .child("No matching recent huddles.")
                                                .into_any_element()
                                        } else {
                                            v_flex()
                                                .rounded_xl()
                                                .border_1()
                                                .border_color(border)
                                                .bg(bg)
                                                .overflow_hidden()
                                                .children(recent_rows)
                                                .into_any_element()
                                        }),
                                )
                            })
                            .when(self.huddles.is_empty(), |col| {
                                let loading = self.loading;
                                col.child(
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
                                        .child(if loading {
                                            "Loading huddles…"
                                        } else {
                                            "No recent huddles."
                                        })
                                        .when(!loading, |col| {
                                            col.child(div().text_xs().child(
                                                "Start one from a conversation's headphones button.",
                                            ))
                                        }),
                                )
                            }),
                    ),
            )
    }
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
            let has_active_participants = room
                .and_then(|room| room.get("participants"))
                .and_then(Value::as_array)
                .is_some_and(|ids| !ids.is_empty());
            let live = room
                .and_then(|room| room.get("has_ended"))
                .and_then(Value::as_bool)
                .is_some_and(|ended| !ended)
                && has_active_participants;
            let participants = room
                .and_then(|room| {
                    room.get(if live {
                        "participants"
                    } else {
                        "participant_history"
                    })
                    .or_else(|| room.get("participants"))
                })
                .and_then(Value::as_array)
                .map_or_else(Vec::new, |ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(UserId::from)
                        .collect()
                });
            let duration_secs = room.and_then(|room| {
                let start = room.get("date_start")?.as_u64()?;
                let end = room.get("date_end")?.as_u64()?;
                (end > start).then_some(end - start)
            });
            huddles.push(RecentHuddle {
                channel: conversation.id.clone(),
                ts: message.ts.clone(),
                participants,
                live,
                duration_secs,
                reply_count: message.reply_count.unwrap_or(0),
            });
        }
    }
    huddles.sort_by_key(|huddle| std::cmp::Reverse(huddle.ts.clone()));
    huddles
}
