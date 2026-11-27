//! Share & Access Management docked panel, Copy canvas link, and server-enforced rule notices
//! (`F-COLLAB-120`, `F-COLLAB-160..174`, `F-COLLAB-204`).

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, Size, h_flex, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, ClipboardItem, Div, FontWeight, InteractiveElement as _, ParentElement as _, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::api::{CanvasAccessLevel, CanvasMeta};
use slackless_core::types::{ChannelId, UserId};

use crate::canvas::{CanvasActions, CanvasUiState, mutate_ui};

#[cfg(test)]
pub use slackless_core::domain::canvas::parse_share_targets;
pub use slackless_core::domain::canvas::{
    ShareAccessRequest, build_share_access_request, general_access_summary,
};

/// Callback invoked when granting/updating canvas access (`Command::SetCanvasAccess`).
pub type CanvasAccessSetFn = Rc<dyn Fn(ShareAccessRequest, &mut Window, &mut App)>;
/// Callback invoked when revoking canvas access (`Command::DeleteCanvasAccess`).
pub type CanvasAccessDeleteFn = Rc<dyn Fn(Vec<ChannelId>, Vec<UserId>, &mut Window, &mut App)>;
/// Callback invoked when deleting a canvas permanently (`Command::DeleteCanvas`, `F-COLLAB-158`).
pub type CanvasDeleteFn = Rc<dyn Fn(&mut Window, &mut App)>;

