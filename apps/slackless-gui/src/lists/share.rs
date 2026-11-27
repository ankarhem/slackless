//! List Share & Access Management dialog/drawer (`F-COLLAB-243..247`, `F-COLLAB-254`,
//! `F-COLLAB-274..279`): tabbed sharing panel (`Share`, `Access`, `Settings`) wired to
//! `slackLists.access.set` and `slackLists.access.delete`, with silent sharing, limit-sharing
//! toggle, item-grants-list-access notice, and server-enforced policy guidance.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, ClipboardItem, Div, Focusable as _, FontWeight, InteractiveElement as _,
    ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    Window, div, px,
};
use slackless_core::api::{ListAccessLevel, ListMeta, format_list_error};
pub use slackless_core::domain::lists::{
    ListShareAccessRequest, ListShareTab, build_list_share_request, list_general_access_summary,
};
use slackless_core::types::{ChannelId, UserId};

use super::{ListActions, ListPanelState, list_meta, mutate_panel};
use crate::fixtures::Fixtures;

/// Callback invoked when granting/updating list access (`CollabCommand::SetListAccess`).
pub type ListAccessSetFn = Rc<dyn Fn(ListShareAccessRequest, &mut Window, &mut App)>;
/// Callback invoked when revoking list access (`CollabCommand::DeleteListAccess`).
pub type ListAccessDeleteFn = Rc<dyn Fn(Vec<ChannelId>, Vec<UserId>, &mut Window, &mut App)>;

/// Open the List Share panel for `list_key`, initializing the target input if needed.
pub(super) fn open_share_panel(
    list_key: &str,
    tab: ListShareTab,
    item_record_id: Option<String>,
    window: &mut Window,
    cx: &mut App,
) {
    let existing = cx
        .try_global::<super::ListUiStore>()
        .and_then(|store| store.panels.get(list_key))
        .and_then(|p| p.share_target_input.clone());
    let input = existing.unwrap_or_else(|| {
        cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("#C01ENG, @U01ALICE…")
                .auto_grow(1, 2)
        })
    });
    let focus_input = (tab == ListShareTab::Share).then(|| input.clone());
    mutate_panel(
        list_key,
        |s| {
            s.share_open = true;
            s.share_tab = tab;
            s.share_item_id = item_record_id;
            s.share_status = None;
            s.link_copied = false;
            if s.share_target_input.is_none() {
                s.share_target_input = Some(input);
            }
        },
        window,
        cx,
    );
    if let Some(input) = focus_input {
        input.read(cx).focus_handle(cx).focus(window, cx);
    }
}

