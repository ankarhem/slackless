//! Durable offline send outbox UI: inline queued/failed cards at the bottom of channels and
//! threads, plus a compact top-bar badge when pending or failed outbox items exist.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, AnyElement, App, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Stateful, Styled as _, Window, div, px,
};
use slackless_core::outbox::decode_outbox_payload;
use slackless_core::store::{OutboxItem, OutboxStatus};
use slackless_core::types::{ChannelId, Ts};

use crate::fixtures::Fixtures;

/// Callback invoked for per-item outbox actions (retry / discard).
pub type OutboxItemCallback = Rc<dyn Fn(i64, &mut Window, &mut App)>;
/// Callback invoked to flush all pending outbox items.
pub type OutboxFlushCallback = Rc<dyn Fn(&mut Window, &mut App)>;
/// Callback invoked to jump to the channel/thread of an outbox item.
pub type OutboxOpenCallback = Rc<dyn Fn(ChannelId, Option<Ts>, &mut Window, &mut App)>;

/// Callbacks wired by the caller (`App` or `ThreadPanel`) for outbox card actions.
#[derive(Clone)]
pub struct OutboxActions {
    pub on_retry: OutboxItemCallback,
    pub on_discard: OutboxItemCallback,
}

/// Filter outbox items matching a specific channel (and optional thread root).
///
/// - `thread_ts == None`: returns channel-level messages and reactions (`item.thread_ts.is_none()`).
/// - `thread_ts == Some(ts)`: returns replies targeting that thread (`item.thread_ts == Some(ts)`).
pub fn filter_outbox_items<'a>(
    items: &'a [OutboxItem],
    channel: &ChannelId,
    thread_ts: Option<&Ts>,
) -> Vec<&'a OutboxItem> {
    items
        .iter()
        .filter(|item| {
            item.channel_id == *channel
                && match thread_ts {
                    Some(ts) => item.thread_ts.as_ref() == Some(ts),
                    None => item.thread_ts.is_none(),
                }
        })
        .collect()
}

/// Human-facing status label for an inline outbox card.
pub const fn status_label(status: OutboxStatus) -> &'static str {
    match status {
        OutboxStatus::Pending => "Queued \u{2014} will send when online",
        OutboxStatus::Sending => "Sending\u{2026}",
        OutboxStatus::Failed => "Failed to send",
    }
}

/// Human-facing preview text for an [`OutboxItem`].
pub fn item_preview(item: &OutboxItem) -> String {
    decode_outbox_payload(item)
        .map(|p| p.preview_text())
        .unwrap_or_else(|| "Queued message".to_owned())
}

/// Compact summary `(pending_count, sending_count, failed_count)` for badges and tests.
pub fn summarize_outbox(items: &[OutboxItem]) -> (usize, usize, usize) {
    let mut pending = 0;
    let mut sending = 0;
    let mut failed = 0;
    for item in items {
        match item.status {
            OutboxStatus::Pending => pending += 1,
            OutboxStatus::Sending => sending += 1,
            OutboxStatus::Failed => failed += 1,
        }
    }
    (pending, sending, failed)
}

