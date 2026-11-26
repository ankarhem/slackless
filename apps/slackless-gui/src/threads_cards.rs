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
use slackless_core::api::{ThreadsView as SlackThreads, list_comment_channel_id};
use slackless_core::store::Store;
use slackless_core::types::{
    ChannelId, ConversationKind, Message, MessageSubtype, TeamId, Ts, UserId,
};

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

/// Metadata identifying a Slack List item comment thread (`F-COLLAB-250`, `F-COLLAB-251`,
/// `lists.getThreadsForRecord`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItemThreadMeta {
    pub list_id: String,
    pub list_title: String,
    pub record_id: String,
    pub item_title: String,
}

/// One followed thread (channel/DM message thread or Slack List item comment thread).
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
    /// Present when this thread belongs to a Slack List item (`lists.getThreadsForRecord`).
    pub list_item: Option<ListItemThreadMeta>,
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
    if let Some((secs, frac)) = raw.split_once('.')
        && let (Ok(s), Ok(f)) = (secs.parse::<u64>(), frac.parse::<u64>())
    {
        return if f > 0 {
            Ts::from(format!("{s}.{:06}", f - 1))
        } else if s > 0 {
            Ts::from(format!("{}.999999", s - 1))
        } else {
            Ts::from("0.000000")
        };
    }
    Ts::from("0.000000")
}

