//! The directory's row lists and filters: the people matching a search (by name or title) and
//! the channels matching one (by name or topic), plus their rows. The view entity and tabs
//! live in [`crate::directory`].

use std::collections::HashSet;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::{
    AnyElement, App, ClickEvent, Context, ElementId, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, px,
};
use slackless_core::types::{ChannelId, Conversation, ConversationKind, UserId, Usergroup};

use crate::avatars::user_chip;
use crate::directory::{DirectoryEvent, DirectoryView};
use crate::fixtures::Fixtures;

/// People matching `needle` (name, title, pronouns, contact info, or custom profile fields;
/// case-insensitive, `F-SHELL-005`), sorted by name.
pub(super) fn people(fixtures: &Fixtures, needle: &str) -> Vec<UserId> {
    let mut people: Vec<UserId> = fixtures
        .users
        .iter()
        .filter(|user| {
            if needle.is_empty() {
                return true;
            }
            let name = fixtures.user_name(&user.id).to_lowercase();
            if name.contains(needle) || user.name.to_lowercase().contains(needle) {
                return true;
            }
            let profile_match = [
                user.profile.title.as_deref(),
                user.profile.pronouns.as_deref(),
                user.profile.email.as_deref(),
                user.profile.phone.as_deref(),
            ]
            .into_iter()
            .flatten()
            .any(|v| v.to_lowercase().contains(needle));
            if profile_match {
                return true;
            }
            fixtures.profile_extras.get(&user.id).is_some_and(|extra| {
                extra
                    .pronouns
                    .as_deref()
                    .is_some_and(|p| p.to_lowercase().contains(needle))
                    || extra.fields.iter().any(|(k, v)| {
                        k.to_lowercase().contains(needle) || v.to_lowercase().contains(needle)
                    })
            })
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

/// Channels matching `needle` (name or topic, case-insensitive), sorted by name.
/// Shared by both [`crate::directory::DirectoryView`] and [`crate::browser::ChannelBrowser`].
pub(crate) fn filter_channels(fixtures: &Fixtures, needle: &str) -> Vec<Conversation> {
    let needle = needle.trim().to_lowercase();
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
                    .contains(&needle)
                || channel
                    .topic
                    .as_ref()
                    .is_some_and(|t| t.value.to_lowercase().contains(&needle))
        })
        .cloned()
        .collect();
    channels.sort_by(|a, b| a.name.cmp(&b.name));
    channels
}

/// Channels matching `needle` (name or topic, case-insensitive), by name.
pub(super) fn channels(fixtures: &Fixtures, needle: &str) -> Vec<Conversation> {
    filter_channels(fixtures, needle)
}

/// Parameters and visual layout variant for [`render_channel_row`].
pub(crate) struct ChannelRowSpec {
    pub row_id: ElementId,
    pub join_id: SharedString,
    pub is_member: bool,
    /// Place the channel icon inline next to the name (`ChannelBrowser`) instead of as a leading
    /// column (`DirectoryView`).
    pub inline_icon: bool,
    /// Use `Size::Small` on the Join button (`DirectoryView`).
    pub compact_join: bool,
}

/// Shared channel-list row renderer used by both [`crate::browser::ChannelBrowser`] and
/// [`crate::directory::DirectoryView`].
pub(crate) fn render_channel_row(
    channel: &Conversation,
    spec: ChannelRowSpec,
    on_join: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_open: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (border, muted, foreground, success, hover) = (
        theme.border,
        theme.muted_foreground,
        theme.foreground,
        theme.success,
        theme.accent,
    );
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
            .map_or_else(String::new, |t| format!(" \u{b7} {}", t.value))
    );
    let action = if spec.is_member {
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
        let btn = Button::new(spec.join_id).primary().label("Join");
        let btn = if spec.compact_join {
            btn.with_size(Size::Small)
        } else {
            btn
        };
        btn.on_click(on_join).into_any_element()
    };

    let name = channel.name.clone().unwrap_or_default();
    let mut row = h_flex()
        .id(spec.row_id)
        .items_center()
        .gap_3()
        .px_4()
        .py(if spec.inline_icon { px(10.) } else { px(8.) })
        .border_b_1()
        .border_color(border)
        .cursor_pointer()
        .hover(move |row| row.bg(hover));

    if spec.inline_icon {
        row = row.child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    h_flex()
                        .items_center()
                        .gap_1()
                        .text_color(foreground)
                        .child(Icon::new(icon).text_color(foreground).size(px(14.)))
                        .child(div().truncate().font_weight(FontWeight::BOLD).child(name)),
                )
                .child(div().truncate().text_xs().text_color(muted).child(meta)),
        );
    } else {
        row = row
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
                            .child(name),
                    )
                    .child(div().truncate().text_xs().text_color(muted).child(meta)),
            );
    }

    row.child(action).on_click(on_open).into_any_element()
}

