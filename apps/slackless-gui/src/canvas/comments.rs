//! Threaded Canvas Comments drawer: show all threads, Open/Resolved/All filters, inline replies,
//! per-thread Resolve/Reopen, and Resolve all with confirmation
//! (`F-COLLAB-152`, `F-COLLAB-155`, `F-COLLAB-156`, `F-COLLAB-197`, `F-COLLAB-199`).

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Div, FontWeight, InteractiveElement as _, ParentElement as _,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::api::{CanvasCommentReply, CanvasCommentThread, format_canvas_error};
use slackless_core::types::{Ts, UserId};

use crate::canvas::keyboard::CanvasFocusRegion;
use crate::canvas::parse::resolve_canvas_emoji;
use crate::canvas::{CanvasActions, CanvasUiState, mutate_ui};

/// Filter tab for the Canvas Comments drawer (`F-COLLAB-155`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CanvasCommentFilter {
    #[default]
    All,
    Open,
    Resolved,
}

/// Callback invoked when opening or refreshing the Comments drawer (`Command::LoadCanvasComments`).
pub type CanvasCommentsLoadFn = Rc<dyn Fn(&mut Window, &mut App)>;
/// Callback invoked when posting a root comment (`thread_ts: None`) or reply (`Some(root_ts)`).
pub type CanvasCommentPostFn = Rc<dyn Fn(String, Option<Ts>, &mut Window, &mut App)>;
/// Callback invoked when resolving (`true`) or reopening (`false`) one comment thread.
pub type CanvasCommentResolveFn = Rc<dyn Fn(Ts, bool, &mut Window, &mut App)>;
/// Callback invoked when resolving all open comment threads (`Command::ResolveAllCanvasComments`).
pub type CanvasCommentResolveAllFn = Rc<dyn Fn(&mut Window, &mut App)>;

/// Filter comment threads by [`CanvasCommentFilter`] (`F-COLLAB-155`).
pub fn filter_comment_threads(
    threads: &[CanvasCommentThread],
    filter: CanvasCommentFilter,
) -> Vec<&CanvasCommentThread> {
    threads
        .iter()
        .filter(|t| match filter {
            CanvasCommentFilter::All => true,
            CanvasCommentFilter::Open => !t.resolved,
            CanvasCommentFilter::Resolved => t.resolved,
        })
        .collect()
}

/// Compute `(total, open, resolved)` counts for the Comments drawer filter tabs.
pub fn comment_thread_counts(threads: &[CanvasCommentThread]) -> (usize, usize, usize) {
    let total = threads.len();
    let resolved = threads.iter().filter(|t| t.resolved).count();
    let open = total.saturating_sub(resolved);
    (total, open, resolved)
}

/// Pre-seeded sample comment threads for fixture mode so threaded replies & resolve/reopen work
/// out of the box without a live Slack backend.
pub(super) fn sample_comment_threads() -> Vec<CanvasCommentThread> {
    vec![
        CanvasCommentThread {
            root_ts: Ts::from("1700000100.000100"),
            author: Some(UserId::from("U01MAYA01")),
            text: "Should we include the notarization checklist in the public beta milestone?"
                .to_owned(),
            reply_count: 1,
            replies: vec![CanvasCommentReply {
                ts: Ts::from("1700000120.000200"),
                author: Some(UserId::from("U02LEO02")),
                text: "Yes — added it to the rollout steps above :white_check_mark:".to_owned(),
            }],
            resolved: false,
        },
        CanvasCommentThread {
            root_ts: Ts::from("1700000050.000100"),
            author: Some(UserId::from("U03SAM03")),
            text: "Updated the GA target date to Dec 2 after QA sign-off.".to_owned(),
            reply_count: 0,
            replies: Vec::new(),
            resolved: true,
        },
    ]
}

fn author_label(author: Option<&UserId>, actions: &CanvasActions) -> String {
    let Some(uid) = author else {
        return "Teammate".to_owned();
    };
    if let Some(lookup) = &actions.user_lookup
        && let Some(profile) = lookup(uid.as_str())
    {
        return profile.display_name;
    }
    if uid.as_str() == "YOU" {
        "You".to_owned()
    } else {
        uid.as_str().to_owned()
    }
}