/// Detect whether a thread root message represents a Slack List item comment thread
/// (`list_record_comment` subtype or `list_record` / `list_id`+`record_id` metadata in `extra`).
pub fn detect_list_item_meta(root: &Message) -> Option<ListItemThreadMeta> {
    let is_list_subtype = matches!(
        root.subtype.as_ref(),
        Some(MessageSubtype::Other(s))
            if s.contains("list_record") || s.contains("list_item") || s == "list_comment"
    );
    let record_obj = root
        .extra
        .get("list_record")
        .or_else(|| root.extra.get("list_item"))
        .and_then(|v| v.as_object());
    let list_id = record_obj
        .and_then(|o| o.get("list_id").and_then(|v| v.as_str()))
        .or_else(|| root.extra.get("list_id").and_then(|v| v.as_str()))
        .map(str::to_owned);
    let record_id = record_obj
        .and_then(|o| {
            o.get("record_id")
                .or_else(|| o.get("item_id"))
                .or_else(|| o.get("id"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| root.extra.get("record_id").and_then(|v| v.as_str()))
        .map(str::to_owned);
    if !is_list_subtype && (list_id.is_none() || record_id.is_none()) {
        return None;
    }
    let list_id = list_id.unwrap_or_else(|| {
        root.channel
            .as_ref()
            .map(|c| c.as_str().to_owned())
            .unwrap_or_default()
    });
    let record_id = record_id.unwrap_or_else(|| root.ts.as_str().to_owned());
    let list_title = record_obj
        .and_then(|o| o.get("list_title").and_then(|v| v.as_str()))
        .or_else(|| root.extra.get("list_title").and_then(|v| v.as_str()))
        .filter(|s| !s.is_empty())
        .unwrap_or("List")
        .to_owned();
    let item_title = record_obj
        .and_then(|o| {
            o.get("title")
                .or_else(|| o.get("item_title"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| root.extra.get("item_title").and_then(|v| v.as_str()))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Item {record_id}"));
    Some(ListItemThreadMeta {
        list_id,
        list_title,
        record_id,
        item_title,
    })
}

/// Build a [`ThreadCard`] from list record comment messages returned by `lists.getThreadsForRecord`
/// (`F-COLLAB-250`, `F-COLLAB-251`).
pub fn list_item_thread_card(
    meta: ListItemThreadMeta,
    mut messages: Vec<Message>,
) -> Option<ThreadCard> {
    if messages.is_empty() {
        return None;
    }
    messages.sort_by(|a, b| a.ts.cmp(&b.ts));
    let mut root = messages.remove(0);
    let channel = root
        .channel
        .clone()
        .unwrap_or_else(|| list_comment_channel_id(&meta.list_id));
    if root.channel.is_none() {
        root.channel = Some(channel.clone());
    }
    let thread_ts = root.thread_ts.clone().unwrap_or_else(|| {
        if root.ts.as_str().is_empty() {
            Ts::from(format!("{}-{}", meta.list_id, meta.record_id))
        } else {
            root.ts.clone()
        }
    });
    if let Some(last) = messages.last() {
        root.latest_reply = Some(last.ts.clone());
    }
    let reply_total = root
        .reply_count
        .unwrap_or_default()
        .max(u32::try_from(messages.len()).unwrap_or(u32::MAX));
    let last_read = root.last_read.clone();
    Some(ThreadCard {
        channel,
        thread_ts,
        root,
        replies: messages,
        reply_total,
        unread: 0,
        last_read,
        list_item: Some(meta),
    })
}

/// A card from a root and its full reply list.
fn card(channel: ChannelId, root: Message, replies: &[Message]) -> ThreadCard {
    let last_read = root.last_read.clone();
    let list_item = detect_list_item_meta(&root);
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
        list_item,
    }
}

/// Order a card list like Slack's Threads view: unread first, then latest activity.
pub(crate) fn sort_cards(cards: &mut [ThreadCard]) {
    cards.sort_by_key(|card| std::cmp::Reverse(card.sort_key()));
}

/// Sample list item thread shown in fixture mode alongside channel threads so list item threads
/// can be inspected and filtered in the Threads view (`F-COLLAB-250`, `F-COLLAB-251`).
fn fixture_list_item_card(fixtures: &Fixtures) -> Option<ThreadCard> {
    let peer = fixtures
        .users
        .iter()
        .find(|u| u.id != fixtures.self_user)
        .map(|u| u.id.clone())?;
    let meta = ListItemThreadMeta {
        list_id: "fixture".to_owned(),
        list_title: "Launch tasks".to_owned(),
        record_id: "fix_2".to_owned(),
        item_title: "Beta invite list".to_owned(),
    };
    let root = Message {
        channel: Some(ChannelId::from("C0LISTFIXTURE")),
        ts: Ts::from("1700000100.000100"),
        user: Some(peer),
        text: "Finalized the first 25 external beta workspaces — ready for sign-off.".into(),
        reply_count: Some(1),
        latest_reply: Some(Ts::from("1700000105.000200")),
        reply_users: vec![fixtures.self_user.clone()],
        ..Default::default()
    };
    let reply = Message {
        channel: Some(ChannelId::from("C0LISTFIXTURE")),
        ts: Ts::from("1700000105.000200"),
        thread_ts: Some(Ts::from("1700000100.000100")),
        user: Some(fixtures.self_user.clone()),
        text: "Looks great — let's move this item to Done once the invites go out.".into(),
        ..Default::default()
    };
    list_item_thread_card(meta, vec![root, reply])
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
    if let Some(list_card) = fixture_list_item_card(fixtures) {
        cards.push(list_card);
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
            let list_item = detect_list_item_meta(&root);
            Some(ThreadCard {
                thread_ts: root.thread_ts.clone().unwrap_or_else(|| root.ts.clone()),
                replies,
                reply_total,
                unread: unread_count,
                last_read,
                channel,
                root,
                list_item,
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

struct ThreadMsgRenderArgs<'a> {
    fixtures: &'a Arc<Fixtures>,
    card: &'a ThreadCard,
    msg: &'a Message,
    key: String,
    msg_group: SharedString,
    grouped: Option<SharedString>,
    is_first_in_card: bool,
    saved_set: &'a HashSet<Ts>,
    editing: Option<&'a (ChannelId, Ts, Entity<TextareaState>)>,
    links: &'a LinkHandler,
}

/// Render one interactive message row (root or reply) inside a thread card, with hover highlight,
/// saved banner/tint, interactive reaction chips, floating hover toolbar, context menu, and
/// inline edit mode.
fn render_thread_message(
    args: ThreadMsgRenderArgs<'_>,
    cx: &mut Context<ThreadsView>,
) -> AnyElement {
    let ThreadMsgRenderArgs {
        fixtures,
        card,
        msg,
        key,
        msg_group,
        grouped,
        is_first_in_card,
        saved_set,
        editing,
        links,
    } = args;
    let theme = cx.theme();
    let (bg, ring, hover_bg, saved_bg) = (
        theme.background,
        theme.ring,
        theme.secondary.opacity(0.5),
        theme.warning.opacity(0.08),
    );
    if let Some((edit_ch, edit_ts, edit_input)) = editing
        && *edit_ch == card.channel
        && *edit_ts == msg.ts
    {
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
    let quick = crate::preferences::active_quick_reactions(&fixtures.quick_reactions, cx);
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

pub(super) struct ThreadCardRenderArgs<'a> {
    pub fixtures: &'a Arc<Fixtures>,
    pub ix: usize,
    pub card: &'a ThreadCard,
    pub active: bool,
    pub expanded: bool,
    pub saved_set: &'a HashSet<Ts>,
    pub editing: Option<&'a (ChannelId, Ts, Entity<TextareaState>)>,
    pub composer: &'a Entity<Composer>,
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
pub(super) fn card_view(
    args: ThreadCardRenderArgs<'_>,
    cx: &mut Context<ThreadsView>,
) -> AnyElement {
    let ThreadCardRenderArgs {
        fixtures,
        ix,
        card,
        active,
        expanded,
        saved_set,
        editing,
        composer,
    } = args;
    let theme = cx.theme();
    let (border, muted, background, link, danger, danger_foreground, success) = (
        theme.border,
        theme.muted_foreground,
        theme.background,
        theme.link,
        theme.danger,
        theme.danger_foreground,
        theme.success,
    );
    let conversation = fixtures.conversation(&card.channel);
    let kind = conversation.map(|c| c.kind());
    let raw_label = conversation
        .map(|c| fixtures.conversation_label(c))
        .unwrap_or_else(|| card.channel.to_string());
    let channel_name = if let Some(meta) = &card.list_item {
        format!("{} · {}", meta.list_title, meta.item_title)
    } else {
        match kind {
            Some(ConversationKind::PublicChannel) | Some(ConversationKind::PrivateChannel) => {
                raw_label.trim_start_matches("# ").to_owned()
            }
            _ => raw_label,
        }
    };
    let is_list_item = card.list_item.is_some();
    let badge = if is_list_item {
        Icon::new(IconName::ListChecks)
            .text_color(success)
            .size(px(15.))
            .into_any_element()
    } else {
        match kind {
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
        }
    };
    let participants = participant_summary(fixtures, card);
    let open = (card.channel.clone(), card.thread_ts.clone());
    let open_list_item = card
        .list_item
        .as_ref()
        .map(|m| (m.list_id.clone(), m.record_id.clone()));

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
        ThreadMsgRenderArgs {
            fixtures,
            card,
            msg: &card.root,
            key: format!("thread-{ix}-root-{}", card.root.ts.as_str()),
            msg_group: SharedString::from(format!("thread-{ix}-root")),
            grouped: None,
            is_first_in_card: true,
            saved_set,
            editing,
            links: &links,
        },
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
            ThreadMsgRenderArgs {
                fixtures,
                card,
                msg: reply,
                key: format!("thread-{ix}-reply-{}-{}", r_ix, reply.ts.as_str()),
                msg_group,
                grouped,
                is_first_in_card: false,
                saved_set,
                editing,
                links: &links,
            },
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
                    if let Some((list_id, record_id)) = &open_list_item {
                        cx.emit(ThreadsEvent::OpenListItem {
                            list_id: list_id.clone(),
                            record_id: record_id.clone(),
                        });
                    } else {
                        cx.emit(ThreadsEvent::Open {
                            channel: open.0.clone(),
                            thread_ts: open.1.clone(),
                        });
                    }
                }))
                .child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(badge)
                        .when(is_list_item, |head| {
                            head.child(
                                div()
                                    .rounded_sm()
                                    .bg(success.opacity(0.14))
                                    .px_1p5()
                                    .py(px(1.))
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(success)
                                    .child("List item"),
                            )
                        })
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

    #[test]
    fn list_item_threads_detect_metadata_build_cards_and_appear_in_local_cards() {
        let mut msg = Message {
            channel: Some(ChannelId::from("C0LIST1")),
            ts: Ts::from("1700000000.000100"),
            text: "Item comment".into(),
            subtype: Some(MessageSubtype::Other("list_record_comment".into())),
            ..Default::default()
        };
        msg.extra.insert(
            "list_record".into(),
            crate::blockkit::serde_json::json!({
                "list_id": "F0LIST1",
                "list_title": "Q4 Launch",
                "record_id": "Rec01",
                "title": "Ship release"
            }),
        );
        let meta = detect_list_item_meta(&msg).expect("should detect list_record_comment");
        assert_eq!(meta.list_id, "F0LIST1");
        assert_eq!(meta.list_title, "Q4 Launch");
        assert_eq!(meta.record_id, "Rec01");
        assert_eq!(meta.item_title, "Ship release");

        let card = list_item_thread_card(
            meta.clone(),
            vec![
                Message {
                    ts: Ts::from("1700000000.000200"),
                    text: "Reply".into(),
                    ..Default::default()
                },
                Message {
                    ts: Ts::from("1700000000.000100"),
                    text: "Root".into(),
                    ..Default::default()
                },
            ],
        )
        .expect("builds sorted list item card");
        assert_eq!(card.root.text, "Root");
        assert_eq!(card.replies.len(), 1);
        assert_eq!(card.replies[0].text, "Reply");
        assert_eq!(card.channel, ChannelId::from("C0LIST1"));
        assert_eq!(card.list_item, Some(meta));

        let fixtures = fixtures::load();
        let cards = local_cards(&fixtures);
        assert!(
            cards.iter().any(|c| c.list_item.is_some()),
            "sample workspace Threads view should include a list item thread"
        );
    }
}
