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

/// Where a composed message goes.
#[derive(Clone)]
pub(crate) enum Recipient {
    /// An existing conversation, ready to receive the message.
    Conversation(ChannelId),
    /// A person with no DM yet; the root opens one before delivering.
    User(UserId),
}

/// One row of the To field's dropdown.
pub(crate) struct Suggestion {
    pub label: String,
    pub kind: &'static str,
    pub recipient: Recipient,
}

/// The picked recipient and the label shown in the To field.
#[derive(Clone)]
pub(crate) struct Picked {
    pub recipient: Recipient,
    pub label: String,
}

/// Every pickable recipient matching `query` (lowercased, trimmed), best first.
pub(crate) fn suggestions(fixtures: &Fixtures, query: &str) -> Vec<Suggestion> {
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
    let all: Vec<Suggestion> = conversations
        .into_iter()
        .map(|conversation| Suggestion {
            recipient: Recipient::Conversation(conversation.id.clone()),
            label: fixtures.conversation_label(&conversation),
            kind: match conversation.kind() {
                ConversationKind::Im => "Direct message",
                ConversationKind::Mpim => "Group DM",
                _ => "Channel",
            },
        })
        .chain(
            fixtures
                .users
                .iter()
                .filter(|user| user.id != fixtures.self_user && !with_dm.contains(&user.id))
                .map(|user| Suggestion {
                    label: fixtures.user_name(&user.id).to_owned(),
                    kind: "Person",
                    recipient: Recipient::User(user.id.clone()),
                }),
        )
        .collect();
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
        if self
            .picked
            .as_ref()
            .is_some_and(|picked| picked.label != value)
        {
            self.picked = None;
        }
        self.query = value.trim().to_lowercase();
        self.selected = 0;
        cx.notify();
    }

    pub(super) fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.query.clone();
        let fixtures = self.fixtures.clone();
        let Some(suggestion) = suggestions(&fixtures, &query)
            .into_iter()
            .nth(self.selected)
        else {
            return;
        };
        let label = suggestion.label.trim_start_matches("# ").to_owned();
        self.picked = Some(Picked {
            recipient: suggestion.recipient,
            label: label.clone(),
        });
        self.selected = 0;
        self.to
            .update(cx, |state, cx| state.set_value(label, window, cx));
        cx.notify();
    }

    pub(super) fn send(
        &mut self,
        content: MessageContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(picked) = self.picked.clone() else {
            // Keep the draft (its mrkdwn reads back with formatting): nothing is picked yet.
            self.composer
                .update(cx, |composer, cx| composer.load_draft(&content.text, cx));
            self.to.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
            return;
        };
        match picked.recipient {
            Recipient::Conversation(channel) => {
                cx.emit(NewMessageEvent::Send { channel, content });
            }
            Recipient::User(user) => {
                cx.emit(NewMessageEvent::OpenDm { user, content });
            }
        }
    }

    pub(super) fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (fixtures, query) = (self.fixtures.clone(), self.query.clone());
        let count = suggestions(&fixtures, &query).len();
        match event.keystroke.key.as_str() {
            "down" if count > 0 => self.selected = (self.selected + 1) % count,
            "up" if count > 0 => self.selected = (self.selected + count - 1) % count,
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}