/// Render inline queued / sending / failed outbox cards for the bottom of a channel or thread.
/// Returns `None` when no outbox items belong to `(channel, thread_ts)`.
pub fn inline_outbox_strip(
    items: &[OutboxItem],
    channel: &ChannelId,
    thread_ts: Option<&Ts>,
    actions: OutboxActions,
    id_prefix: &'static str,
    cx: &App,
) -> Option<Stateful<gpui_kit::gpui::Div>> {
    let matching = filter_outbox_items(items, channel, thread_ts);
    if matching.is_empty() {
        return None;
    }

    let theme = cx.theme();
    let fg = theme.foreground;
    let muted = theme.muted_foreground;
    let border = theme.border;
    let warning = theme.warning;
    let danger = theme.danger;
    let card_bg = theme.secondary.opacity(0.45);

    Some(
        v_flex()
            .id(SharedString::from(format!("{id_prefix}-outbox-strip")))
            .flex_none()
            .w_full()
            .px_4()
            .py_2()
            .gap_1p5()
            .border_t_1()
            .border_color(border.opacity(0.6))
            .children(matching.into_iter().map(|item| {
                let item_id = item.id;
                let preview = item_preview(item);
                let is_failed = item.status == OutboxStatus::Failed;
                let is_sending = item.status == OutboxStatus::Sending;
                let (icon, accent, row_bg, row_border) = match item.status {
                    OutboxStatus::Pending => {
                        (IconName::Clock, warning, card_bg, warning.opacity(0.35))
                    }
                    OutboxStatus::Sending => (IconName::RefreshCw, muted, card_bg, border),
                    OutboxStatus::Failed => (
                        IconName::TriangleAlert,
                        danger,
                        danger.opacity(0.08),
                        danger.opacity(0.4),
                    ),
                };
                let status_text = status_label(item.status);
                let err_detail = item
                    .last_error
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| {
                        let clipped: String = s.chars().take(96).collect();
                        if s.chars().count() > 96 {
                            format!("{clipped}\u{2026}")
                        } else {
                            clipped
                        }
                    });
                let retry_cb = Rc::clone(&actions.on_retry);
                let discard_cb = Rc::clone(&actions.on_discard);

                h_flex()
                    .id(SharedString::from(format!("{id_prefix}-outbox-{item_id}")))
                    .w_full()
                    .min_w_0()
                    .items_start()
                    .justify_between()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded(px(8.))
                    .bg(row_bg)
                    .border_1()
                    .border_color(row_border)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(if is_failed { fg } else { fg.opacity(0.82) })
                                    .truncate()
                                    .child(preview),
                            )
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .items_center()
                                    .gap_1p5()
                                    .child(Icon::new(icon).size(px(12.)).text_color(accent))
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(if is_failed { danger } else { muted })
                                            .child(status_text),
                                    )
                                    .when_some(err_detail, |row, detail| {
                                        row.child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_xs()
                                                .text_color(muted)
                                                .truncate()
                                                .child(format!("\u{00B7} {detail}")),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .when(!is_sending, |btns| {
                                btns.child(
                                    Button::new(SharedString::from(format!(
                                        "{id_prefix}-outbox-retry-{item_id}"
                                    )))
                                    .ghost()
                                    .with_size(Size::XSmall)
                                    .icon(IconName::RefreshCw)
                                    .label("Retry")
                                    .on_click(
                                        move |_, window, cx| {
                                            retry_cb(item_id, window, cx);
                                        },
                                    ),
                                )
                            })
                            .child(
                                Button::new(SharedString::from(format!(
                                    "{id_prefix}-outbox-discard-{item_id}"
                                )))
                                .ghost()
                                .with_size(Size::XSmall)
                                .icon(IconName::Trash)
                                .label("Discard")
                                .on_click(
                                    move |_, window, cx| {
                                        discard_cb(item_id, window, cx);
                                    },
                                ),
                            ),
                    )
            })),
    )
}

/// Callbacks wired by `App` for the compact top-bar outbox badge dropdown.
#[derive(Clone)]
pub struct TopBarOutboxActions {
    pub on_flush: OutboxFlushCallback,
    pub on_open: OutboxOpenCallback,
    pub on_retry: OutboxItemCallback,
    pub on_discard: OutboxItemCallback,
}

