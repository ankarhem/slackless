//! The Threads view's data and card rendering: the [`ThreadCard`] model, its derivations
//! (Slack's own `subscriptions.thread.getView` list, the bundled sample workspace, the local
//! store) and one card's element, matching Slack's `p-threads_view` layout (channel + participants
//! header above a rounded bordered card containing the root, a clickable "Show N more replies"
//! link, the latest replies, and a bordered reply composer with "Also send to #channel").
//! The view entity lives in [`crate::threads_view`].
//!
//! A thread is followed when the signed-in user is among its repliers or authored its root -
//! the store-side rule Slack's own list answers server-side.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px,
};
use slackless_core::api::ThreadsView as SlackThreads;
use slackless_core::store::Store;
use slackless_core::types::{ChannelId, ConversationKind, Message, TeamId, Ts, UserId};

use crate::actions::{self, ActionHandler, ActionSet, MenuOrigin};
use crate::avatars::conversation_avatar;
use crate::composer::Composer;
use crate::fixtures::Fixtures;
use crate::messages::{LinkHandler, message_row_linked, same_group};
use crate::threads_view::{ThreadsEvent, ThreadsView};

/// How many latest replies a collapsed card shows, matching Slack's Threads view.
const SHOWN_REPLIES: usize = 2;

/// Root-history rows scanned per conversation when deriving local cards.
const STORE_SCAN_LIMIT: usize = 500;

/// One followed thread.
#[derive(Clone)]
pub struct ThreadCard {
    pub channel: ChannelId,
    pub thread_ts: Ts,
    pub root: Message,
    /// Known replies, oldest first (collapsed cards slice to the latest [`SHOWN_REPLIES`]).
    pub replies: Vec<Message>,
    pub reply_total: u32,
    pub unread: u32,
    /// Pre-mark read cursor used to draw a subtle `"New"` divider above unread replies.
    pub last_read: Option<Ts>,
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

    /// The newest message timestamp in this thread (the cursor sent to `subscriptions.thread.mark`
    /// when marking the thread fully read).
    pub fn newest_ts(&self) -> Ts {
        let latest_reply = self.replies.last().map(|m| m.ts.clone());
        let root_latest = self.root.latest_reply.clone();
        match (latest_reply, root_latest) {
            (Some(a), Some(b)) => a.max(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => self.root.ts.clone(),
        }
    }

    /// The cursor immediately before `ts` in this thread (so `subscriptions.thread.mark` moves
    /// the read marker back and makes `ts` and everything after it unread).
    pub fn cursor_before(&self, ts: &Ts) -> Ts {
        let mut prev: Option<&Ts> = None;
        for msg in std::iter::once(&self.root).chain(&self.replies) {
            if msg.ts < *ts {
                prev = Some(&msg.ts);
            } else {
                break;
            }
        }
        prev.cloned().unwrap_or_else(|| ts_just_before(ts))
    }
}

/// One microsecond before `ts`, matching Slack's `DDDDDDDDDD.MMMMMM` timestamp format.
pub(crate) fn ts_just_before(ts: &Ts) -> Ts {
    let raw = ts.as_str();
    if let Some((secs, frac)) = raw.split_once('.') {
        if let (Ok(s), Ok(f)) = (secs.parse::<u64>(), frac.parse::<u64>()) {
            return if f > 0 {
                Ts::from(format!("{s}.{:06}", f - 1))
            } else if s > 0 {
                Ts::from(format!("{}.999999", s - 1))
            } else {
                Ts::from("0.000000")
            };
        }
    }
    Ts::from("0.000000")
}

/// A card from a root and its full reply list.
fn card(channel: ChannelId, root: Message, replies: &[Message]) -> ThreadCard {
    let last_read = root.last_read.clone();
    ThreadCard {
        thread_ts: root.ts.clone(),
        reply_total: root
            .reply_count
            .unwrap_or_else(|| u32::try_from(replies.len()).unwrap_or(u32::MAX)),
        replies: replies.to_vec(),
        unread: 0,
        last_read,
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
/// cards, unread first, then latest activity. Merges `latest_replies` and `unread_replies`
/// (deduped by `ts`, sorted oldest first) so unread replies are never hidden.
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
            let unread_count = u32::try_from(thread.unread_replies.len()).unwrap_or(u32::MAX);
            let first_unread_ts = thread.unread_replies.iter().map(|m| &m.ts).min().cloned();
            let mut seen_ts: HashSet<Ts> = HashSet::new();
            let mut replies: Vec<Message> =
                Vec::with_capacity(thread.latest_replies.len() + thread.unread_replies.len());
            for msg in thread
                .latest_replies
                .into_iter()
                .chain(thread.unread_replies)
            {
                if seen_ts.insert(msg.ts.clone()) {
                    replies.push(msg);
                }
            }
            replies.sort_by(|a, b| a.ts.cmp(&b.ts));
            let last_read = root
                .last_read
                .clone()
                .or_else(|| first_unread_ts.as_ref().map(ts_just_before));
            let reply_total = root
                .reply_count
                .unwrap_or_default()
                .max(u32::try_from(replies.len()).unwrap_or(u32::MAX));
            Some(ThreadCard {
                thread_ts: root.thread_ts.clone().unwrap_or_else(|| root.ts.clone()),
                replies,
                reply_total,
                unread: unread_count,
                last_read,
                channel,
                root,
            })
        })
        .collect();
    sort_cards(&mut cards);
    cards
}

