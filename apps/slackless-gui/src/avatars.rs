//! Avatars shared by the sidebar, the DM list, the conversation header and message rows:
//! the profile photo (Slack's `image_*` URL, loaded and cached by GPUI's `img`), else an
//! initials chip in the user's identity color; Slack's presence dot (filled green when
//! active, a hollow ring when away) and the stacked two-chip avatar of group DMs.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon};
use gpui_kit::gpui::{
    App, Div, FontWeight, Hsla, ParentElement as _, SharedString, Styled as _, div, img, px,
};
use slackless_core::types::{Conversation, Message, Presence, Profile, Team, UserId};

use crate::fixtures::Fixtures;
use crate::theme;

/// The avatar for a conversation row or header: the peer chip with its presence dot for IMs,
/// stacked peer chips for MPIMs, nothing for channels. `ring` is the surface color behind
/// the avatar, used to cut the dot and the front chip out of what they overlap.
pub fn conversation_avatar(
    fixtures: &Fixtures,
    conversation: &Conversation,
    size: f32,
    ring: Hsla,
    cx: &App,
) -> Div {
    let peers = fixtures.peers(conversation);
    match peers.as_slice() {
        [] => div(),
        [peer] if !conversation.is_mpim => {
            let chip = user_chip(fixtures, peer, size);
            let in_huddle = fixtures
                .user(peer)
                .and_then(|u| u.profile.huddle_state.as_deref())
                == Some("in_a_huddle");
            let avatar = if let Some(team) = fixtures.external_team(peer) {
                with_external_team_badge(team, chip, size, ring)
            } else {
                with_presence(chip, fixtures.presence(peer), size, ring, cx)
            };
            if in_huddle {
                with_huddle_badge(avatar, size, ring, cx)
            } else {
                avatar
            }
        }
        [only] => user_chip(fixtures, only, size),
        [first, second, ..] => stacked(fixtures, first, second, size, ring),
    }
}

/// Overlay a compact green headphones badge on the top-right corner of an avatar when the user's
/// Slack profile reports `huddle_state == "in_a_huddle"` (`F-COLLAB-001`).
pub fn with_huddle_badge(avatar: Div, size: f32, ring: Hsla, cx: &App) -> Div {
    let badge = (size * 0.45).clamp(11., 16.);
    let icon_sz = (badge * 0.65).clamp(7., 10.);
    div().relative().flex_none().child(avatar).child(
        div()
            .absolute()
            .right(px(-3.))
            .top(px(-3.))
            .size(px(badge))
            .rounded_full()
            .bg(cx.theme().success)
            .border_1()
            .border_color(ring)
            .flex()
            .items_center()
            .justify_center()
            .child(
                Icon::new(IconName::Headphones)
                    .size(px(icon_sz))
                    .text_color(theme::avatar_foreground()),
            ),
    )
}

/// The user's profile photo, or an initials chip in their identity color without one.
pub fn user_chip(fixtures: &Fixtures, user: &UserId, size: f32) -> Div {
    match fixtures
        .user(user)
        .and_then(|u| photo_url(&u.profile, size))
    {
        Some(url) => photo(url, user.as_str(), size),
        None => initials_chip(user.as_str(), fixtures.user_name(user), size),
    }
}

/// A message author's avatar: the user's photo, else the bot's icon, else an initials chip.
pub fn author_avatar(
    fixtures: &Fixtures,
    message: &Message,
    key: &str,
    name: &str,
    size: f32,
) -> Div {
    if let Some(user) = message
        .user
        .as_ref()
        .filter(|user| fixtures.user(user).is_some())
    {
        return user_chip(fixtures, user, size);
    }
    let cached_bot = message
        .bot_profile
        .is_none()
        .then(|| {
            message
                .bot_id
                .as_ref()
                .and_then(|id| fixtures.bot_profile(id))
        })
        .flatten();
    let bot_icon = message
        .bot_profile
        .as_ref()
        .or(cached_bot.as_ref())
        .and_then(|bot| bot.icons.as_ref())
        .and_then(|icons| {
            [&icons.image_72, &icons.image_48, &icons.image_36]
                .into_iter()
                .find_map(|url| url.as_deref())
        });
    match bot_icon {
        Some(url) => photo(url, key, size),
        None => initials_chip(key, name, size),
    }
}

