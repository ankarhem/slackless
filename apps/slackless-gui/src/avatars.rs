//! Avatars shared by the sidebar, the DM list, the conversation header and message rows:
//! the profile photo (Slack's `image_*` URL, loaded and cached by GPUI's `img`), else an
//! initials chip in the user's identity color; Slack's presence dot (filled green when
//! active, a hollow ring when away) and the stacked two-chip avatar of group DMs.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::gpui::{
    App, Div, FontWeight, Hsla, ParentElement as _, SharedString, Styled as _, div, img, px,
};
use slackless_core::types::{Conversation, Message, Presence, Profile, UserId};

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
            with_presence(chip, fixtures.presence(peer), size, ring, cx)
        }
        [only] => user_chip(fixtures, only, size),
        [first, second, ..] => stacked(fixtures, first, second, size, ring),
    }
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
    let bot_icon = message
        .bot_profile
        .as_ref()
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

/// The profile image for an avatar `size` points wide, at twice that for Retina.
fn photo_url(profile: &Profile, size: f32) -> Option<&str> {
    let candidates = if size > 24. {
        [&profile.image_72, &profile.image_192, &profile.image_48]
    } else {
        [&profile.image_48, &profile.image_72, &profile.image_32]
    };
    candidates.into_iter().find_map(|url| url.as_deref())
}

/// A rounded-square remote image, on the identity color of `key` while it loads.
fn photo(url: &str, key: &str, size: f32) -> Div {
    div()
        .size(px(size))
        .flex_none()
        .rounded(px((size / 5.).max(3.)))
        .overflow_hidden()
        .bg(theme::avatar_color(key))
        .child(img(SharedString::from(url.to_owned())).size_full())
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
        .rounded(px((size / 5.).max(3.)))
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
    presence: Option<&Presence>,
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