/// Slack's thread participant subtitle below the channel name, e.g.
/// `"Linus Brännström, Jira, and David Michaelsen"` or `"Veljko Rajic and you"`.
pub(crate) fn participant_summary(fixtures: &Fixtures, card: &ThreadCard) -> String {
    let me = &fixtures.self_user;
    let mut seen: HashSet<String> = HashSet::new();
    let mut names: Vec<String> = Vec::new();
    let mut saw_me = false;

    for msg in std::iter::once(&card.root).chain(&card.replies) {
        if let Some(u) = &msg.user {
            if u == me {
                saw_me = true;
            } else {
                let name = fixtures.user_name(u).to_owned();
                if !name.is_empty() && seen.insert(name.clone()) {
                    names.push(name);
                }
            }
        } else if let Some(bot) = msg.username.as_deref().filter(|s| !s.is_empty()) {
            let name = bot.to_owned();
            if seen.insert(name.clone()) {
                names.push(name);
            }
        }
    }
    for u in &card.root.reply_users {
        if u == me {
            saw_me = true;
        } else {
            let name = fixtures.user_name(u).to_owned();
            if !name.is_empty() && seen.insert(name.clone()) {
                names.push(name);
            }
        }
    }
    if saw_me {
        names.push("you".to_owned());
    }
    format_name_list(&names)
}

fn format_name_list(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        2 => format!("{} and {}", names[0], names[1]),
        3 => format!("{}, {}, and {}", names[0], names[1], names[2]),
        n => {
            let others = n - 3;
            format!(
                "{}, {}, {}, and {} {}",
                names[0],
                names[1],
                names[2],
                others,
                if others == 1 { "other" } else { "others" }
            )
        }
    }
}

