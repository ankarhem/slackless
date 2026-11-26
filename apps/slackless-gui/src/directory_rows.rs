//! The directory's row lists and filters: the people matching a search (by name or title),
//! the channels matching one (by name or topic), and the user groups matching one (active and
//! disabled, with Members / Edit / Disable / Enable actions), plus their rows. The view entity
//! and tabs live in [`crate::directory`].

use std::collections::{HashMap, HashSet};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, ClickEvent, Context, ElementId, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, px,
};
use slackless_core::types::{
    ChannelId, Conversation, ConversationKind, UserId, Usergroup, UsergroupId,
};

use crate::avatars::user_chip;
use crate::directory::{
    DirectoryEvent, DirectoryView, UsergroupAction, UsergroupLocalEdit, cached_usergroup_members,
    dispatch_usergroup_action,
};
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
    people.sort_by_cached_key(|id| fixtures.user_name(id).to_lowercase());
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

/// Extract embedded member IDs from `Usergroup.extra["users"]` when present.
pub(crate) fn usergroup_embedded_users(group: &Usergroup) -> Vec<UserId> {
    group
        .extra
        .get("users")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(UserId::from)
                .collect()
        })
        .unwrap_or_default()
}

fn matches_usergroup_needle(group: &Usergroup, needle: &str) -> bool {
    needle.is_empty()
        || group.handle.to_lowercase().contains(needle)
        || group.name.to_lowercase().contains(needle)
        || group
            .description
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
            .contains(needle)
}

