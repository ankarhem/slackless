//! Activity feed (docs/research/api-activity.md: the `activity.feed` item types
//! `message_mention`, `message_reaction` and `thread_reply`): what happened to the signed-in
//! user, newest first, with All / Mentions / Reactions / Threads filters, per-item read state
//! and "Mark all as read". It renders in the sidebar's list column when the rail's Activity
//! destination is active; its unread count drives the rail badge and the dock badge.
//!
//! Fixture mode derives the feed from the workspace fixtures:
//! - mentions: messages by others that @-mention the signed-in user; unread when newer than
//!   the conversation's `last_read`;
//! - reactions: the signed-in user's messages that others reacted to; the newest two unread;
//! - threads: threads the signed-in user started or replied in whose latest reply is by
//!   someone else; unread.
//!
//! `ActivityFeed::new(Arc<Fixtures>, &mut App) -> Entity<ActivityFeed>`
//! emits `ActivityEvent::Open { channel, thread_ts }` (thread replies open their thread)

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
};
use slackless_core::types::{ChannelId, Message, Ts};

use crate::emoji;
use crate::fixtures::Fixtures;
use crate::messages::list_stamp;
use crate::sidebar::mentions_user;

/// Reaction items marked unread in fixture mode (the newest ones).
const UNREAD_REACTIONS: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Mentions,
    Reactions,
    Threads,
}

impl Filter {
    const ALL: [Filter; 4] = [
        Filter::All,
        Filter::Mentions,
        Filter::Reactions,
        Filter::Threads,
    ];

    fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Mentions => "Mentions",
            Filter::Reactions => "Reactions",
            Filter::Threads => "Threads",
        }
    }

    fn admits(self, kind: &Kind) -> bool {
        matches!(
            (self, kind),
            (Filter::All, _)
                | (Filter::Mentions, Kind::Mention)
                | (Filter::Reactions, Kind::Reaction { .. })
                | (Filter::Threads, Kind::ThreadReply)
        )
    }
}

enum Kind {
    Mention,
    /// `glyph` of the newest reaction, or its `:name:`.
    Reaction {
        glyph: String,
    },
    ThreadReply,
}

struct Item {
    kind: Kind,
    channel: ChannelId,
    /// Thread root, for thread replies (opens the thread).
    thread_ts: Option<Ts>,
    /// When it happened (sort key and stamp).
    ts: Ts,
    /// Who did it: author, reactors or repliers.
    actor: String,
    /// Conversation label.
    label: String,
    preview: String,
    unread: bool,
}

pub enum ActivityEvent {
    Open {
        channel: ChannelId,
        thread_ts: Option<Ts>,
    },
}

pub struct ActivityFeed {
    items: Vec<Item>,
    filter: Filter,
}

impl EventEmitter<ActivityEvent> for ActivityFeed {}

impl ActivityFeed {
    pub fn new(fixtures: Arc<Fixtures>, cx: &mut App) -> Entity<ActivityFeed> {
        cx.new(|_| ActivityFeed {
            items: build(&fixtures),
            filter: Filter::All,
        })
    }

    /// Unread items across every filter.
    pub fn unread_count(&self) -> u32 {
        self.items.iter().filter(|item| item.unread).count() as u32
    }

    fn unread_in(&self, filter: Filter) -> usize {
        self.items
            .iter()
            .filter(|item| item.unread && filter.admits(&item.kind))
            .count()
    }

    fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        for item in self
            .items
            .iter_mut()
            .filter(|item| self.filter.admits(&item.kind))
        {
            item.unread = false;
        }
        cx.notify();
    }

    fn toggle_read(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.items[ix].unread = !self.items[ix].unread;
        cx.notify();
    }

    fn open(&mut self, ix: usize, cx: &mut Context<Self>) {
        let item = &mut self.items[ix];
        item.unread = false;
        cx.emit(ActivityEvent::Open {
            channel: item.channel.clone(),
            thread_ts: item.thread_ts.clone(),
        });
        cx.notify();
    }

    fn row(&self, ix: usize, item: &Item, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (fg, dim, hover, border) = (
            theme.sidebar_foreground,
            theme.sidebar_foreground.opacity(0.6),
            theme.accent,
            theme.sidebar_border,
        );
        let (icon, icon_color, verb) = match &item.kind {
            Kind::Mention => (IconName::AtSign, theme.danger, "Mention in".to_owned()),
            Kind::Reaction { glyph } => (
                IconName::Heart,
                theme.warning,
                format!("Reacted {glyph} in"),
            ),
            Kind::ThreadReply => (
                IconName::MessageSquareReply,
                theme.link,
                "Thread in".to_owned(),
            ),
        };
        let unread_dot = theme.sidebar_primary;
        let unread = item.unread;
        let group = SharedString::from(format!("activity-group-{ix}"));
        h_flex()
            .id(("activity", ix))
            .group(group.clone())
            .relative()
            .items_start()
            .gap_2()
            .px_2()
            .py_1p5()
            .rounded_md()
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .hover(|row| row.bg(hover))
            .child(
                div()
                    .mt_0p5()
                    .flex_none()
                    .child(Icon::new(icon).text_color(icon_color).size(px(14.))),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .text_xs()
                            .text_color(dim)
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .child(format!("{verb} {}", item.label)),
                            )
                            .child(div().flex_none().child(list_stamp(&item.ts))),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_color(fg)
                            .font_weight(if unread {
                                FontWeight::BOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .child(item.actor.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(dim)
                            .child(item.preview.clone()),
                    ),
            )
            .child(
                v_flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .size(px(8.))
                            .mt_1()
                            .rounded_full()
                            .when(unread, |dot| dot.bg(unread_dot)),
                    )
                    .child(
                        div()
                            .opacity(0.)
                            .group_hover(group, |el| el.opacity(1.))
                            .child(
                                Button::new(("activity-read", ix))
                                    .ghost()
                                    .with_size(Size::XSmall)
                                    .icon(if unread {
                                        IconName::Check
                                    } else {
                                        IconName::Dot
                                    })
                                    .tooltip(if unread {
                                        "Mark as read"
                                    } else {
                                        "Mark as unread"
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.toggle_read(ix, cx);
                                    })),
                            ),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.open(ix, cx)))
    }
}

impl Render for ActivityFeed {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, dim, fg, hover, primary, primary_fg) = (
            theme.sidebar_border,
            theme.sidebar_foreground.opacity(0.6),
            theme.sidebar_foreground,
            theme.accent,
            theme.sidebar_primary,
            theme.sidebar_primary_foreground,
        );
        let filters = Filter::ALL.map(|filter| {
            let active = filter == self.filter;
            let unread = self.unread_in(filter);
            h_flex()
                .id(SharedString::from(format!(
                    "activity-filter-{}",
                    filter.label()
                )))
                .flex_none()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
                .rounded_full()
                .text_xs()
                .cursor_pointer()
                .border_1()
                .border_color(border)
                .when(active, |chip| {
                    chip.bg(primary)
                        .text_color(primary_fg)
                        .border_color(primary)
                })
                .when(!active, |chip| {
                    chip.text_color(dim).hover(|el| el.bg(hover))
                })
                .child(filter.label())
                .when(unread > 0, |chip| chip.child(unread.to_string()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.filter = filter;
                    cx.notify();
                }))
        });

        let filter = self.filter;
        let mut order: Vec<usize> = (0..self.items.len())
            .filter(|&ix| filter.admits(&self.items[ix].kind))
            .collect();
        order.sort_by(|&a, &b| self.items[b].ts.cmp(&self.items[a].ts));
        let rows: Vec<_> = order
            .into_iter()
            .map(|ix| self.row(ix, &self.items[ix], cx))
            .collect();
        let empty = rows.is_empty();
        let any_unread = self.unread_in(filter) > 0;

        v_flex()
            .child(
                h_flex()
                    .px_4()
                    .pt_3()
                    .pb_2()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .flex_1()
                            .text_base()
                            .font_weight(FontWeight::BOLD)
                            .text_color(fg)
                            .child("Activity"),
                    )
                    .child(
                        Button::new("activity-mark-all")
                            .ghost()
                            .with_size(Size::XSmall)
                            .icon(IconName::CheckCheck)
                            .tooltip("Mark all as read")
                            .disabled(!any_unread)
                            .on_click(cx.listener(|this, _, _, cx| this.mark_all_read(cx))),
                    ),
            )
            .child(h_flex().flex_wrap().gap_1().px_2().pt_2().children(filters))
            .child(v_flex().p_2().gap_1().children(rows))
            .when(empty, |list| {
                list.child(
                    v_flex()
                        .items_center()
                        .gap_2()
                        .p_6()
                        .text_color(dim)
                        .child(
                            Icon::new(IconName::CircleCheck)
                                .text_color(dim)
                                .size(px(28.)),
                        )
                        .child("You're all caught up"),
                )
            })
    }
}

