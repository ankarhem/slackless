//! The Threads view's data and card rendering: the [`ThreadCard`] model, its derivations
//! (Slack's own `subscriptions.thread.getView` list, the bundled sample workspace, the local
//! store) and one card's element. The view entity lives in [`crate::threads_view`].
//!
//! A thread is followed when the signed-in user is among its repliers or authored its root -
//! the store-side rule Slack's own list answers server-side.

use std::sync::Arc;

use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::api::ThreadsView as SlackThreads;
use slackless_core::store::Store;
use slackless_core::types::{ChannelId, Message, TeamId, Ts, UserId};

use crate::composer::Composer;
use crate::fixtures::Fixtures;
use crate::messages::{list_stamp, message_row};
use crate::threads_view::{ThreadsEvent, ThreadsView};

/// How many latest replies a card shows, like Slack's Threads view.
const SHOWN_REPLIES: usize = 2;

/// Root-history rows scanned per conversation when deriving local cards.
const STORE_SCAN_LIMIT: usize = 500;

/// One followed thread.
#[derive(Clone)]
pub struct ThreadCard {
    pub channel: ChannelId,
    pub thread_ts: Ts,
    pub root: Message,
    /// The latest replies, oldest first (the full count rides the divider row).
    pub replies: Vec<Message>,
    pub reply_total: u32,
    pub unread: u32,
}

impl ThreadCard {
    /// Slack's card order: unread threads first, then latest activity.
    fn sort_key(&self) -> (bool, Ts) {
        (
            self.unread > 0,
            self.root
                .latest_reply
                .clone()
                .unwrap_or_else(|| self.thread_ts.clone()),
        )
    }
}

/// The latest `SHOWN_REPLIES` of `replies`, oldest first.
fn latest(replies: &[Message]) -> Vec<Message> {
    let at = replies.len().saturating_sub(SHOWN_REPLIES);
    replies[at..].to_vec()
}

/// A card from a root and its full reply list.
fn card(channel: ChannelId, root: Message, replies: &[Message]) -> ThreadCard {
    ThreadCard {
        thread_ts: root.ts.clone(),
        reply_total: root
            .reply_count
            .unwrap_or_else(|| u32::try_from(replies.len()).unwrap_or(u32::MAX)),
        replies: latest(replies),
        unread: 0,
        channel,
        root,
    }
}

/// Order a card list like Slack's Threads view: unread first, then latest activity.
fn sort_cards(cards: &mut [ThreadCard]) {
    cards.sort_by_key(|card| std::cmp::Reverse(card.sort_key()));
}

/// The bundled sample workspace's followed threads (fixture mode).
pub fn local_cards(fixtures: &Fixtures) -> Vec<ThreadCard> {
    let me = &fixtures.self_user;
    let mut cards = Vec::new();
    for ((channel, thread_ts), replies) in &fixtures.threads {
        let Some(root) = fixtures
            .messages
            .get(channel)
            .and_then(|history| history.iter().find(|message| message.ts == *thread_ts))
        else {
            continue;
        };
        let followed = root.reply_users.contains(me) || root.user.as_ref() == Some(me);
        if followed {
            cards.push(card(channel.clone(), root.clone(), replies));
        }
    }
    sort_cards(&mut cards);
    cards
}

/// The stored followed threads (live mode, until or instead of Slack's own list).
pub fn store_cards(store: &Store, team: &TeamId, me: &UserId) -> Vec<ThreadCard> {
    let Ok(conversations) = store.conversations(team) else {
        return Vec::new();
    };
    let mut cards = Vec::new();
    for conversation in &conversations {
        let Ok(rows) = store.channel_history(team, &conversation.id, None, STORE_SCAN_LIMIT) else {
            continue;
        };
        for row in rows {
            let root = &row.message;
            let followed =
                root.reply_users.iter().any(|user| user == me) || root.user.as_ref() == Some(me);
            if row.deleted || root.reply_count.unwrap_or(0) == 0 || !followed {
                continue;
            }
            if let Ok(thread) = store.thread_replies(team, &conversation.id, &root.ts) {
                let replies: Vec<Message> = thread
                    .into_iter()
                    .filter(|reply| !reply.deleted && reply.message.ts != root.ts)
                    .map(|reply| reply.message)
                    .collect();
                if !replies.is_empty() {
                    cards.push(card(conversation.id.clone(), root.clone(), &replies));
                }
            }
        }
    }
    sort_cards(&mut cards);
    cards
}