/// Render the tabbed List Share & Access Management dialog drawer (`F-COLLAB-274..279`).
pub(super) fn render_list_share_panel(
    list_key: &str,
    list_title: &str,
    row_count: usize,
    fixtures: &Fixtures,
    state: &ListPanelState,
    actions: &ListActions,
    cx: &App,
) -> Option<Stateful<Div>> {
    if !state.share_open {
        return None;
    }
    let theme = cx.theme();
    let meta: Option<ListMeta> = list_meta(list_key, cx);
    let is_fixture = list_key == "fixture" || fixtures.sample;
    let effective_link = meta
        .as_ref()
        .and_then(|m| m.permalink.as_deref())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let team = fixtures
                .own_team()
                .map(|t| t.id.as_str())
                .unwrap_or("T0SAMPLE");
            let domain = fixtures
                .own_team()
                .map(|t| t.domain.as_str())
                .filter(|d| !d.is_empty())
                .unwrap_or("slackless");
            format!("https://{domain}.slack.com/lists/{team}/{list_key}")
        });

    let status_banner = state.share_status.clone().map(|(is_err, msg)| {
        let bg = if is_err {
            theme.danger.opacity(0.14)
        } else {
            theme.success.opacity(0.14)
        };
        let fg = if is_err { theme.danger } else { theme.success };
        let icon = if is_err {
            IconName::TriangleAlert
        } else {
            IconName::Check
        };
        h_flex()
            .items_start()
            .gap_1p5()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(fg.opacity(0.4))
            .bg(bg)
            .text_xs()
            .text_color(theme.foreground)
            .child(Icon::new(icon).flex_none().size(px(14.)).text_color(fg))
            .child(div().flex_1().min_w_0().child(format_list_error(&msg)))
    });

    let tab_bar = {
        let tabs = [
            (ListShareTab::Share, "Share"),
            (ListShareTab::Access, "Access"),
            (ListShareTab::Settings, "Settings"),
        ];
        h_flex()
            .items_center()
            .gap_1()
            .pb_2()
            .border_b_1()
            .border_color(theme.border)
            .children(tabs.into_iter().map(|(tab, label)| {
                let active = state.share_tab == tab;
                let key = list_key.to_owned();
                Button::new(SharedString::from(format!("list-share-tab-{key}-{label}")))
                    .when(active, |b| b.primary())
                    .when(!active, |b| b.ghost())
                    .with_size(Size::XSmall)
                    .label(label)
                    .on_click(move |_, window, cx| {
                        mutate_panel(&key, |s| s.share_tab = tab, window, cx);
                    })
            }))
    };

    let tab_body = match state.share_tab {
        ListShareTab::Share => render_share_tab(
            list_key,
            &effective_link,
            fixtures,
            state,
            actions,
            is_fixture,
            cx,
        ),
        ListShareTab::Access => render_access_tab(
            list_key,
            meta.as_ref(),
            fixtures,
            state,
            actions,
            is_fixture,
            cx,
        ),
        ListShareTab::Settings => {
            render_settings_tab(list_key, meta.as_ref(), row_count, state, cx)
        }
    };

    let key_copy = list_key.to_owned();
    let key_close = list_key.to_owned();
    let key_esc = list_key.to_owned();
    let link_for_copy = effective_link.clone();
    let header_title = if state.share_item_id.is_some() {
        format!("Share item \u{b7} {list_title}")
    } else {
        format!("Share \"{list_title}\"")
    };

    Some(
        v_flex()
            .id(SharedString::from(format!("list-share-drawer-{list_key}")))
            .w(px(340.))
            .flex_none()
            .h_full()
            .min_h_0()
            .overflow_y_scroll()
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .on_key_down(move |event, window, cx| {
                if event.keystroke.key == "escape" {
                    mutate_panel(&key_esc, |s| s.share_open = false, window, cx);
                    cx.stop_propagation();
                }
            })
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.background)
            .p_3()
            .gap_3()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .min_w_0()
                            .flex_1()
                            .items_center()
                            .gap_1p5()
                            .child(
                                Icon::new(IconName::Users)
                                    .size(px(14.))
                                    .text_color(theme.muted_foreground),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(header_title),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new(SharedString::from(format!(
                                    "list-share-copy-{list_key}"
                                )))
                                .outline()
                                .with_size(Size::XSmall)
                                .icon(if state.link_copied {
                                    IconName::Check
                                } else {
                                    IconName::Link
                                })
                                .label(if state.link_copied {
                                    "Copied"
                                } else {
                                    "Copy link"
                                })
                                .on_click(
                                    move |_, window, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            link_for_copy.clone(),
                                        ));
                                        mutate_panel(
                                            &key_copy,
                                            |s| s.link_copied = true,
                                            window,
                                            cx,
                                        );
                                        crate::actions::notify(
                                            "Link copied",
                                            "List link copied to clipboard.",
                                            window,
                                            cx,
                                        );
                                    },
                                ),
                            )
                            .child(
                                Button::new(SharedString::from(format!(
                                    "list-share-close-{list_key}"
                                )))
                                .ghost()
                                .with_size(Size::XSmall)
                                .icon(IconName::X)
                                .on_click(
                                    move |_, window, cx| {
                                        mutate_panel(
                                            &key_close,
                                            |s| {
                                                s.share_open = false;
                                                s.share_item_id = None;
                                            },
                                            window,
                                            cx,
                                        );
                                    },
                                ),
                            ),
                    ),
            )
            .child(tab_bar)
            .children(status_banner)
            .child(tab_body),
    )
}