/// Render the docked Share & Access Management panel (`F-COLLAB-160..174`).
pub(super) fn render_share_panel(
    key: &str,
    meta: Option<&CanvasMeta>,
    canvas_url: Option<&str>,
    ui: &CanvasUiState,
    actions: &CanvasActions,
    is_fixture: bool,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let effective_link = meta
        .and_then(|m| m.permalink.as_deref())
        .filter(|s| !s.is_empty())
        .or(canvas_url)
        .unwrap_or("https://slackless.slack.com/docs/T01SAMPLE/F01CANVAS")
        .to_owned();

    let key_close = key.to_owned();
    let key_copy = key.to_owned();
    let link_for_copy = effective_link.clone();

    let status_banner = actions
        .status_banner
        .as_ref()
        .or(ui.share_status.as_ref())
        .map(|(is_err, msg)| {
            let bg = if *is_err {
                theme.danger.opacity(0.14)
            } else {
                theme.success.opacity(0.14)
            };
            let fg = if *is_err { theme.danger } else { theme.success };
            let icon = if *is_err {
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
                .child(Icon::new(icon).size(px(14.)).text_color(fg))
                .child(div().flex_1().child(msg.clone()))
        });

    let access_level_btn = |id_tag: &str, label: &str, level: CanvasAccessLevel| {
        let active = ui.share_access_level == level;
        let k = key.to_owned();
        Button::new(SharedString::from(format!(
            "canvas-share-lvl-{key}-{id_tag}"
        )))
        .when(active, |b| b.primary())
        .when(!active, |b| b.outline())
        .with_size(Size::XSmall)
        .label(label.to_owned())
        .on_click(move |_, window, cx| {
            mutate_ui(&k, |s| s.share_access_level = level, window, cx);
        })
    };

    let key_silent = key.to_owned();
    let silent_toggle = h_flex()
        .id(SharedString::from(format!("canvas-share-silent-{key}")))
        .items_center()
        .gap_1p5()
        .cursor_pointer()
        .text_xs()
        .text_color(theme.foreground)
        .on_click(move |_, window, cx| {
            mutate_ui(
                &key_silent,
                |s| s.share_without_posting = !s.share_without_posting,
                window,
                cx,
            );
        })
        .child(
            Icon::new(if ui.share_without_posting {
                IconName::SquareCheck
            } else {
                IconName::Square
            })
            .size(px(14.))
            .text_color(if ui.share_without_posting {
                theme.link
            } else {
                theme.muted_foreground
            }),
        )
        .child("Share without posting a message");

    let target_section = {
        let target_input = ui.share_target_input.clone();
        let on_set_access = actions.on_set_access.clone();
        let level_cur = ui.share_access_level;
        let silent_cur = ui.share_without_posting;
        let link_for_share = effective_link.clone();
        let k_submit = key.to_owned();

        v_flex()
            .gap_2()
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
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .child(access_level_btn(
                        "read",
                        "Can view",
                        CanvasAccessLevel::Read,
                    ))
                    .child(access_level_btn(
                        "write",
                        "Can edit",
                        CanvasAccessLevel::Write,
                    ))
                    .child(access_level_btn("owner", "Owner", CanvasAccessLevel::Owner)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Can comment: follows view & edit access in Slack canvases."),
            )
            .child(silent_toggle)
            .child(div().text_xs().text_color(theme.muted_foreground).child(
                "Sharing to a public channel makes this canvas visible to all workspace members.",
            ))
            .child(
                h_flex().justify_end().child(
                    Button::new(SharedString::from(format!("canvas-share-submit-{key}")))
                        .primary()
                        .with_size(Size::XSmall)
                        .icon(IconName::UserPlus)
                        .label("Share")
                        .on_click(move |_, window, cx| {
                            let raw = target_input
                                .as_ref()
                                .map(|inp| inp.read(cx).value().to_string())
                                .unwrap_or_default();
                            match build_share_access_request(
                                &raw,
                                level_cur,
                                silent_cur,
                                Some(&link_for_share),
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
                                    if is_fixture {
                                        let users_added = req.user_ids.clone();
                                        let summary = format!(
                                            "Shared ({}) with {} target(s){}.",
                                            req.access_level.as_str(),
                                            req.channel_ids.len() + req.user_ids.len(),
                                            if req.post_permalink.is_some() {
                                                " and posted link"
                                            } else {
                                                " silently"
                                            }
                                        );
                                        mutate_ui(
                                            &k_submit,
                                            |s| {
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
                                    mutate_ui(
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
    };

    // General access + Limit sharing (`F-COLLAB-163`, `F-COLLAB-164`, `F-COLLAB-168`).
    let (gen_badge, gen_desc) = general_access_summary(
        meta,
        (!ui.share_general_access.is_empty()).then_some(ui.share_general_access.as_str()),
        ui.share_limit_sharing,
    );
    let gen_btn = |tag: &str, label: &'static str, value: &'static str| {
        let active = gen_badge.eq_ignore_ascii_case(label);
        let k = key.to_owned();
        Button::new(SharedString::from(format!("canvas-gen-{key}-{tag}")))
            .when(active, |b| b.primary())
            .when(!active, |b| b.outline())
            .with_size(Size::XSmall)
            .label(label.to_owned())
            .disabled(!is_fixture)
            .when(!is_fixture, |b| {
                b.tooltip("Change workspace-wide general access in Slack (Open in browser)")
            })
            .on_click(move |_, window, cx| {
                if !is_fixture {
                    return;
                }
                mutate_ui(
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
    let k_limit = key.to_owned();
    let general_section = v_flex()
        .gap_1p5()
        .pt_2()
        .border_t_1()
        .border_color(theme.border)
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
        .when(!is_fixture, |col| {
            col.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Change workspace-wide general access in Slack (Open in browser)."),
            )
        })
        .child(
            h_flex()
                .flex_wrap()
                .gap_1()
                .child(gen_btn("invite", "Invite only", "private"))
                .child(gen_btn("view", "Workspace can view", "read"))
                .child(gen_btn("edit", "Workspace can edit", "write")),
        )
        .child(
            h_flex()
                .id(SharedString::from(format!("canvas-limit-share-{key}")))
                .items_center()
                .gap_1p5()
                .when(is_fixture, |row| row.cursor_pointer())
                .when(!is_fixture, |row| row.opacity(0.55))
                .text_xs()
                .text_color(theme.foreground)
                .when(is_fixture, |row| {
                    row.on_click(move |_, window, cx| {
                        mutate_ui(
                            &k_limit,
                            |s| s.share_limit_sharing = !s.share_limit_sharing,
                            window,
                            cx,
                        );
                    })
                })
                .child(
                    Icon::new(if ui.share_limit_sharing {
                        IconName::SquareCheck
                    } else {
                        IconName::Square
                    })
                    .size(px(14.))
                    .text_color(if ui.share_limit_sharing {
                        theme.link
                    } else {
                        theme.muted_foreground
                    }),
                )
                .child("Limit sharing (owners only)"),
        );

    // Owners & current editors list (`F-COLLAB-169`, `F-COLLAB-170`).
    let owner_id = meta
        .and_then(|m| m.canvas_creator_id.clone())
        .or_else(|| is_fixture.then(|| UserId::from("U01MAYA01")));
    let editors: Vec<UserId> = if let Some(m) = meta {
        m.editors.clone()
    } else if is_fixture {
        if ui.share_local_editors.is_empty() {
            vec![UserId::from("U02LEO02")]
        } else {
            ui.share_local_editors.clone()
        }
    } else {
        Vec::new()
    };

    let resolve_user = |uid: &UserId| -> String {
        actions
            .user_lookup
            .as_ref()
            .and_then(|f| f(uid.as_str()))
            .map(|p| p.display_name)
            .unwrap_or_else(|| uid.as_str().to_owned())
    };

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
        let owner_name = resolve_user(&owner);
        people_col = people_col.child(
            h_flex()
                .items_center()
                .justify_between()
                .p_1p5()
                .rounded_sm()
                .bg(theme.secondary.opacity(0.3))
                .text_xs()
                .child(div().font_weight(FontWeight::MEDIUM).child(owner_name))
                .child(
                    div()
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
        let name = resolve_user(&uid);
        let on_set = actions.on_set_access.clone();
        let on_del = actions.on_delete_access.clone();
        let uid_owner = uid.clone();
        let uid_del = uid.clone();
        let k_owner = key.to_owned();
        let k_del = key.to_owned();
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
                .child(div().flex_1().truncate().child(name))
                .child(
                    Button::new(SharedString::from(format!(
                        "canvas-share-promote-{key}-{ix}"
                    )))
                    .ghost()
                    .with_size(Size::XSmall)
                    .label("Owner")
                    .tooltip("Grant Owner role")
                    .on_click(move |_, window, cx| {
                        let req = ShareAccessRequest {
                            access_level: CanvasAccessLevel::Owner,
                            channel_ids: Vec::new(),
                            user_ids: vec![uid_owner.clone()],
                            post_permalink: None,
                        };
                        if let Some(cb) = &on_set {
                            cb(req, window, cx);
                        } else if is_fixture {
                            let uid_s = uid_owner.as_str().to_owned();
                            mutate_ui(
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
                    Button::new(SharedString::from(format!("canvas-share-rm-{key}-{ix}")))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::X)
                        .tooltip("Remove access")
                        .on_click(move |_, window, cx| {
                            if let Some(cb) = &on_del {
                                cb(Vec::new(), vec![uid_del.clone()], window, cx);
                            }
                            if is_fixture {
                                let target = uid_del.clone();
                                mutate_ui(
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

    let rules_footer = div()
        .pt_2()
        .border_t_1()
        .border_color(theme.border)
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(
            "Workspace policy: guest/external access, frontline creation limits, free-plan canvas caps, converted channel rights, and admin deletion are enforced by Slack.",
        );

    v_flex()
        .id(SharedString::from(format!("canvas-share-drawer-{key}")))
        .w(px(320.))
        .flex_none()
        .h_full()
        .overflow_y_scroll()
        .border_l_1()
        .border_color(theme.border)
        .bg(theme.background)
        .p_3()
        .gap_3()
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .pb_2()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            Icon::new(IconName::Users)
                                .size(px(14.))
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Share & access"),
                        ),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap_1()
                        .child(
                            Button::new(SharedString::from(format!("canvas-share-copy-{key}")))
                                .outline()
                                .with_size(Size::XSmall)
                                .icon(if ui.link_copied {
                                    IconName::Check
                                } else {
                                    IconName::Link
                                })
                                .label(if ui.link_copied {
                                    "Copied"
                                } else {
                                    "Copy link"
                                })
                                .on_click(move |_, window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        link_for_copy.clone(),
                                    ));
                                    mutate_ui(&key_copy, |s| s.link_copied = true, window, cx);
                                    crate::actions::notify(
                                        "Link copied",
                                        "Canvas link copied to clipboard.",
                                        window,
                                        cx,
                                    );
                                }),
                        )
                        .child(
                            Button::new(SharedString::from(format!("canvas-share-close-{key}")))
                                .ghost()
                                .with_size(Size::XSmall)
                                .icon(IconName::X)
                                .on_click(move |_, window, cx| {
                                    mutate_ui(&key_close, |s| s.share_open = false, window, cx);
                                }),
                        ),
                ),
        )
        .children(status_banner)
        .child(target_section)
        .child(general_section)
        .child(people_col)
        .child(rules_footer)
}