/// Active user groups matching `needle` (handle, name, or description), by handle.
pub(super) fn usergroups(fixtures: &Fixtures, needle: &str) -> Vec<Usergroup> {
    let mut groups: Vec<Usergroup> = fixtures
        .usergroups
        .iter()
        .filter(|g| g.date_delete == 0)
        .filter(|g| {
            needle.is_empty()
                || g.handle.to_lowercase().contains(needle)
                || g.name.to_lowercase().contains(needle)
                || g.description
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(needle)
        })
        .cloned()
        .collect();
    groups.sort_by_key(|a| a.handle.to_lowercase());
    groups
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
    let id = channel.id.clone();
    let join_id = channel.id.clone();
    let member = channel.is_member || joined.contains(&channel.id);
    render_channel_row(
        channel,
        ChannelRowSpec {
            row_id: ("directory-channel", ix).into(),
            join_id: SharedString::from(format!("directory-join-{id}")),
            is_member: member,
            inline_icon: false,
            compact_join: true,
        },
        cx.listener(move |this, _, _, cx| {
            this.join(join_id.clone(), cx);
        }),
        cx.listener(move |_, _, _, cx| cx.emit(DirectoryEvent::Open(id.clone()))),
        cx,
    )
}

/// One user group row: `@handle`, name, member count, description, and member names preview.
pub(super) fn usergroup_row(
    view: &DirectoryView,
    ix: usize,
    group: &Usergroup,
    cx: &mut Context<DirectoryView>,
) -> AnyElement {
    let fixtures = view.fixtures();
    let theme = cx.theme();
    let (border, muted, foreground, link, hover) = (
        theme.border,
        theme.muted_foreground,
        theme.foreground,
        theme.link,
        theme.accent,
    );
    let user_ids: Vec<UserId> = group
        .extra
        .get("users")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(UserId::from)
                .collect()
        })
        .unwrap_or_default();
    let count = group.user_count.unwrap_or(0).max(user_ids.len() as u32);
    let members_preview = if user_ids.is_empty() {
        String::new()
    } else {
        let names: Vec<&str> = user_ids
            .iter()
            .take(5)
            .map(|id| fixtures.user_name(id))
            .collect();
        let extra = user_ids.len().saturating_sub(names.len());
        if extra > 0 {
            format!(" · {}, +{extra} more", names.join(", "))
        } else {
            format!(" · {}", names.join(", "))
        }
    };
    let desc = group.description.as_deref().unwrap_or_default().trim();
    let subtitle = if desc.is_empty() {
        format!("{count} members{members_preview}")
    } else {
        format!("{desc} · {count} members{members_preview}")
    };
    let default_channels: Vec<ChannelId> = group
        .extra
        .get("prefs")
        .and_then(|p| p.get("channels"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(ChannelId::from)
                .collect()
        })
        .unwrap_or_default();
    let expanded = view.is_group_expanded(group.id.as_str());
    let toggle_id = group.id.to_string();
    let chip_bg = theme.secondary;
    let chip_hover = theme.secondary_hover;
    let details = expanded.then(|| {
        let mut panel = v_flex().pl(px(56.)).pr_4().pb_3().gap_2();
        if !user_ids.is_empty() {
            let member_chips =
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1p5()
                    .children(user_ids.iter().enumerate().map(|(m_ix, uid)| {
                        let open_uid = uid.clone();
                        let label = fixtures.user_name(uid).to_owned();
                        h_flex()
                            .id(("ug-member", ix * 1000 + m_ix))
                            .items_center()
                            .gap_1()
                            .px_2()
                            .py_0p5()
                            .rounded_full()
                            .bg(chip_bg)
                            .hover(|el| el.bg(chip_hover))
                            .cursor_pointer()
                            .text_xs()
                            .text_color(foreground)
                            .child(user_chip(fixtures, uid, 16.))
                            .child(label)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(DirectoryEvent::OpenProfile(open_uid.clone()));
                            }))
                    }));
            panel = panel
                .child(div().text_xs().text_color(muted).child("Members"))
                .child(member_chips);
        }
        if !default_channels.is_empty() {
            let ch_chips = div().flex().flex_wrap().gap_1p5().children(
                default_channels.iter().enumerate().map(|(c_ix, ch_id)| {
                    let open_ch = ch_id.clone();
                    let label = fixtures
                        .conversation(ch_id)
                        .and_then(|c| c.name.as_deref())
                        .map(|n| format!("#{n}"))
                        .unwrap_or_else(|| ch_id.to_string());
                    div()
                        .id(("ug-channel", ix * 1000 + c_ix))
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .bg(chip_bg)
                        .hover(|el| el.bg(chip_hover))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(link)
                        .child(label)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(DirectoryEvent::Open(open_ch.clone()));
                        }))
                }),
            );
            panel = panel
                .child(div().text_xs().text_color(muted).child("Default channels"))
                .child(ch_chips);
        }
        if user_ids.is_empty() && default_channels.is_empty() {
            panel = panel.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("No member list available for this group."),
            );
        }
        panel
    });
    v_flex()
        .border_b_1()
        .border_color(border)
        .child(
            h_flex()
                .id(("directory-usergroup", ix))
                .items_center()
                .gap_3()
                .px_4()
                .py_2()
                .cursor_pointer()
                .hover(|row| row.bg(hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_group(toggle_id.clone(), cx);
                }))
                .child(
                    div()
                        .size(px(32.))
                        .flex_none()
                        .rounded_md()
                        .bg(link.opacity(0.14))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::Users).text_color(link).size(px(16.))),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_baseline()
                                .child(
                                    div()
                                        .truncate()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(foreground)
                                        .child(group.name.clone()),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(link)
                                        .child(format!("@{}", group.handle)),
                                ),
                        )
                        .child(div().truncate().text_xs().text_color(muted).child(subtitle)),
                )
                .child(
                    Icon::new(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(px(14.))
                    .text_color(muted),
                ),
        )
        .children(details)
        .into_any_element()
}
