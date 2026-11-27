//! The "New message" view's recipient model and To-field dropdown: what a composed message
//! can target ([`Recipient`]), the suggestion list over conversations and people, and the
//! dropdown's rows. The view entity lives in [`crate::new_message`].
//!
//! Conversations come first, ordered by latest activity (Slack's "recent"), people without a
//! DM after them; prefix matches rank before substring matches.

use std::collections::HashSet;

use gpui_kit::component::input::TextareaState;
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, Context, Entity, Focusable as _, FontWeight, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window, div,
};
use slackless_core::api::MessageContent;
use slackless_core::types::{ChannelId, Conversation, ConversationKind, Ts, UserId};

use crate::avatars::{conversation_avatar, user_chip};
use crate::fixtures::Fixtures;
use crate::new_message::{NewMessageEvent, NewMessageView};

/// Suggestions listed at once.
const MAX_ROWS: usize = 8;

/// Slack's group DM limit: up to 8 other people (9 total including yourself).
pub(crate) const MAX_GROUP_USERS: usize = 8;

/// Where a composed message goes.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Recipient {
    /// An existing conversation, ready to receive the message.
    Conversation(ChannelId),
    /// A person; one or more people open a DM / group DM (`conversations.open`).
    User(UserId),
}

/// One row of the To field's dropdown.
pub(crate) struct Suggestion {
    pub label: String,
    pub kind: &'static str,
    pub recipient: Recipient,
}

/// A picked recipient chip in the To field.
#[derive(Clone)]
pub(crate) struct Picked {
    pub recipient: Recipient,
    pub label: String,
}

/// Every pickable recipient matching `query` (lowercased, trimmed), best first.
/// When one or more people are already picked, only additional people are offered (up to
/// [`MAX_GROUP_USERS`]) so the user can compose a group DM.
pub(crate) fn suggestions(fixtures: &Fixtures, picked: &[Picked], query: &str) -> Vec<Suggestion> {
    let people_only = !picked.is_empty()
        && picked
            .iter()
            .all(|p| matches!(p.recipient, Recipient::User(_)));
    if !picked.is_empty() && !people_only {
        return Vec::new();
    }
    if people_only && picked.len() >= MAX_GROUP_USERS {
        return Vec::new();
    }
    let chosen_users: HashSet<&UserId> = picked
        .iter()
        .filter_map(|p| match &p.recipient {
            Recipient::User(u) => Some(u),
            Recipient::Conversation(_) => None,
        })
        .collect();

    let all: Vec<Suggestion> = if people_only {
        fixtures
            .people()
            .filter(|u| !u.deleted && !chosen_users.contains(&u.id) && u.id != fixtures.self_user)
            .map(|user| Suggestion {
                label: fixtures.user_name(&user.id).to_owned(),
                kind: "Person",
                recipient: Recipient::User(user.id.clone()),
            })
            .collect()
    } else {
        let mut conversations: Vec<Conversation> = fixtures
            .channels
            .iter()
            .chain(&fixtures.dms)
            .cloned()
            .collect();
        conversations
            .sort_by_key(|conversation| std::cmp::Reverse(latest_activity(fixtures, conversation)));
        let with_dm: HashSet<UserId> = fixtures
            .dms
            .iter()
            .filter_map(|dm| dm.user.clone())
            .collect();
        conversations
            .into_iter()
            .map(|conversation| {
                // Picking a 1:1 IM's peer as `User` lets the user add more people to form a
                // group DM; if sent alone, the root still routes to the existing IM.
                let (recipient, label, kind) = match conversation.kind() {
                    ConversationKind::Im => match conversation.user.clone() {
                        Some(peer) => {
                            let is_self = peer == fixtures.self_user;
                            let name = if is_self {
                                format!("{} (you)", fixtures.user_name(&peer))
                            } else {
                                fixtures.user_name(&peer).to_owned()
                            };
                            (Recipient::User(peer), name, "Direct message")
                        }
                        None => (
                            Recipient::Conversation(conversation.id.clone()),
                            fixtures.conversation_label(&conversation),
                            "Direct message",
                        ),
                    },
                    ConversationKind::Mpim => (
                        Recipient::Conversation(conversation.id.clone()),
                        fixtures.conversation_label(&conversation),
                        "Group DM",
                    ),
                    _ => (
                        Recipient::Conversation(conversation.id.clone()),
                        fixtures.conversation_label(&conversation),
                        "Channel",
                    ),
                };
                Suggestion {
                    recipient,
                    label,
                    kind,
                }
            })
            .chain(
                fixtures
                    .people()
                    .filter(|user| !user.deleted && !with_dm.contains(&user.id))
                    .map(|user| {
                        let is_self = user.id == fixtures.self_user;
                        Suggestion {
                            label: if is_self {
                                format!("{} (you)", fixtures.user_name(&user.id))
                            } else {
                                fixtures.user_name(&user.id).to_owned()
                            },
                            kind: if is_self { "Direct message" } else { "Person" },
                            recipient: Recipient::User(user.id.clone()),
                        }
                    }),
            )
            .collect()
    };
    let key = |label: &str| label.trim_start_matches("# ").to_lowercase();
    let (mut prefix, mut rest): (Vec<_>, Vec<_>) = all
        .into_iter()
        .filter(|suggestion| key(&suggestion.label).contains(query))
        .partition(|suggestion| key(&suggestion.label).starts_with(query));
    prefix.append(&mut rest);
    prefix.truncate(MAX_ROWS);
    prefix
}