/// Compact top-bar badge shown whenever pending/sending/failed outbox items exist.
pub fn top_bar_outbox_badge(
    items: &[OutboxItem],
    fixtures: &Fixtures,
    chrome_fg: Hsla,
    actions: TopBarOutboxActions,
    cx: &App,
) -> Option<AnyElement> {
    if items.is_empty() {
        return None;
    }
    let (pending, sending, failed) = summarize_outbox(items);
    let total = items.len();
    let theme = cx.theme();
    let accent = if failed > 0 {
        theme.danger
    } else {
        theme.warning
    };
    let icon = if failed > 0 {
        IconName::TriangleAlert
    } else {
        IconName::Clock
    };
    let badge_text = if failed > 0 && pending + sending > 0 {
        format!("{total} outbox ({failed} failed)")
    } else if failed > 0 {
        format!("{failed} failed")
    } else {
        format!("{total} queued")
    };

    let entries: Vec<(i64, ChannelId, Option<Ts>, String, OutboxStatus)> = items
        .iter()
        .take(12)
        .map(|item| {
            let ch_label = fixtures
                .conversation(&item.channel_id)
                .map(|c| fixtures.conversation_label(c))
                .unwrap_or_else(|| item.channel_id.as_str().to_owned());
            let summary = decode_outbox_payload(item)
                .map(|p| p.summary())
                .unwrap_or_else(|| "Queued message".to_owned());
            (
                item.id,
                item.channel_id.clone(),
                item.thread_ts.clone(),
                format!("{ch_label}: {summary}"),
                item.status,
            )
        })
        .collect();

    Some(
        Button::new("top-bar-outbox-badge")
            .ghost()
            .with_size(Size::XSmall)
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .px_1p5()
                    .h(px(22.))
                    .rounded_full()
                    .border_1()
                    .border_color(accent.opacity(0.55))
                    .bg(accent.opacity(0.18))
                    .child(Icon::new(icon).size(px(12.)).text_color(accent))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(chrome_fg)
                            .child(badge_text),
                    ),
            )
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                if pending > 0 {
                    let on_flush = Rc::clone(&actions.on_flush);
                    menu = menu.item(
                        PopupMenuItem::new(format!("Send queued now ({pending})"))
                            .icon(IconName::Send)
                            .on_click(move |_, window, cx| on_flush(window, cx)),
                    );
                }
                menu = menu.separator().label("Outbox");
                for (id, channel, thread_ts, label, status) in &entries {
                    let item_id = *id;
                    let open_cb = Rc::clone(&actions.on_open);
                    let open_ch = channel.clone();
                    let open_ts = thread_ts.clone();
                    let row_icon = match status {
                        OutboxStatus::Failed => IconName::TriangleAlert,
                        OutboxStatus::Sending => IconName::RefreshCw,
                        OutboxStatus::Pending => IconName::Clock,
                    };
                    menu = menu.item(PopupMenuItem::new(label.clone()).icon(row_icon).on_click(
                        move |_, window, cx| {
                            open_cb(open_ch.clone(), open_ts.clone(), window, cx);
                        },
                    ));
                    if *status == OutboxStatus::Failed {
                        let retry_cb = Rc::clone(&actions.on_retry);
                        let discard_cb = Rc::clone(&actions.on_discard);
                        menu = menu
                            .item(
                                PopupMenuItem::new("  Retry send")
                                    .icon(IconName::RefreshCw)
                                    .on_click(move |_, window, cx| retry_cb(item_id, window, cx)),
                            )
                            .item(
                                PopupMenuItem::new("  Discard")
                                    .icon(IconName::Trash)
                                    .on_click(move |_, window, cx| discard_cb(item_id, window, cx)),
                            );
                    }
                }
                menu
            })
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use slackless_core::api::MessageContent;
    use slackless_core::outbox::OutboxPayload;
    use slackless_core::store::OutboxKind;
    use slackless_core::types::TeamId;

    use super::*;

    fn sample_item(
        id: i64,
        channel: &str,
        thread_ts: Option<&str>,
        status: OutboxStatus,
        text: &str,
    ) -> OutboxItem {
        let payload = match thread_ts {
            Some(_) => OutboxPayload::Reply {
                content: MessageContent::plain(text),
                broadcast: false,
            },
            None => OutboxPayload::Message {
                content: MessageContent::plain(text),
            },
        };
        OutboxItem {
            id,
            workspace_id: TeamId::from("T1"),
            channel_id: ChannelId::from(channel),
            thread_ts: thread_ts.map(Ts::from),
            kind: if thread_ts.is_some() {
                OutboxKind::Reply
            } else {
                OutboxKind::Message
            },
            payload_json: payload.encode(),
            status,
            attempts: if status == OutboxStatus::Failed { 1 } else { 0 },
            last_error: (status == OutboxStatus::Failed).then(|| "network timeout".to_owned()),
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
        }
    }

    #[test]
    fn filter_outbox_items_scopes_channel_and_thread_cleanly() {
        let items = vec![
            sample_item(1, "C1", None, OutboxStatus::Pending, "channel msg"),
            sample_item(2, "C1", Some("100.0"), OutboxStatus::Failed, "thread reply"),
            sample_item(3, "C2", None, OutboxStatus::Sending, "other channel"),
        ];
        let c1_root = filter_outbox_items(&items, &ChannelId::from("C1"), None);
        assert_eq!(c1_root.len(), 1);
        assert_eq!(c1_root[0].id, 1);

        let c1_thread =
            filter_outbox_items(&items, &ChannelId::from("C1"), Some(&Ts::from("100.0")));
        assert_eq!(c1_thread.len(), 1);
        assert_eq!(c1_thread[0].id, 2);

        assert_eq!(summarize_outbox(&items), (1, 1, 1));
        assert_eq!(item_preview(&items[0]), "channel msg");
        assert_eq!(
            status_label(OutboxStatus::Pending),
            "Queued \u{2014} will send when online"
        );
        assert_eq!(status_label(OutboxStatus::Failed), "Failed to send");
    }
}