/// User groups matching `needle` (handle, name, or description), merging any session-local
/// disabled/edit overrides. Returns `(visible_groups, total_matching_disabled_count)` sorted with
/// active groups first and then alphabetically by handle.
pub(super) fn usergroups(
    fixtures: &Fixtures,
    disabled_overrides: &HashMap<UsergroupId, Usergroup>,
    edit_overrides: &HashMap<UsergroupId, UsergroupLocalEdit>,
    needle: &str,
    include_disabled: bool,
) -> (Vec<Usergroup>, usize) {
    let mut seen: HashSet<UsergroupId> = HashSet::new();
    let mut merged: Vec<Usergroup> = Vec::with_capacity(
        fixtures
            .usergroups
            .len()
            .saturating_add(disabled_overrides.len()),
    );

    for raw in &fixtures.usergroups {
        seen.insert(raw.id.clone());
        let mut group = raw.clone();
        if disabled_overrides.contains_key(&group.id) && group.date_delete == 0 {
            group.date_delete = 1;
        }
        if let Some(edit) = edit_overrides.get(&group.id) {
            group.name = edit.name.clone();
            if !edit.handle.is_empty() {
                group.handle = edit.handle.clone();
            }
            group.description = edit.description.clone();
        }
        merged.push(group);
    }

    for (id, disabled) in disabled_overrides {
        if !seen.contains(id) {
            let mut group = disabled.clone();
            if group.date_delete == 0 {
                group.date_delete = 1;
            }
            if let Some(edit) = edit_overrides.get(id) {
                group.name = edit.name.clone();
                if !edit.handle.is_empty() {
                    group.handle = edit.handle.clone();
                }
                group.description = edit.description.clone();
            }
            merged.push(group);
        }
    }

    let mut disabled_count = 0usize;
    let mut groups: Vec<Usergroup> = Vec::new();
    for group in merged {
        if !matches_usergroup_needle(&group, needle) {
            continue;
        }
        let is_disabled = group.is_deleted();
        if is_disabled {
            disabled_count += 1;
            if !include_disabled {
                continue;
            }
        }
        groups.push(group);
    }

    groups.sort_by(|a, b| {
        a.is_deleted()
            .cmp(&b.is_deleted())
            .then_with(|| a.handle.to_lowercase().cmp(&b.handle.to_lowercase()))
    });
    (groups, disabled_count)
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

/// One user group row: `@handle`, name, active/disabled status pill, member count, description,
/// inline action buttons (`Members`, `Edit`, `Disable` / `Enable`), and expandable member/channel
/// chips.
pub(super) fn usergroup_row(
    view: &DirectoryView,
    ix: usize,
    group: &Usergroup,
    cx: &mut Context<DirectoryView>,
) -> AnyElement {
    let fixtures = view.fixtures();
    let theme = cx.theme();
    let (border, muted, foreground, link, hover, success, warning) = (
        theme.border,
        theme.muted_foreground,
        theme.foreground,
        theme.link,
        theme.accent,
        theme.success,
        theme.warning,
    );
    let disabled = group.is_deleted();
    let cached_users = cached_usergroup_members(&group.id, cx);
    let user_ids: Vec<UserId> = cached_users
        .clone()
        .unwrap_or_else(|| usergroup_embedded_users(group));
    let count = if cached_users.is_some() {
        user_ids.len() as u32
    } else {
        group.user_count.unwrap_or(0).max(user_ids.len() as u32)
    };
    let member_word = if count == 1 { "member" } else { "members" };
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
        format!("{count} {member_word}{members_preview}")
    } else {
        format!("{desc} · {count} {member_word}{members_preview}")
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

    let group_for_members = group.clone();
    let group_for_edit = group.clone();
    let group_for_toggle = group.clone();

    let status_pill = div()
        .flex_none()
        .px_1p5()
        .py_0p5()
        .rounded_full()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .bg(if disabled {
            warning.opacity(0.16)
        } else {
            success.opacity(0.14)
        })
        .text_color(if disabled { warning } else { success })
        .child(if disabled { "Disabled" } else { "Active" });

    let actions = h_flex()
        .flex_none()
        .items_center()
        .gap_1()
        .child(
            Button::new(SharedString::from(format!("ug-members-{}", group.id)))
                .ghost()
                .with_size(Size::XSmall)
                .icon(IconName::Users)
                .label("Members")
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    dispatch_usergroup_action(
                        UsergroupAction::InspectMembers(group_for_members.clone()),
                        window,
                        cx,
                    );
                }),
        )
        .child(
            Button::new(SharedString::from(format!("ug-edit-{}", group.id)))
                .ghost()
                .with_size(Size::XSmall)
                .icon(IconName::Pencil)
                .label("Edit")
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    dispatch_usergroup_action(
                        UsergroupAction::Edit(group_for_edit.clone()),
                        window,
                        cx,
                    );
                }),
        )
        .child(if disabled {
            Button::new(SharedString::from(format!("ug-enable-{}", group.id)))
                .outline()
                .with_size(Size::XSmall)
                .icon(IconName::RotateCcw)
                .label("Enable")
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    dispatch_usergroup_action(
                        UsergroupAction::Enable(group_for_toggle.clone()),
                        window,
                        cx,
                    );
                })
        } else {
            Button::new(SharedString::from(format!("ug-disable-{}", group.id)))
                .ghost()
                .with_size(Size::XSmall)
                .label("Disable")
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    dispatch_usergroup_action(
                        UsergroupAction::Disable(group_for_toggle.clone()),
                        window,
                        cx,
                    );
                })
        });

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
                    .child("Click Members to load or manage this group's member list."),
            );
        }
        panel
    });
    v_flex()
        .border_b_1()
        .border_color(border)
        .when(disabled, |row| row.opacity(0.7))
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
                        .bg(if disabled {
                            muted.opacity(0.14)
                        } else {
                            link.opacity(0.14)
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::Users)
                                .text_color(if disabled { muted } else { link })
                                .size(px(16.)),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .truncate()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(if disabled { muted } else { foreground })
                                        .child(group.name.clone()),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(if disabled { muted } else { link })
                                        .child(format!("@{}", group.handle)),
                                )
                                .child(status_pill),
                        )
                        .child(div().truncate().text_xs().text_color(muted).child(subtitle)),
                )
                .child(actions)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_group(id: &str, name: &str, handle: &str, date_delete: i64) -> Usergroup {
        Usergroup {
            id: UsergroupId::from(id),
            name: name.to_owned(),
            handle: handle.to_owned(),
            description: Some(format!("{name} team")),
            user_count: Some(2),
            date_delete,
            ..Default::default()
        }
    }

    #[test]
    fn usergroups_filters_disabled_and_sorts_active_first() {
        let mut fx = crate::fixtures::load();
        fx.usergroups = vec![
            sample_group("S2", "Platform", "platform", 0),
            sample_group("S1", "Alerts", "alerts", 1_700_000_000),
            sample_group("S3", "Design", "design", 0),
        ];

        let empty_disabled = HashMap::new();
        let empty_edits = HashMap::new();

        // Active-only by default, but still reports total disabled count.
        let (active_only, disabled_count) =
            usergroups(&fx, &empty_disabled, &empty_edits, "", false);
        assert_eq!(disabled_count, 1);
        assert_eq!(
            active_only
                .iter()
                .map(|g| g.handle.as_str())
                .collect::<Vec<_>>(),
            vec!["design", "platform"]
        );

        // Including disabled places active groups first, then disabled groups by handle.
        let (with_disabled, disabled_count) =
            usergroups(&fx, &empty_disabled, &empty_edits, "", true);
        assert_eq!(disabled_count, 1);
        assert_eq!(
            with_disabled
                .iter()
                .map(|g| (g.handle.as_str(), g.is_deleted()))
                .collect::<Vec<_>>(),
            vec![("design", false), ("platform", false), ("alerts", true)]
        );

        // Local disable override hides an active group until "Show disabled" is toggled on.
        let mut local_disabled = HashMap::new();
        local_disabled.insert(
            UsergroupId::from("S3"),
            sample_group("S3", "Design", "design", 1),
        );
        let (filtered, disabled_count) = usergroups(&fx, &local_disabled, &empty_edits, "", false);
        assert_eq!(disabled_count, 2);
        assert_eq!(
            filtered
                .iter()
                .map(|g| g.handle.as_str())
                .collect::<Vec<_>>(),
            vec!["platform"]
        );

        // Search needle filters both active and disabled groups cleanly.
        let (searched, disabled_count) =
            usergroups(&fx, &local_disabled, &empty_edits, "des", true);
        assert_eq!(disabled_count, 1);
        assert_eq!(searched.len(), 1);
        assert_eq!(searched[0].id, UsergroupId::from("S3"));
        assert!(searched[0].is_deleted());
    }
}