fn render_share_tab(
    list_key: &str,
    effective_link: &str,
    fixtures: &Fixtures,
    state: &ListPanelState,
    actions: &ListActions,
    is_fixture: bool,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let access_btn = |tag: &str, label: &str, level: ListAccessLevel| {
        let active = state.share_access_level == level;
        let key = list_key.to_owned();
        Button::new(SharedString::from(format!(
            "list-share-lvl-{list_key}-{tag}"
        )))
        .when(active, |b| b.primary())
        .when(!active, |b| b.outline())
        .with_size(Size::XSmall)
        .label(label.to_owned())
        .on_click(move |_, window, cx| {
            mutate_panel(&key, |s| s.share_access_level = level, window, cx);
        })
    };

    let key_silent = list_key.to_owned();
    let silent_toggle = h_flex()
        .id(SharedString::from(format!("list-share-silent-{list_key}")))
        .items_center()
        .gap_1p5()
        .cursor_pointer()
        .text_xs()
        .text_color(theme.foreground)
        .on_click(move |_, window, cx| {
            mutate_panel(
                &key_silent,
                |s| s.share_without_posting = !s.share_without_posting,
                window,
                cx,
            );
        })
        .child(
            Icon::new(if state.share_without_posting {
                IconName::SquareCheck
            } else {
                IconName::Square
            })
            .size(px(14.))
            .text_color(if state.share_without_posting {
                theme.link
            } else {
                theme.muted_foreground
            }),
        )
        .child("Share without posting a message");

    // Quick-add chips from workspace channels and teammates for fast selection.
    let quick_channels: Vec<(ChannelId, String)> = fixtures
        .channels
        .iter()
        .take(4)
        .map(|c| {
            (
                c.id.clone(),
                c.name
                    .as_deref()
                    .map(|n| format!("#{n}"))
                    .unwrap_or_else(|| c.id.to_string()),
            )
        })
        .collect();
    let quick_users: Vec<(UserId, String)> = fixtures
        .people()
        .filter(|u| !u.deleted && !u.is_bot && u.id != fixtures.self_user)
        .take(4)
        .map(|u| (u.id.clone(), format!("@{}", u.display_name())))
        .collect();

    let target_input = state.share_target_input.clone();
    let quick_chips = target_input.as_ref().map(|inp| {
        let mut chips = h_flex().flex_wrap().gap_1();
        for (ix, (cid, label)) in quick_channels.into_iter().enumerate() {
            let inp = inp.clone();
            let token = format!("#{}", cid.as_str());
            chips = chips.child(
                Button::new(SharedString::from(format!(
                    "list-share-qch-{list_key}-{ix}"
                )))
                .outline()
                .with_size(Size::XSmall)
                .label(label)
                .on_click(move |_, window, cx| {
                    inp.update(cx, |st, cx| {
                        let cur = st.value().trim().to_owned();
                        let next = if cur.is_empty() {
                            token.clone()
                        } else if cur.contains(&token) {
                            cur
                        } else {
                            format!("{cur}, {token}")
                        };
                        st.set_value(next, window, cx);
                    });
                }),
            );
        }
        for (ix, (uid, label)) in quick_users.into_iter().enumerate() {
            let inp = inp.clone();
            let token = format!("@{}", uid.as_str());
            chips = chips.child(
                Button::new(SharedString::from(format!(
                    "list-share-qusr-{list_key}-{ix}"
                )))
                .outline()
                .with_size(Size::XSmall)
                .label(label)
                .on_click(move |_, window, cx| {
                    inp.update(cx, |st, cx| {
                        let cur = st.value().trim().to_owned();
                        let next = if cur.is_empty() {
                            token.clone()
                        } else if cur.contains(&token) {
                            cur
                        } else {
                            format!("{cur}, {token}")
                        };
                        st.set_value(next, window, cx);
                    });
                }),
            );
        }
        chips
    });

    let on_set_access = actions.on_set_access.clone();
    let level_cur = state.share_access_level;
    let silent_cur = state.share_without_posting;
    let item_cur = state.share_item_id.clone();
    let link_for_share = effective_link.to_owned();
    let k_submit = list_key.to_owned();

    v_flex()
        .gap_2p5()
        .when(state.share_item_id.is_some(), |col| {
            col.child(
                h_flex()
                    .items_start()
                    .gap_1p5()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.link.opacity(0.35))
                    .bg(theme.link.opacity(0.08))
                    .text_xs()
                    .text_color(theme.foreground)
                    .child(
                        Icon::new(IconName::Info)
                            .size(px(14.))
                            .text_color(theme.link),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child("Sharing an item grants access to the entire list."),
                    ),
            )
        })
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child("Share with people or channels"),
        )
        .when_some(target_input.clone(), |col, input| {
            col.child(crate::preferences::text_input_box(&input, true, cx).bg(theme.background))
        })
        .children(quick_chips)
        .child(
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Access level"),
                )
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .child(access_btn("read", "Can view", ListAccessLevel::Read))
                        .child(access_btn("write", "Can edit", ListAccessLevel::Write))
                        .child(access_btn("owner", "Owner", ListAccessLevel::Owner)),
                ),
        )
        .child(div().text_xs().text_color(theme.muted_foreground).child(
            match state.share_access_level {
                ListAccessLevel::Read => "Viewers can read this list and comment on item threads.",
                ListAccessLevel::Write => "Editors can add, edit, archive, and comment on items.",
                ListAccessLevel::Owner => {
                    "Owners can edit list structure, fields, and sharing settings."
                }
            },
        ))
        .child(silent_toggle)
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Linked lists maintain separate permissions from this list."),
        )
        .child(
            h_flex().justify_end().child(
                Button::new(SharedString::from(format!("list-share-submit-{list_key}")))
                    .primary()
                    .with_size(Size::XSmall)
                    .icon(IconName::UserPlus)
                    .label("Share")
                    .on_click(move |_, window, cx| {
                        let raw = target_input
                            .as_ref()
                            .map(|inp| inp.read(cx).value().to_string())
                            .unwrap_or_default();
                        match build_list_share_request(
                            &raw,
                            level_cur,
                            silent_cur,
                            Some(&link_for_share),
                            item_cur.as_deref(),
                        ) {
                            Ok(req) => {
                                if let Some(inp) = &target_input {
                                    inp.update(cx, |st, cx| {
                                        st.set_value(String::new(), window, cx)
                                    });
                                }
                                if let Some(cb) = &on_set_access {
                                    cb(req.clone(), window, cx);
                                }
                                let channels_added = req.channel_ids.clone();
                                let users_added = req.user_ids.clone();
                                let summary = format!(
                                    "Shared ({}) with {} target(s){}.",
                                    req.access_level.as_str(),
                                    req.channel_ids.len() + req.user_ids.len(),
                                    if req.post_permalink.is_some() {
                                        " and posted link"
                                    } else {
                                        " without posting a message"
                                    }
                                );
                                if is_fixture || on_set_access.is_none() {
                                    mutate_panel(
                                        &k_submit,
                                        |s| {
                                            for ch in channels_added {
                                                if !s.share_local_channels.contains(&ch) {
                                                    s.share_local_channels.push(ch);
                                                }
                                            }
                                            for u in users_added {
                                                if !s.share_local_editors.contains(&u) {
                                                    s.share_local_editors.push(u);
                                                }
                                            }
                                            s.share_status = Some((false, summary));
                                        },
                                        window,
                                        cx,
                                    );
                                }
                            }
                            Err(msg) => {
                                mutate_panel(
                                    &k_submit,
                                    |s| s.share_status = Some((true, msg)),
                                    window,
                                    cx,
                                );
                            }
                        }
                    }),
            ),
        )
}

