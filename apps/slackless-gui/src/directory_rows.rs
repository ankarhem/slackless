//! The directory's row lists and filters: the people matching a search (by name or title) and
//! the channels matching one (by name or topic), plus their rows. The view entity and tabs
//! live in [`crate::directory`].

use std::collections::HashSet;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use slackless_core::types::{ChannelId, Conversation, ConversationKind, UserId};

use crate::avatars::user_chip;
use crate::directory::{DirectoryEvent, DirectoryView};
use crate::fixtures::Fixtures;

/// People matching `needle` (name or title, case-insensitive), by name.
pub(super) fn people(fixtures: &Fixtures, needle: &str) -> Vec<UserId> {
    let mut people: Vec<UserId> = fixtures
        .users
        .iter()
        .filter(|user| {
            let name = fixtures.user_name(&user.id).to_lowercase();
            let title = user.profile.title.as_deref().unwrap_or_default();
            needle.is_empty() || name.contains(needle) || title.to_lowercase().contains(needle)
        })
        .map(|user| user.id.clone())
        .collect();
    people.sort_by(|a, b| {
        fixtures
            .user_name(a)
            .to_lowercase()
            .cmp(&fixtures.user_name(b).to_lowercase())
    });
    people
}

/// Channels matching `needle` (name or topic, case-insensitive), by name.
pub(super) fn channels(fixtures: &Fixtures, needle: &str) -> Vec<Conversation> {
    let mut channels: Vec<Conversation> = fixtures
        .channels
        .iter()
        .chain(&fixtures.directory)
        .filter(|channel| {
            needle.is_empty()
                || channel
                    .name
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(needle)
                || channel
                    .topic
                    .as_ref()
                    .is_some_and(|t| t.value.to_lowercase().contains(needle))
        })
        .cloned()
        .collect();
    channels.sort_by(|a, b| a.name.cmp(&b.name));
    channels
}

/// One person row: avatar, name and title; a click opens the profile panel.
pub(super) fn person_row(
    view: &DirectoryView,
    ix: usize,
    user: &UserId,
    cx: &mut Context<DirectoryView>,
) -> AnyElement {
    let fixtures = view.fixtures();
    let theme = cx.theme();
    let (border, muted, foreground, hover) = (
        theme.border,
        theme.muted_foreground,
        theme.foreground,
        theme.accent,
    );
    let name = fixtures.user_name(user).to_owned();
    let title = fixtures
        .user(user)
        .and_then(|user| user.profile.title.clone())
        .unwrap_or_default();
    let open = user.clone();
    h_flex()
        .id(("directory-person", ix))
        .items_center()
        .gap_3()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(border)
        .cursor_pointer()
        .hover(|row| row.bg(hover))
        .child(user_chip(fixtures, user, 32.))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(foreground)
                        .child(name),
                )
                .child(div().truncate().text_xs().text_color(muted).child(title)),
        )
        .on_click(
            cx.listener(move |_, _, _, cx| cx.emit(DirectoryEvent::OpenProfile(open.clone()))),
        )
        .into_any_element()
}

/// One channel row: icon, name, member count and topic; a click opens it, and an unjoined
/// one offers the channel browser's local Join first.
pub(super) fn channel_row(
    ix: usize,
    channel: &Conversation,
    joined: &HashSet<ChannelId>,
    cx: &mut Context<DirectoryView>,
) -> AnyElement {
    let theme = cx.theme();
    let (border, muted, foreground, success, hover) = (
        theme.border,
        theme.muted_foreground,
        theme.foreground,
        theme.success,
        theme.accent,
    );
    let id = channel.id.clone();
    let join_id = channel.id.clone();
    let member = channel.is_member || joined.contains(&channel.id);
    let icon = if channel.kind() == ConversationKind::PrivateChannel {
        IconName::Lock
    } else {
        IconName::Hash
    };
    let members = channel.num_members.unwrap_or(0);
    let meta = format!(
        "{members} members{}",
        channel
            .topic
            .as_ref()
            .filter(|t| !t.value.is_empty())
            .map_or_else(String::new, |t| format!(" · {}", t.value))
    );
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
        Button::new(SharedString::from(format!("directory-join-{id}")))
            .primary()
            .with_size(Size::Small)
            .label("Join")
            .on_click(cx.listener(move |this, _, _, cx| {
                this.join(join_id.clone(), cx);
            }))
            .into_any_element()
    };
    h_flex()
        .id(("directory-channel", ix))
        .items_center()
        .gap_3()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(border)
        .cursor_pointer()
        .hover(|row| row.bg(hover))
        .child(
            Icon::new(icon)
                .text_color(foreground)
                .size(px(16.))
                .flex_none(),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(foreground)
                        .child(channel.name.clone().unwrap_or_default()),
                )
                .child(div().truncate().text_xs().text_color(muted).child(meta)),
        )
        .child(action)
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(DirectoryEvent::Open(id.clone()))))
        .into_any_element()
}