/// Overlay an external organization's icon over the bottom-right corner of `avatar`, cut out by
/// a ring of `ring`. Shared by external users and external apps/bots (`F-COLLAB-800`, `804`).
pub fn with_external_team_badge(team: &Team, avatar: Div, size: f32, ring: Hsla) -> Div {
    let badge = (size * 0.44).round().clamp(10., 44.);
    let pad = if size <= 22. { 1. } else { 2. };
    let offset = if size <= 22. { -2. } else { -3. };
    div().relative().flex_none().child(avatar).child(
        div()
            .absolute()
            .right(px(offset))
            .bottom(px(offset))
            .p(px(pad))
            .rounded(px(avatar_radius(badge) + pad))
            .bg(ring)
            .child(crate::channel_details::org_chip(team, badge)),
    )
}

/// Slack's badge on a member of another organization: `avatar` with that organization's icon
/// over its bottom-right corner, cut out of the avatar by a ring of `ring` (the surface behind
/// it). Own-organization people, bots and unknown organizations get `avatar` as is.
pub fn with_external_badge(
    fixtures: &Fixtures,
    user: Option<&UserId>,
    avatar: Div,
    size: f32,
    ring: Hsla,
) -> Div {
    let Some(team) = user.and_then(|user| fixtures.external_team(user)) else {
        return avatar;
    };
    with_external_team_badge(team, avatar, size, ring)
}

/// The profile image for an avatar `size` points wide, at twice that for Retina.
fn photo_url(profile: &Profile, size: f32) -> Option<&str> {
    let candidates = if size > 96. {
        [&profile.image_512, &profile.image_1024, &profile.image_192]
    } else if size > 24. {
        [&profile.image_72, &profile.image_192, &profile.image_48]
    } else {
        [&profile.image_48, &profile.image_72, &profile.image_32]
    };
    candidates.into_iter().find_map(|url| url.as_deref())
}

/// Slack's avatar corner radius: 22.2% of the side, between 4px and 12px (`.c-avatar`).
pub fn avatar_radius(size: f32) -> f32 {
    (size * 0.2222).clamp(4., 12.)
}

/// A rounded-square remote image, on the identity color of `key` while it loads.
fn photo(url: &str, key: &str, size: f32) -> Div {
    div()
        .size(px(size))
        .flex_none()
        .rounded(px(avatar_radius(size)))
        .overflow_hidden()
        .bg(theme::avatar_color(key))
        // GPUI clips square: the image rounds its own corners.
        .child(
            img(SharedString::from(url.to_owned()))
                .size_full()
                .rounded(px(avatar_radius(size))),
        )
}

fn initials_chip(key: &str, label: &str, size: f32) -> Div {
    let initial = label
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let chip = div()
        .size(px(size))
        .flex_none()
        .rounded(px(avatar_radius(size)))
        .flex()
        .items_center()
        .justify_center()
        .font_weight(FontWeight::SEMIBOLD)
        .bg(theme::avatar_color(key))
        .text_color(theme::avatar_foreground())
        .child(initial);
    if size >= 28. {
        chip.text_sm()
    } else {
        chip.text_xs()
    }
}

/// `chip` with the presence dot riding its bottom-right corner.
pub fn with_presence(
    chip: Div,
    presence: Option<Presence>,
    size: f32,
    ring: Hsla,
    cx: &App,
) -> Div {
    let Some(presence) = presence else {
        return chip;
    };
    let dot = (size * 0.45).clamp(9., 13.);
    let inner = match presence {
        Presence::Active => div().size_full().rounded_full().bg(cx.theme().success),
        _ => div()
            .size_full()
            .rounded_full()
            .border_1()
            .border_color(cx.theme().muted_foreground),
    };
    div().relative().flex_none().child(chip).child(
        div()
            .absolute()
            .bottom(px(-2.))
            .right(px(-2.))
            .size(px(dot))
            .p(px(2.))
            .rounded_full()
            .bg(ring)
            .child(inner),
    )
}

/// Group-DM avatar: the first peer's chip top-left, the second's bottom-right on top of it.
fn stacked(fixtures: &Fixtures, back: &UserId, front: &UserId, size: f32, ring: Hsla) -> Div {
    let chip = (size * 0.72).round();
    div()
        .relative()
        .flex_none()
        .size(px(size))
        .child(user_chip(fixtures, back, chip).absolute().top_0().left_0())
        .child(
            user_chip(fixtures, front, chip)
                .absolute()
                .bottom(px(-1.))
                .right(px(-1.))
                .border_1()
                .border_color(ring),
        )
}