fn render_access_tab(
    list_key: &str,
    meta: Option<&ListMeta>,
    fixtures: &Fixtures,
    state: &ListPanelState,
    actions: &ListActions,
    is_fixture: bool,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let (gen_badge, gen_desc) = list_general_access_summary(
        meta,
        (!state.share_general_access.is_empty()).then_some(state.share_general_access.as_str()),
        state.share_limit_sharing,
    );
    let gen_btn = |tag: &str, label: &'static str, value: &'static str| {
        let active = gen_badge.eq_ignore_ascii_case(label);
        let k = list_key.to_owned();
        Button::new(SharedString::from(format!("list-gen-{list_key}-{tag}")))
            .when(active, |b| b.primary())
            .when(!active, |b| b.outline())
            .with_size(Size::XSmall)
            .label(label)
            .on_click(move |_, window, cx| {
                mutate_panel(
                    &k,
                    |s| {
                        s.share_general_access = value.to_owned();
                        s.share_status = Some((false, format!("General access set to {label}.")));
                    },
                    window,
                    cx,
                );
            })
    };

    let owner_id = meta
        .and_then(|m| m.owner_id.clone())
        .or_else(|| is_fixture.then(|| fixtures.self_user.clone()));
    let mut editors: Vec<UserId> = meta.map(|m| m.editors.clone()).unwrap_or_default();
    for u in &state.share_local_editors {
        if !editors.contains(u) {
            editors.push(u.clone());
        }
    }
    if editors.is_empty()
        && is_fixture
        && let Some(u) = fixtures
            .people()
            .find(|u| !u.deleted && !u.is_bot && u.id != fixtures.self_user)
    {
        editors.push(u.id.clone());
    }

    let mut channels: Vec<ChannelId> = meta.map(|m| m.shared_channels.clone()).unwrap_or_default();
    for ch in &state.share_local_channels {
        if !channels.contains(ch) {
            channels.push(ch.clone());
        }
    }
    if channels.is_empty()
        && is_fixture
        && let Some(ch) = fixtures.channels.first()
    {
        channels.push(ch.id.clone());
    }

    let mut people_col = v_flex()
        .gap_1p5()
        .pt_2()
        .border_t_1()
        .border_color(theme.border)
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child("Owners & editors"),
        );

    if let Some(owner) = owner_id {
        let owner_name = fixtures.user_name(&owner).to_owned();
        people_col = people_col.child(
            h_flex()
                .items_center()
                .justify_between()
                .gap_1()
                .p_1p5()
                .rounded_sm()
                .bg(theme.secondary.opacity(0.3))
                .text_xs()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .child(owner_name),
                )
                .child(
                    div()
                        .flex_none()
                        .px_1p5()
                        .py_0p5()
                        .rounded_sm()
                        .bg(theme.link.opacity(0.16))
                        .text_color(theme.link)
                        .child("Owner"),
                ),
        );
    }

    for (ix, uid) in editors.into_iter().enumerate() {
        let name = fixtures.user_name(&uid).to_owned();
        let on_set = actions.on_set_access.clone();
        let on_del = actions.on_delete_access.clone();
        let uid_owner = uid.clone();
        let uid_del = uid.clone();
        let k_owner = list_key.to_owned();
        let k_del = list_key.to_owned();
        people_col = people_col.child(
            h_flex()
                .items_center()
                .justify_between()
                .gap_1()
                .p_1p5()
                .rounded_sm()
                .border_1()
                .border_color(theme.border)
                .text_xs()
                .child(div().flex_1().min_w_0().truncate().child(name))
                .child(
                    Button::new(SharedString::from(format!(
                        "list-share-promote-{list_key}-{ix}"
                    )))
                    .ghost()
                    .with_size(Size::XSmall)
                    .label("Owner")
                    .tooltip("Grant Owner role")
                    .on_click(move |_, window, cx| {
                        let req = ListShareAccessRequest {
                            access_level: ListAccessLevel::Owner,
                            channel_ids: Vec::new(),
                            user_ids: vec![uid_owner.clone()],
                            post_permalink: None,
                            item_record_id: None,
                        };
                        if let Some(cb) = &on_set {
                            cb(req, window, cx);
                        } else {
                            let uid_s = uid_owner.as_str().to_owned();
                            mutate_panel(
                                &k_owner,
                                |s| {
                                    s.share_status =
                                        Some((false, format!("Granted Owner role to {uid_s}.")));
                                },
                                window,
                                cx,
                            );
                        }
                    }),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "list-share-rm-usr-{list_key}-{ix}"
                    )))
                    .ghost()
                    .with_size(Size::XSmall)
                    .icon(IconName::X)
                    .tooltip("Remove access")
                    .on_click(move |_, window, cx| {
                        if let Some(cb) = &on_del {
                            cb(Vec::new(), vec![uid_del.clone()], window, cx);
                        }
                        if is_fixture || on_del.is_none() {
                            let target = uid_del.clone();
                            mutate_panel(
                                &k_del,
                                |s| {
                                    s.share_local_editors.retain(|u| u != &target);
                                    s.share_status = Some((
                                        false,
                                        format!("Removed access for {}.", target.as_str()),
                                    ));
                                },
                                window,
                                cx,
                            );
                        }
                    }),
                ),
        );
    }

    let mut channels_col = v_flex()
        .gap_1p5()
        .pt_2()
        .border_t_1()
        .border_color(theme.border)
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child("Shared channels"),
        );

    if channels.is_empty() {
        channels_col = channels_col.child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Not shared with any channels yet."),
        );
    } else {
        for (ix, cid) in channels.into_iter().enumerate() {
            let label = fixtures
                .conversation(&cid)
                .map(|c| fixtures.conversation_label(c))
                .unwrap_or_else(|| cid.to_string());
            let on_del = actions.on_delete_access.clone();
            let cid_del = cid.clone();
            let k_del = list_key.to_owned();
            channels_col = channels_col.child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .p_1p5()
                    .rounded_sm()
                    .border_1()
                    .border_color(theme.border)
                    .text_xs()
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(
                        Button::new(SharedString::from(format!(
                            "list-share-rm-ch-{list_key}-{ix}"
                        )))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::X)
                        .tooltip("Remove channel access")
                        .on_click(move |_, window, cx| {
                            if let Some(cb) = &on_del {
                                cb(vec![cid_del.clone()], Vec::new(), window, cx);
                            }
                            if is_fixture || on_del.is_none() {
                                let target = cid_del.clone();
                                mutate_panel(
                                    &k_del,
                                    |s| {
                                        s.share_local_channels.retain(|c| c != &target);
                                        s.share_status = Some((
                                            false,
                                            format!(
                                                "Revoked channel access for {}.",
                                                target.as_str()
                                            ),
                                        ));
                                    },
                                    window,
                                    cx,
                                );
                            }
                        }),
                    ),
            );
        }
    }

    v_flex()
        .gap_2p5()
        .child(
            v_flex()
                .gap_1p5()
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child("General access"),
                        )
                        .child(
                            div()
                                .px_1p5()
                                .py_0p5()
                                .rounded_sm()
                                .bg(theme.secondary.opacity(0.6))
                                .text_xs()
                                .text_color(theme.foreground)
                                .child(gen_badge),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(gen_desc),
                )
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .child(gen_btn("invite", "Invite only", "private"))
                        .child(gen_btn("view", "Workspace can view", "read"))
                        .child(gen_btn("edit", "Workspace can edit", "write")),
                ),
        )
        .child(people_col)
        .child(channels_col)
}