/// Derive the fixture feed (see the module docs).
fn build(fixtures: &Fixtures) -> Vec<Item> {
    let me = &fixtures.self_user;
    let name = |message: &Message| {
        message
            .user
            .as_ref()
            .map(|user| fixtures.user_name(user).to_owned())
            .unwrap_or_default()
    };
    let mut items = Vec::new();
    let conversations = fixtures.channels.iter().chain(&fixtures.dms);

    let mut reactions = Vec::new();
    for conversation in conversations {
        let label = fixtures.conversation_label(conversation);
        let history = fixtures
            .messages
            .get(&conversation.id)
            .into_iter()
            .flatten();
        for message in history {
            let from_me = message.user.as_ref() == Some(me);
            if !from_me && mentions_user(message, me) {
                let unread = conversation
                    .last_read
                    .as_ref()
                    .is_none_or(|read| message.ts > *read);
                items.push(Item {
                    kind: Kind::Mention,
                    channel: conversation.id.clone(),
                    thread_ts: None,
                    ts: message.ts.clone(),
                    actor: name(message),
                    label: label.clone(),
                    preview: fixtures.preview(message),
                    unread,
                });
            }
            let others: Vec<&str> = message
                .reactions
                .iter()
                .flat_map(|reaction| reaction.users.iter())
                .filter(|user| *user != me)
                .map(|user| fixtures.user_name(user))
                .collect();
            if from_me && !others.is_empty() {
                let newest = message
                    .reactions
                    .last()
                    .map(|reaction| reaction.name.as_str())
                    .unwrap_or("");
                let glyph = emoji::glyph(newest)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!(":{newest}:"));
                let mut actor: Vec<&str> = Vec::new();
                for user in others {
                    if !actor.contains(&user) {
                        actor.push(user);
                    }
                }
                reactions.push(Item {
                    kind: Kind::Reaction { glyph },
                    channel: conversation.id.clone(),
                    thread_ts: None,
                    ts: message.ts.clone(),
                    actor: actor.join(", "),
                    label: label.clone(),
                    preview: fixtures.preview(message),
                    unread: false,
                });
            }
        }
    }
    reactions.sort_by(|a, b| b.ts.cmp(&a.ts));
    for item in reactions.iter_mut().take(UNREAD_REACTIONS) {
        item.unread = true;
    }
    items.extend(reactions);

    for ((channel, root_ts), replies) in &fixtures.threads {
        let root = fixtures
            .messages
            .get(channel)
            .and_then(|history| history.iter().find(|message| message.ts == *root_ts));
        let involved = root.is_some_and(|root| root.user.as_ref() == Some(me))
            || replies.iter().any(|reply| reply.user.as_ref() == Some(me));
        let Some(latest) = replies.iter().max_by(|a, b| a.ts.cmp(&b.ts)) else {
            continue;
        };
        if !involved || latest.user.as_ref() == Some(me) {
            continue;
        }
        let label = fixtures
            .conversation(channel)
            .map(|conversation| fixtures.conversation_label(conversation))
            .unwrap_or_else(|| channel.to_string());
        let mut repliers: Vec<String> = Vec::new();
        for reply in replies
            .iter()
            .filter(|reply| reply.user.as_ref() != Some(me))
        {
            let reply_name = name(reply);
            if !repliers.contains(&reply_name) {
                repliers.push(reply_name);
            }
        }
        items.push(Item {
            kind: Kind::ThreadReply,
            channel: channel.clone(),
            thread_ts: Some(root_ts.clone()),
            ts: latest.ts.clone(),
            actor: repliers.join(", "),
            label,
            preview: fixtures.preview(latest),
            unread: true,
        });
    }
    items
}