/// Render one interactive message row (root or reply) inside a thread card, with hover highlight,
/// saved banner/tint, interactive reaction chips, floating hover toolbar, context menu, and
/// inline edit mode.
#[allow(clippy::too_many_arguments)]
fn render_thread_message(
    fixtures: &Arc<Fixtures>,
    card: &ThreadCard,
    msg: &Message,
    key: String,
    msg_group: SharedString,
    grouped: Option<SharedString>,
    is_first_in_card: bool,
    saved_set: &HashSet<Ts>,
    editing: Option<&(ChannelId, Ts, Entity<TextareaState>)>,
    links: &LinkHandler,
    cx: &mut Context<ThreadsView>,
) -> AnyElement {
    let theme = cx.theme();
    let (bg, ring, hover_bg, saved_bg) = (
        theme.background,
        theme.ring,
        theme.secondary.opacity(0.5),
        theme.warning.opacity(0.08),
    );
    if let Some((edit_ch, edit_ts, edit_input)) = editing {
        if *edit_ch == card.channel && *edit_ts == msg.ts {
            return v_flex()
                .id(SharedString::from(format!("{key}-edit")))
                .mx_4()
                .my_1()
                .p_2p5()
                .gap_2()
                .rounded_md()
                .border_1()
                .border_color(ring)
                .bg(bg)
                .child(Textarea::new(edit_input).appearance(false).bordered(false))
                .child(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new(SharedString::from(format!("{key}-edit-cancel")))
                                .outline()
                                .with_size(Size::XSmall)
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_edit(cx))),
                        )
                        .child(
                            Button::new(SharedString::from(format!("{key}-edit-save")))
                                .primary()
                                .with_size(Size::XSmall)
                                .label("Save Changes")
                                .on_click(cx.listener(|this, _, _, cx| this.save_edit(cx))),
                        ),
                )
                .into_any_element();
        }
    }

    let is_saved = saved_set.contains(&msg.ts);
    let view = cx.entity();
    let ch_for_action = card.channel.clone();
    let thread_ts_for_action = card.thread_ts.clone();
    let msg_for_action = msg.clone();
    let handler: ActionHandler = Rc::new(move |action, _window, cx| {
        let ch = ch_for_action.clone();
        let thread_ts = thread_ts_for_action.clone();
        let message = Box::new(msg_for_action.clone());
        view.update(cx, |_, cx| {
            cx.emit(ThreadsEvent::MessageAction {
                channel: ch,
                thread_ts,
                message,
                action,
            });
        });
    });
    let set = ActionSet {
        own: msg.user.as_ref() == Some(&fixtures.self_user),
        reply: true,
        saved: is_saved,
        pinned: msg.pinned_to.contains(&card.channel),
        in_thread: msg.ts != card.thread_ts,
    };
    let mut display = msg.clone();
    let reactions = std::mem::take(&mut display.reactions);
    let chips = actions::reaction_chips(&key, &reactions, fixtures, Some(&handler), cx)
        .map(|c| c.pl(px(48.)));
    let row_body = message_row_linked(fixtures, &display, Some(&handler), Some(links), grouped, cx);
    let quick = fixtures.quick_reactions.clone();
    let menu_handler = handler.clone();
    let row_key = key.clone();

    actions::HoverOverlay::new(SharedString::from(key), move |hover, _, cx| {
        let revealed = hover.revealed(cx);
        v_flex()
            .id(SharedString::from(row_key.clone()))
            .group(msg_group)
            .relative()
            .px_4()
            .py_0p5()
            .when(is_saved, |row| {
                row.bg(saved_bg).child(actions::saved_banner(cx))
            })
            .when(!is_saved && revealed, |row| row.bg(hover_bg))
            .child(row_body)
            .children(chips)
            .when(revealed, |row| {
                row.child(
                    actions::hover_toolbar(&row_key, &quick, set, &handler, &hover, cx)
                        .when(is_first_in_card, |bar| bar.top(px(3.))),
                )
            })
            .on_hover(hover.host_on_hover())
            .context_menu(move |menu, window, cx| {
                actions::actions_menu(menu, set, &menu_handler, MenuOrigin::Context, window, cx)
            })
            .into_any_element()
    })
    .into_any_element()
}