fn render_settings_tab(
    list_key: &str,
    meta: Option<&ListMeta>,
    row_count: usize,
    state: &ListPanelState,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let k_limit = list_key.to_owned();
    let active_rows = meta.and_then(|m| m.row_count).unwrap_or(row_count as u32);
    let limit_line = match meta.and_then(|m| m.row_count_limit) {
        Some(limit) => format!("{active_rows} / {limit} active items"),
        None => format!("{active_rows} active items"),
    };
    let archived_note = meta
        .and_then(|m| m.archived_row_count)
        .filter(|&n| n > 0)
        .map(|n| format!(" ({n} archived)"));
    let over_max = meta.is_some_and(|m| m.over_row_maximum);

    v_flex()
        .gap_3()
        .child(
            v_flex()
                .gap_1p5()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child("Sharing restrictions"),
                )
                .child(
                    h_flex()
                        .id(SharedString::from(format!("list-limit-share-{list_key}")))
                        .items_center()
                        .gap_1p5()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(theme.foreground)
                        .on_click(move |_, window, cx| {
                            mutate_panel(
                                &k_limit,
                                |s| {
                                    s.share_limit_sharing = !s.share_limit_sharing;
                                    let msg = if s.share_limit_sharing {
                                        "Sharing limited to list owners only.".to_owned()
                                    } else {
                                        "Editors can share this list with others.".to_owned()
                                    };
                                    s.share_status = Some((false, msg));
                                },
                                window,
                                cx,
                            );
                        })
                        .child(
                            Icon::new(if state.share_limit_sharing {
                                IconName::SquareCheck
                            } else {
                                IconName::Square
                            })
                            .size(px(14.))
                            .text_color(
                                if state.share_limit_sharing {
                                    theme.link
                                } else {
                                    theme.muted_foreground
                                },
                            ),
                        )
                        .child("Limit sharing (owners only)"),
                )
                .child(div().text_xs().text_color(theme.muted_foreground).child(
                    if state.share_limit_sharing {
                        "Only list owners can change permissions or invite new channels and people."
                    } else {
                        "People with edit access can share this list with others in your workspace."
                    },
                )),
        )
        .child(
            v_flex()
                .gap_1p5()
                .pt_2()
                .border_t_1()
                .border_color(theme.border)
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child("Item limits & archiving"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(if over_max {
                            theme.danger
                        } else {
                            theme.muted_foreground
                        })
                        .child(format!("{limit_line}{}", archived_note.unwrap_or_default())),
                )
                .when(over_max, |col| {
                    col.child(
                        div()
                            .text_xs()
                            .text_color(theme.danger)
                            .child("Item maximum reached — archive completed rows to free space."),
                    )
                }),
        )
        .child(
            v_flex()
                .gap_1p5()
                .pt_2()
                .border_t_1()
                .border_color(theme.border)
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child("Permissions & workspace policy"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("\u{2022} People with view access can comment on items."),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("\u{2022} Only list owners can modify fields and list structure."),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("\u{2022} Sharing a single item grants access to the entire list."),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("\u{2022} Linked lists keep separate permissions."),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("\u{2022} Lists are available on paid Slack plans."),
                ),
        )
}