/// A conversation's latest known activity: Slack's `counts` value, else its stored history.
fn latest_activity(fixtures: &Fixtures, conversation: &Conversation) -> Ts {
    fixtures
        .counts
        .get(&conversation.id)
        .and_then(|counts| counts.latest.clone())
        .or_else(|| {
            fixtures
                .messages
                .get(&conversation.id)
                .and_then(|history| history.last())
                .map(|message| message.ts.clone())
        })
        .unwrap_or_default()
}

/// The dropdown's rows; row `selected` is highlighted while `picking`.
pub(super) fn suggestion_rows(
    fixtures: &Fixtures,
    suggestions: &[Suggestion],
    selected: usize,
    picking: bool,
    cx: &mut Context<NewMessageView>,
) -> Vec<AnyElement> {
    let theme = cx.theme();
    let (hover, active, active_fg, popover, muted) = (
        theme.accent,
        theme.sidebar_accent,
        theme.sidebar_accent_foreground,
        theme.popover,
        theme.muted_foreground,
    );
    suggestions
        .iter()
        .enumerate()
        .map(|(ix, suggestion)| {
            let on = ix == selected && picking;
            let label = suggestion.label.trim_start_matches("# ").to_owned();
            let leading = match &suggestion.recipient {
                Recipient::Conversation(channel) => fixtures
                    .conversation(channel)
                    .map(|conversation| {
                        conversation_avatar(fixtures, conversation, 20., popover, cx)
                    })
                    .unwrap_or_else(div)
                    .into_any_element(),
                Recipient::User(user) => user_chip(fixtures, user, 20.).into_any_element(),
            };
            h_flex()
                .id(("compose-to-row", ix))
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
                        .child(label),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(if on { active_fg } else { muted })
                        .child(suggestion.kind),
                )
                .on_click(cx.listener(move |this, _, window: &mut Window, cx| {
                    this.selected = ix;
                    this.confirm(window, cx);
                }))
                .into_any_element()
        })
        .collect()
}

/// The recipient pick and send routing of [`NewMessageView`]: the To field's query state, the
/// confirm that picks a suggestion, the send that routes by recipient, and the dropdown's
/// Up/Down keys.
impl NewMessageView {
    pub(super) fn on_query_change(&mut self, to: &Entity<TextareaState>, cx: &mut Context<Self>) {
        let value = to.read(cx).value().to_string();
        self.query = value.trim().to_lowercase();
        self.selected = 0;
        cx.notify();
    }

    pub(super) fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.query.clone();
        let fixtures = self.fixtures.clone();
        let Some(suggestion) = suggestions(&fixtures, &self.picked, &query)
            .into_iter()
            .nth(self.selected)
        else {
            return;
        };
        let label = suggestion.label.trim_start_matches("# ").to_owned();
        match &suggestion.recipient {
            Recipient::Conversation(_) => {
                self.picked = vec![Picked {
                    recipient: suggestion.recipient,
                    label,
                }];
            }
            Recipient::User(user) => {
                if !self
                    .picked
                    .iter()
                    .any(|p| matches!(&p.recipient, Recipient::User(u) if u == user))
                    && self.picked.len() < MAX_GROUP_USERS
                {
                    self.picked.push(Picked {
                        recipient: suggestion.recipient,
                        label,
                    });
                }
            }
        }
        self.query.clear();
        self.selected = 0;
        self.to
            .update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    pub(super) fn remove_picked(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.picked.len() {
            self.picked.remove(index);
            self.selected = 0;
            cx.notify();
        }
    }

    pub(super) fn send(
        &mut self,
        content: MessageContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.picked.is_empty() {
            // Keep the draft (its mrkdwn reads back with formatting): nothing is picked yet.
            self.composer
                .update(cx, |composer, cx| composer.load_draft(&content.text, cx));
            self.to.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
            return;
        }
        if let [
            Picked {
                recipient: Recipient::Conversation(channel),
                ..
            },
        ] = self.picked.as_slice()
        {
            cx.emit(NewMessageEvent::Send {
                channel: channel.clone(),
                content,
            });
            return;
        }
        let users: Vec<UserId> = self
            .picked
            .iter()
            .filter_map(|p| match &p.recipient {
                Recipient::User(u) => Some(u.clone()),
                Recipient::Conversation(_) => None,
            })
            .collect();
        if !users.is_empty() {
            cx.emit(NewMessageEvent::OpenDm { users, content });
        }
    }

    pub(super) fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key.as_str() == "backspace"
            && self.query.is_empty()
            && !self.picked.is_empty()
        {
            self.picked.pop();
            self.selected = 0;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let (fixtures, query) = (self.fixtures.clone(), self.query.clone());
        let count = suggestions(&fixtures, &self.picked, &query).len();
        match event.keystroke.key.as_str() {
            "down" if count > 0 => self.selected = (self.selected + 1) % count,
            "up" if count > 0 => self.selected = (self.selected + count - 1) % count,
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}
