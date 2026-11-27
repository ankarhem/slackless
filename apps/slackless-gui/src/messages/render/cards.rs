//! Huddle thread cards and duration formatting inside the message transcript.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, Div, FontWeight, ParentElement as _, SharedString, Styled as _, div, px,
};
use slackless_core::types::{Message, UserId};

use super::now_secs;
use crate::blockkit::KitCx;

/// Format huddle duration cleanly (`"16s"`, `"12m 4s"`, `"1h 5m"`).
pub(crate) fn format_huddle_duration(secs: u64) -> String {
    slackless_core::types::time::format_huddle_duration(secs)
}

/// Render official Slack's rich `huddle_thread` card in the message transcript (`F-COLLAB-003`).
pub(super) fn render_huddle_thread_card(message: &Message, kcx: &KitCx, cx: &App) -> Option<Div> {
    let fixtures = kcx.fixtures;
    let room = message.extra.get("room")?;
    let theme = cx.theme();
    let active_participants_empty = room
        .get("participants")
        .and_then(|v| v.as_array())
        .is_none_or(Vec::is_empty);
    let date_start = room.get("date_start").and_then(|v| v.as_u64());
    let now_u64 = now_secs().max(0) as u64;
    let stale_empty = active_participants_empty
        && date_start.is_none_or(|start| now_u64.saturating_sub(start) > 120);
    // Match `ShRoom::is_active()`: a huddle room with zero active participants after initial
    // handshake (e.g. abandoned without a final `has_ended` mutation) is not live.
    let has_ended = room
        .get("has_ended")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || stale_empty;
    let custom_name = room
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let title = custom_name.map(str::to_owned).unwrap_or_else(|| {
        if has_ended {
            "A huddle happened".to_owned()
        } else {
            "A huddle is happening".to_owned()
        }
    });
    let date_end = room.get("date_end").and_then(|v| v.as_u64());
    let duration_label = match (date_start, date_end) {
        (Some(start), Some(end)) if end > start => Some(format_huddle_duration(end - start)),
        _ => None,
    };
    let participant_ids: Vec<UserId> = room
        .get(if has_ended {
            "participant_history"
        } else {
            "participants"
        })
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|u| u.as_str().map(UserId::from))
                .collect()
        })
        .unwrap_or_default();
    let joined_summary = match participant_ids.len() {
        0 => None,
        1 => Some(format!(
            "{} joined",
            fixtures.user_name(&participant_ids[0])
        )),
        n => Some(format!("{n} people joined")),
    };
    let accent = if has_ended { theme.link } else { theme.success };
    let subtitle = match (joined_summary, duration_label) {
        (Some(j), Some(d)) => format!("{j} · {d}"),
        (Some(j), None) => j,
        (None, Some(d)) => format!("Lasted {d}"),
        (None, None) => {
            if has_ended {
                "Ended".to_owned()
            } else {
                "Live now".to_owned()
            }
        }
    };
    // Live huddle row: Official Slack's soft mint-green full-width callout banner with
    // green headphones, participant summary, and prominent green "Join Huddle" button.
    if !has_ended {
        let who = match participant_ids.as_slice() {
            [] => String::new(),
            [only] => format!(" {} is already there.", fixtures.user_name(only)),
            [first, second] => format!(
                " {} and {} are already there.",
                fixtures.user_name(first),
                fixtures.user_name(second),
            ),
            [first, second, rest @ ..] => format!(
                " {}, {} and {} others are already there.",
                fixtures.user_name(first),
                fixtures.user_name(second),
                rest.len(),
            ),
        };
        let banner_bg = if theme.mode.is_dark() {
            theme.success.opacity(0.14)
        } else {
            theme.success.opacity(0.10)
        };
        return Some(
            h_flex()
                .w_full()
                .px_3()
                .py_2p5()
                .gap_3()
                .items_start()
                .rounded_lg()
                .bg(banner_bg)
                .child(
                    div().pt(px(2.)).flex_none().child(
                        Icon::new(IconName::Headphones)
                            .size(px(16.))
                            .text_color(theme.primary),
                    ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.foreground)
                                .child(format!("{title}.{who}")),
                        )
                        .when_some(kcx.on_action.clone(), |col, handler| {
                            let scope = kcx.kit.map(|k| k.entity_id().as_u64()).unwrap_or(0);
                            let ts_key = message.ts.as_str().to_owned();
                            let join_handler = handler.clone();
                            let thread_id =
                                SharedString::from(format!("huddle-thread-{scope}-{ts_key}"));
                            col.child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Button::new(SharedString::from(format!(
                                            "huddle-join-{scope}-{ts_key}"
                                        )))
                                        .primary()
                                        .with_size(Size::Small)
                                        .label("Join Huddle")
                                        .on_click(
                                            move |_, window, cx| {
                                                join_handler(
                                                    crate::actions::MessageAction::JoinHuddle,
                                                    window,
                                                    cx,
                                                );
                                            },
                                        ),
                                    )
                                    .when(message.reply_count.unwrap_or(0) > 0, |btns| {
                                        let replies = message.reply_count.unwrap_or(0);
                                        let label = if replies == 1 {
                                            "1 reply".to_owned()
                                        } else {
                                            format!("{replies} replies")
                                        };
                                        btns.child(
                                            Button::new(thread_id)
                                                .ghost()
                                                .with_size(Size::Small)
                                                .icon(IconName::MessageSquare)
                                                .label(label)
                                                .on_click(move |_, window, cx| {
                                                    handler(
                                                        crate::actions::MessageAction::ReplyInThread,
                                                        window,
                                                        cx,
                                                    );
                                                }),
                                        )
                                    }),
                            )
                        }),
                ),
        );
    }

    Some(
        h_flex()
            .max_w(px(460.))
            .p_3()
            .gap_3()
            .items_center()
            .rounded_lg()
            .border_1()
            .border_color(if has_ended {
                theme.border
            } else {
                accent.opacity(0.45)
            })
            .bg(theme.secondary.opacity(0.42))
            .child(
                div()
                    .size(px(36.))
                    .flex_none()
                    .rounded_md()
                    .bg(accent.opacity(0.16))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::Headphones)
                            .size(px(18.))
                            .text_color(accent),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(subtitle),
                    ),
            )
            .when(!participant_ids.is_empty(), |row| {
                row.child(
                    h_flex().items_center().gap_0p5().children(
                        participant_ids
                            .iter()
                            .take(4)
                            .map(|uid| crate::avatars::user_chip(fixtures, uid, 22.)),
                    ),
                )
            })
            .when_some(kcx.on_action.clone(), |row, handler| {
                let scope = kcx.kit.map(|k| k.entity_id().as_u64()).unwrap_or(0);
                let ts_key = message.ts.as_str().to_owned();
                let join_handler = handler.clone();
                row.child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .flex_none()
                        .when(!has_ended, |btns| {
                            let id = SharedString::from(format!("huddle-join-{scope}-{ts_key}"));
                            btns.child(
                                Button::new(id)
                                    .primary()
                                    .with_size(Size::XSmall)
                                    .label("Join")
                                    .on_click(move |_, window, cx| {
                                        join_handler(
                                            crate::actions::MessageAction::JoinHuddle,
                                            window,
                                            cx,
                                        );
                                    }),
                            )
                        })
                        .child(
                            Button::new(SharedString::from(format!(
                                "huddle-thread-{scope}-{ts_key}"
                            )))
                            .outline()
                            .with_size(Size::XSmall)
                            .label("Thread")
                            .on_click(move |_, window, cx| {
                                handler(crate::actions::MessageAction::ReplyInThread, window, cx);
                            }),
                        ),
                )
            }),
    )
}