/// One followed thread group matching official Slack's Threads view:
/// 1. Channel header + participants subtitle above the card (click opens thread in channel)
/// 2. Rounded bordered card containing:
///    - Root message (with full hover actions, reactions, context menu, and inline edit)
///    - Clickable blue `"Show N more replies"` link when replies are hidden
///    - Latest replies (or all replies when expanded), with consecutive same-author grouping
///      and full hover actions, reactions, context menu, and inline edit
///    - Bordered reply composer (`Composer` with `"Also send to #channel"` when active, or a
///      matching bordered `"Reply..."` box that activates & focuses it on click).
#[allow(clippy::too_many_arguments)]
pub(super) fn card_view(
    fixtures: &Arc<Fixtures>,
    ix: usize,
    card: &ThreadCard,
    active: bool,
    expanded: bool,
    saved_set: &HashSet<Ts>,
    editing: Option<&(ChannelId, Ts, Entity<TextareaState>)>,
    composer: &Entity<Composer>,
    cx: &mut Context<ThreadsView>,
) -> AnyElement {
    let theme = cx.theme();
    let (border, muted, background, link, danger, danger_foreground) = (
        theme.border,
        theme.muted_foreground,
        theme.background,
        theme.link,
        theme.danger,
        theme.danger_foreground,
    );
    let conversation = fixtures.conversation(&card.channel);
    let kind = conversation.map(|c| c.kind());
    let raw_label = conversation
        .map(|c| fixtures.conversation_label(c))
        .unwrap_or_else(|| card.channel.to_string());
    let channel_name = match kind {
        Some(ConversationKind::PublicChannel) | Some(ConversationKind::PrivateChannel) => {
            raw_label.trim_start_matches("# ").to_owned()
        }
        _ => raw_label,
    };
    let badge = match kind {
        Some(ConversationKind::Im) | Some(ConversationKind::Mpim) => conversation
            .map(|c| conversation_avatar(fixtures, c, 18., background, cx).into_any_element())
            .unwrap_or_else(|| div().into_any_element()),
        Some(ConversationKind::PrivateChannel) => Icon::new(IconName::Lock)
            .text_color(theme.foreground)
            .size(px(14.))
            .into_any_element(),
        _ => Icon::new(IconName::Hash)
            .text_color(theme.foreground)
            .size(px(14.))
            .into_any_element(),
    };
    let participants = participant_summary(fixtures, card);
    let open = (card.channel.clone(), card.thread_ts.clone());

    let shown: &[Message] = if expanded || card.replies.len() <= SHOWN_REPLIES {
        &card.replies
    } else {
        let start = card.replies.len() - SHOWN_REPLIES;
        &card.replies[start..]
    };
    let hidden = if expanded {
        0
    } else {
        card.reply_total.saturating_sub(shown.len() as u32)
    };

    let view = cx.entity();
    let links: LinkHandler = Rc::new(move |target, _window, cx| {
        view.update(cx, |_, cx| cx.emit(ThreadsEvent::OpenLink(target.clone())));
    });

    let root_row = render_thread_message(
        fixtures,
        card,
        &card.root,
        format!("thread-{ix}-root-{}", card.root.ts.as_str()),
        SharedString::from(format!("thread-{ix}-root")),
        None,
        true,
        saved_set,
        editing,
        &links,
        cx,
    );

    let expand_key = (card.channel.clone(), card.thread_ts.clone());
    let mut prev_reply: Option<&Message> = None;
    let mut drew_new_divider = false;
    let mut reply_rows: Vec<AnyElement> = Vec::with_capacity(shown.len() + 1);
    for (r_ix, reply) in shown.iter().enumerate() {
        let is_unread = card.last_read.as_ref().is_some_and(|read| reply.ts > *read);
        if is_unread && !drew_new_divider {
            drew_new_divider = true;
            prev_reply = None;
            reply_rows.push(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_0p5()
                    .child(div().flex_1().h(px(1.)).bg(danger.opacity(0.6)))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(danger)
                            .child("New"),
                    )
                    .into_any_element(),
            );
        }
        let msg_group = SharedString::from(format!("thread-{ix}-reply-{r_ix}"));
        let grouped = prev_reply
            .filter(|prev| same_group(prev, reply))
            .map(|_| msg_group.clone());
        prev_reply = Some(reply);
        reply_rows.push(render_thread_message(
            fixtures,
            card,
            reply,
            format!("thread-{ix}-reply-{}-{}", r_ix, reply.ts.as_str()),
            msg_group,
            grouped,
            false,
            saved_set,
            editing,
            &links,
            cx,
        ));
    }

    let mark_key = (card.channel.clone(), card.thread_ts.clone());
    let header = h_flex()
        .items_center()
        .justify_between()
        .gap_2()
        .mb_2()
        .child(
            v_flex()
                .id(("thread-card-head", ix))
                .min_w_0()
                .flex_1()
                .gap_0p5()
                .cursor_pointer()
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(ThreadsEvent::Open {
                        channel: open.0.clone(),
                        thread_ts: open.1.clone(),
                    });
                }))
                .child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(badge)
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_weight(FontWeight::BOLD)
                                .hover(|el| el.underline())
                                .child(channel_name),
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
                        }),
                )
                .when(!participants.is_empty(), |head| {
                    head.child(div().text_xs().text_color(muted).child(participants))
                }),
        )
        .when(card.unread > 0, |row| {
            row.child(
                Button::new(("thread-mark-read", ix))
                    .outline()
                    .with_size(Size::XSmall)
                    .label("Mark as read")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.mark_card_read(mark_key.0.clone(), mark_key.1.clone(), cx);
                    })),
            )
        });

    let reply_footer = if active {
        div().mt_2().child(composer.clone()).into_any_element()
    } else {
        h_flex()
            .id(("thread-reply-bar", ix))
            .mx_4()
            .mt_2()
            .px_3()
            .py_2()
            .h(px(40.))
            .items_center()
            .rounded(px(8.))
            .border_1()
            .border_color(border)
            .cursor_text()
            .text_sm()
            .text_color(muted)
            .hover(|bar| bar.border_color(muted))
            .child("Reply...")
            .on_click(cx.listener(move |this, _, window: &mut Window, cx| {
                this.activate(Some(ix), window, cx);
            }))
            .into_any_element()
    };

    let card_box = v_flex()
        .min_w_0()
        .rounded(px(12.))
        .border_1()
        .border_color(border)
        .bg(background)
        .pt_3()
        .when(!active, |box_el| box_el.pb_3())
        .child(root_row)
        .when(hidden > 0, |box_el| {
            let label = if hidden == 1 {
                "Show 1 more reply".to_owned()
            } else {
                format!("Show {hidden} more replies")
            };
            box_el.child(
                div()
                    .id(("thread-expand", ix))
                    .px_4()
                    .pt_2()
                    .pb_1()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(link)
                    .cursor_pointer()
                    .hover(|el| el.underline())
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.expand(expand_key.0.clone(), expand_key.1.clone(), cx);
                    })),
            )
        })
        .when(!reply_rows.is_empty(), |box_el| {
            box_el.child(v_flex().pt_1().gap_1().children(reply_rows))
        })
        .child(reply_footer);

    v_flex()
        .id(("thread-card", ix))
        .w_full()
        .flex_none()
        .child(header)
        .child(card_box)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    #[test]
    fn participant_summary_formats_names_with_self_last() {
        let fixtures = fixtures::load();
        let cards = local_cards(&fixtures);
        assert!(!cards.is_empty());
        let summary = participant_summary(&fixtures, &cards[0]);
        assert!(
            summary.ends_with("you"),
            "self participant should appear last as 'you', got: {summary}"
        );
    }

    #[test]
    fn format_name_list_handles_various_counts() {
        assert_eq!(format_name_list(&[]), "");
        assert_eq!(format_name_list(&["Alice".into()]), "Alice");
        assert_eq!(
            format_name_list(&["Alice".into(), "you".into()]),
            "Alice and you"
        );
        assert_eq!(
            format_name_list(&["Alice".into(), "Bob".into(), "Cara".into()]),
            "Alice, Bob, and Cara"
        );
        assert_eq!(
            format_name_list(&["A".into(), "B".into(), "C".into(), "D".into(), "E".into()]),
            "A, B, C, and 2 others"
        );
    }

    #[test]
    fn remote_cards_merges_unread_and_latest_replies_sorted() {
        use slackless_core::api::ThreadSubscription;
        let view = SlackThreads {
            total_unread_replies: 2,
            has_more: false,
            threads: vec![ThreadSubscription {
                root: Message {
                    channel: Some(ChannelId::from("C1")),
                    ts: Ts::from("1700000000.000100"),
                    text: "root".into(),
                    reply_count: Some(3),
                    last_read: Some(Ts::from("1700000000.000200")),
                    ..Default::default()
                },
                latest_replies: vec![
                    Message {
                        ts: Ts::from("1700000000.000200"),
                        text: "read".into(),
                        ..Default::default()
                    },
                    Message {
                        ts: Ts::from("1700000000.000400"),
                        text: "newest".into(),
                        ..Default::default()
                    },
                ],
                unread_replies: vec![
                    Message {
                        ts: Ts::from("1700000000.000300"),
                        text: "unread-1".into(),
                        ..Default::default()
                    },
                    Message {
                        ts: Ts::from("1700000000.000400"),
                        text: "newest".into(),
                        ..Default::default()
                    },
                ],
            }],
        };
        let cards = remote_cards(view);
        assert_eq!(cards.len(), 1);
        let card = &cards[0];
        let ts_list: Vec<&str> = card.replies.iter().map(|m| m.ts.as_str()).collect();
        assert_eq!(
            ts_list,
            vec![
                "1700000000.000200",
                "1700000000.000300",
                "1700000000.000400"
            ]
        );
        assert_eq!(card.unread, 2);
        assert_eq!(card.newest_ts(), Ts::from("1700000000.000400"));
        assert_eq!(
            card.cursor_before(&Ts::from("1700000000.000300")),
            Ts::from("1700000000.000200")
        );
    }
}