/// Slack's own followed threads (`subscriptions.thread.getView`): the view's threads become
/// cards, unread first, then latest activity.
pub fn remote_cards(view: SlackThreads) -> Vec<ThreadCard> {
    let mut cards: Vec<ThreadCard> = view
        .threads
        .into_iter()
        .filter_map(|thread| {
            let root = thread.root;
            let channel = root.channel.clone()?;
            if root.ts.as_str().is_empty() {
                return None;
            }
            Some(ThreadCard {
                thread_ts: root.thread_ts.clone().unwrap_or_else(|| root.ts.clone()),
                replies: latest(&thread.latest_replies),
                reply_total: root.reply_count.unwrap_or_default(),
                unread: u32::try_from(thread.unread_replies.len()).unwrap_or(u32::MAX),
                channel,
                root,
            })
        })
        .collect();
    sort_cards(&mut cards);
    cards
}

/// One card: the clickable channel header, the root message, the reply divider and the
/// latest replies, and the reply affordance (the docked composer while `active`).
pub(super) fn card_view(
    fixtures: &Arc<Fixtures>,
    ix: usize,
    card: &ThreadCard,
    active: bool,
    composer: &Entity<Composer>,
    cx: &mut Context<ThreadsView>,
) -> AnyElement {
    let theme = cx.theme();
    let (border, muted, hover, danger, danger_foreground) = (
        theme.border,
        theme.muted_foreground,
        theme.accent,
        theme.danger,
        theme.danger_foreground,
    );
    let label = fixtures
        .conversation(&card.channel)
        .map(|c| fixtures.conversation_label(c))
        .unwrap_or_else(|| card.channel.to_string());
    let open = (card.channel.clone(), card.thread_ts.clone());
    let shown: Vec<Message> = latest(&card.replies);

    let body = v_flex()
        .min_w_0()
        .gap_1()
        .child(message_row(fixtures, &card.root, None, cx))
        .when(card.reply_total > 0, |body| {
            body.child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("{} replies", card.reply_total)),
                    )
                    .child(div().flex_1().h(px(1.)).bg(border)),
            )
        })
        .children(
            shown
                .iter()
                .map(|reply| message_row(fixtures, reply, None, cx)),
        )
        .child(if active {
            composer.clone().into_any_element()
        } else {
            h_flex()
                .id(("thread-reply-bar", ix))
                .mx_1()
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_color(muted)
                .hover(|bar| bar.bg(hover))
                .child("Reply...")
                .on_click(cx.listener(move |this, _, window: &mut Window, cx| {
                    this.activate(Some(ix), window, cx);
                }))
                .into_any_element()
        });

    v_flex()
        .id(("thread-card", ix))
        .px_4()
        .py_3()
        .gap_1()
        .border_b_1()
        .border_color(border)
        .child(
            h_flex()
                .id(("thread-card-head", ix))
                .items_center()
                .gap_2()
                .cursor_pointer()
                .hover(|head| head.bg(hover))
                .rounded_md()
                .px_1()
                .child(
                    Icon::new(gpui_kit::assets::IconName::MessageSquareText)
                        .text_color(muted)
                        .size(px(14.)),
                )
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(label.trim_start_matches("# ").to_owned()),
                )
                .when(card.unread > 0, |head| {
                    head.child(
                        div()
                            .rounded_full()
                            .bg(danger)
                            .text_xs()
                            .px_1p5()
                            .text_color(danger_foreground)
                            .child(card.unread.to_string()),
                    )
                })
                .child(div().flex_1())
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(list_stamp(&card.thread_ts)),
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(ThreadsEvent::Open {
                        channel: open.0.clone(),
                        thread_ts: open.1.clone(),
                    });
                })),
        )
        .child(body)
        .into_any_element()
}