pub(super) fn render_comments_drawer(
    key: &str,
    ui: &CanvasUiState,
    actions: &CanvasActions,
    is_fixture: bool,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let focused = ui.keyboard.region == CanvasFocusRegion::Comments;

    let threads: Vec<CanvasCommentThread> = if let Some(live) = &actions.comment_threads {
        live.clone()
    } else {
        ui.comments.clone()
    };
    let (total_count, open_count, resolved_count) = comment_thread_counts(&threads);
    let visible = filter_comment_threads(&threads, ui.comment_filter);

    let key_close = key.to_owned();
    let key_submit = key.to_owned();
    let on_post_root = actions.on_post_comment.clone();
    let input_opt = ui.comment_input.clone();

    let filter_btn = |id_tag: &str, label: String, filter: CanvasCommentFilter| {
        let active = ui.comment_filter == filter;
        let k = key.to_owned();
        Button::new(SharedString::from(format!("canvas-cmt-flt-{key}-{id_tag}")))
            .when(active, |b| b.primary())
            .when(!active, |b| b.ghost())
            .with_size(Size::XSmall)
            .label(label)
            .on_click(move |_, window, cx| {
                mutate_ui(&k, |s| s.comment_filter = filter, window, cx);
            })
    };

    let filter_bar = h_flex()
        .items_center()
        .gap_1()
        .child(filter_btn(
            "all",
            format!("All ({total_count})"),
            CanvasCommentFilter::All,
        ))
        .child(filter_btn(
            "open",
            format!("Open ({open_count})"),
            CanvasCommentFilter::Open,
        ))
        .child(filter_btn(
            "resolved",
            format!("Resolved ({resolved_count})"),
            CanvasCommentFilter::Resolved,
        ));

    let resolve_all_bar = (open_count > 0).then(|| {
        if ui.confirm_resolve_all {
            let k_yes = key.to_owned();
            let k_no = key.to_owned();
            let on_resolve_all = actions.on_resolve_all_comments.clone();
            h_flex()
                .items_center()
                .justify_between()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.secondary.opacity(0.45))
                .text_xs()
                .child(div().child(format!("Resolve all {open_count} open?")))
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new(SharedString::from(format!("canvas-cmt-resall-ok-{key}")))
                                .primary()
                                .with_size(Size::XSmall)
                                .label("Confirm")
                                .on_click(move |_, window, cx| {
                                    if let Some(cb) = &on_resolve_all {
                                        cb(window, cx);
                                    }
                                    mutate_ui(
                                        &k_yes,
                                        |s| {
                                            s.confirm_resolve_all = false;
                                            if is_fixture {
                                                for t in &mut s.comments {
                                                    t.resolved = true;
                                                }
                                            }
                                        },
                                        window,
                                        cx,
                                    );
                                }),
                        )
                        .child(
                            Button::new(SharedString::from(format!("canvas-cmt-resall-no-{key}")))
                                .ghost()
                                .with_size(Size::XSmall)
                                .label("Cancel")
                                .on_click(move |_, window, cx| {
                                    mutate_ui(&k_no, |s| s.confirm_resolve_all = false, window, cx);
                                }),
                        ),
                )
        } else {
            let k_ask = key.to_owned();
            h_flex().justify_end().child(
                Button::new(SharedString::from(format!("canvas-cmt-resall-{key}")))
                    .outline()
                    .with_size(Size::XSmall)
                    .icon(IconName::CheckCheck)
                    .label("Resolve all")
                    .on_click(move |_, window, cx| {
                        mutate_ui(&k_ask, |s| s.confirm_resolve_all = true, window, cx);
                    }),
            )
        }
    });

    let status_banner = actions.status_banner.as_ref().map(|(is_err, msg)| {
        let fg = if *is_err { theme.danger } else { theme.success };
        div()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(fg.opacity(0.4))
            .bg(fg.opacity(0.12))
            .text_xs()
            .text_color(theme.foreground)
            .child(format_canvas_error(msg))
    });

    let mut body = v_flex()
        .id(SharedString::from(format!("canvas-cmt-list-{key}")))
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .gap_2();

    if visible.is_empty() {
        let empty_msg = match ui.comment_filter {
            CanvasCommentFilter::All => {
                "No comments on this canvas yet. Start a thread below to discuss sections."
            }
            CanvasCommentFilter::Open => "No open comment threads.",
            CanvasCommentFilter::Resolved => "No resolved comment threads yet.",
        };
        body = body.child(
            div()
                .py_4()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(empty_msg),
        );
    } else {
        for (ix, thread) in visible.into_iter().enumerate() {
            let root_ts = thread.root_ts.clone();
            let is_resolved = thread.resolved;
            let is_replying = ui.replying_thread.as_ref() == Some(&root_ts);

            let mut card = v_flex()
                .gap_1p5()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(if is_resolved {
                    theme.secondary.opacity(0.16)
                } else {
                    theme.secondary.opacity(0.32)
                })
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(author_label(thread.author.as_ref(), actions)),
                        )
                        .when(is_resolved, |row| {
                            row.child(
                                div()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded_sm()
                                    .bg(theme.success.opacity(0.16))
                                    .text_xs()
                                    .text_color(theme.success)
                                    .child("Resolved"),
                            )
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(resolve_canvas_emoji(&thread.text)),
                );

            if !thread.replies.is_empty() {
                let mut replies_col = v_flex()
                    .gap_1()
                    .pl_2()
                    .border_l_2()
                    .border_color(theme.border);
                for reply in &thread.replies {
                    replies_col = replies_col.child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.foreground)
                                    .child(author_label(reply.author.as_ref(), actions)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(resolve_canvas_emoji(&reply.text)),
                            ),
                    );
                }
                card = card.child(replies_col);
            }

            let k_reply_open = key.to_owned();
            let ts_reply_open = root_ts.clone();
            let k_resolve = key.to_owned();
            let ts_resolve = root_ts.clone();
            let on_resolve = actions.on_resolve_comment.clone();

            card = card.child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .pt_0p5()
                    .child(
                        Button::new(SharedString::from(format!(
                            "canvas-cmt-reply-btn-{key}-{ix}"
                        )))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::Reply)
                        .label(if thread.replies.is_empty() {
                            "Reply".to_owned()
                        } else {
                            format!("Reply ({})", thread.replies.len())
                        })
                        .on_click(move |_, window, cx| {
                            let input = cx.new(|cx| {
                                TextareaState::new(window, cx)
                                    .placeholder("Write a reply…")
                                    .auto_grow(1, 3)
                            });
                            let ts = ts_reply_open.clone();
                            mutate_ui(
                                &k_reply_open,
                                |s| {
                                    if s.replying_thread.as_ref() == Some(&ts) {
                                        s.replying_thread = None;
                                        s.reply_input = None;
                                    } else {
                                        s.replying_thread = Some(ts);
                                        s.reply_input = Some(input);
                                    }
                                },
                                window,
                                cx,
                            );
                        }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("canvas-cmt-res-btn-{key}-{ix}")))
                            .ghost()
                            .with_size(Size::XSmall)
                            .icon(if is_resolved {
                                IconName::RotateCcw
                            } else {
                                IconName::Check
                            })
                            .label(if is_resolved { "Reopen" } else { "Resolve" })
                            .on_click(move |_, window, cx| {
                                let next_resolved = !is_resolved;
                                if let Some(cb) = &on_resolve {
                                    cb(ts_resolve.clone(), next_resolved, window, cx);
                                }
                                if is_fixture {
                                    let target_ts = ts_resolve.clone();
                                    mutate_ui(
                                        &k_resolve,
                                        |s| {
                                            if let Some(t) = s
                                                .comments
                                                .iter_mut()
                                                .find(|t| t.root_ts == target_ts)
                                            {
                                                t.resolved = next_resolved;
                                            }
                                        },
                                        window,
                                        cx,
                                    );
                                }
                            }),
                    ),
            );

            if is_replying && let Some(reply_input) = ui.reply_input.clone() {
                let input_send = reply_input.clone();
                let k_send = key.to_owned();
                let k_cancel = key.to_owned();
                let ts_send = root_ts.clone();
                let on_post_reply = actions.on_post_comment.clone();
                card = card.child(
                    v_flex()
                        .gap_1()
                        .pt_1()
                        .child(
                            div()
                                .rounded_md()
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.background)
                                .px_2()
                                .py_1()
                                .child(
                                    Textarea::new(&reply_input)
                                        .appearance(false)
                                        .bordered(false),
                                ),
                        )
                        .child(
                            h_flex()
                                .justify_end()
                                .gap_1()
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "canvas-cmt-reply-cancel-{key}-{ix}"
                                    )))
                                    .ghost()
                                    .with_size(Size::XSmall)
                                    .label("Cancel")
                                    .on_click(
                                        move |_, window, cx| {
                                            mutate_ui(
                                                &k_cancel,
                                                |s| {
                                                    s.replying_thread = None;
                                                    s.reply_input = None;
                                                },
                                                window,
                                                cx,
                                            );
                                        },
                                    ),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "canvas-cmt-reply-send-{key}-{ix}"
                                    )))
                                    .primary()
                                    .with_size(Size::XSmall)
                                    .label("Send")
                                    .on_click(
                                        move |_, window, cx| {
                                            let reply_text =
                                                input_send.read(cx).value().trim().to_owned();
                                            if reply_text.is_empty() {
                                                return;
                                            }
                                            if let Some(cb) = &on_post_reply {
                                                cb(
                                                    reply_text.clone(),
                                                    Some(ts_send.clone()),
                                                    window,
                                                    cx,
                                                );
                                            }
                                            let target_ts = ts_send.clone();
                                            mutate_ui(
                                                &k_send,
                                                |s| {
                                                    if is_fixture
                                                        && let Some(t) = s
                                                            .comments
                                                            .iter_mut()
                                                            .find(|t| t.root_ts == target_ts)
                                                    {
                                                        let next_seq = t.replies.len() + 1;
                                                        t.replies.push(CanvasCommentReply {
                                                            ts: Ts::from(format!(
                                                                "{}.{next_seq:06}",
                                                                target_ts.as_str()
                                                            )),
                                                            author: Some(UserId::from("YOU")),
                                                            text: reply_text,
                                                        });
                                                        t.reply_count = t.replies.len();
                                                    }
                                                    s.replying_thread = None;
                                                    s.reply_input = None;
                                                },
                                                window,
                                                cx,
                                            );
                                        },
                                    ),
                                ),
                        ),
                );
            }

            body = body.child(card);
        }
    }

    let composer = input_opt.map(|input| {
        let input_send = input.clone();
        v_flex()
            .gap_1p5()
            .pt_2()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .px_2()
                    .py_1()
                    .child(Textarea::new(&input).appearance(false).bordered(false)),
            )
            .child(
                h_flex().justify_end().child(
                    Button::new(SharedString::from(format!("canvas-comment-post-{key}")))
                        .primary()
                        .with_size(Size::XSmall)
                        .label("Comment")
                        .on_click(move |_, window, cx| {
                            let body_text = input_send.read(cx).value().trim().to_owned();
                            if body_text.is_empty() {
                                return;
                            }
                            input_send.update(cx, |st, cx| st.set_value(String::new(), window, cx));
                            if let Some(cb) = &on_post_root {
                                cb(body_text.clone(), None, window, cx);
                            }
                            if is_fixture {
                                mutate_ui(
                                    &key_submit,
                                    |s| {
                                        let next_id = 1700000200 + s.comments.len();
                                        s.comments.push(CanvasCommentThread {
                                            root_ts: Ts::from(format!("{next_id}.000100")),
                                            author: Some(UserId::from("YOU")),
                                            text: body_text,
                                            reply_count: 0,
                                            replies: Vec::new(),
                                            resolved: false,
                                        });
                                    },
                                    window,
                                    cx,
                                );
                            }
                        }),
                ),
            )
    });

    let refresh_btn = actions.on_load_comments.clone().map(|load_cb| {
        Button::new(SharedString::from(format!("canvas-comments-refresh-{key}")))
            .ghost()
            .with_size(Size::XSmall)
            .icon(IconName::RefreshCw)
            .tooltip("Refresh comments")
            .on_click(move |_, window, cx| load_cb(window, cx))
    });

    v_flex()
        .id(SharedString::from(format!("canvas-cmt-drawer-{key}")))
        .w(px(320.))
        .flex_none()
        .h_full()
        .border_l_1()
        .border_color(if focused { theme.ring } else { theme.border })
        .bg(theme.background)
        .p_3()
        .gap_2()
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
                            Icon::new(IconName::MessageSquare)
                                .size(px(14.))
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Comments"),
                        ),
                )
                .child(
                    h_flex().items_center().gap_1().children(refresh_btn).child(
                        Button::new(SharedString::from(format!("canvas-comments-close-{key}")))
                            .ghost()
                            .with_size(Size::XSmall)
                            .icon(IconName::X)
                            .on_click(move |_, window, cx| {
                                mutate_ui(&key_close, |s| s.comments_open = false, window, cx);
                            }),
                    ),
                ),
        )
        .child(filter_bar)
        .children(resolve_all_bar)
        .children(status_banner)
        .child(body)
        .children(composer)
}
