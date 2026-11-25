//! Inline mention chip renderer for Canvas text blocks (`F-COLLAB-150`), backed by the pure
//! tokenizer in `slackless_core::domain::canvas_doc`.

use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, Div, FontWeight, InteractiveElement as _, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div,
};
use slackless_core::types::{ChannelId, UserId};

use crate::canvas::blocks::BlockCallbacks;
use crate::canvas::parse::resolve_canvas_emoji;

pub use slackless_core::domain::canvas_doc::{CanvasInlineSegment, parse_inline_canvas_segments};

/// Render canvas inline text with resolved emoji and interactive `@user`, `#channel`,
/// `@usergroup`, and `@here`/`@channel` mention chips (`F-COLLAB-150`).
pub(super) fn render_inline_canvas_text(
    id_prefix: &str,
    text: &str,
    cbs: &BlockCallbacks,
    cx: &App,
) -> Div {
    let segments = parse_inline_canvas_segments(text);
    if segments.is_empty() {
        return div();
    }
    if segments.len() == 1
        && let CanvasInlineSegment::Text(raw) = &segments[0]
    {
        return div().child(resolve_canvas_emoji(raw));
    }

    let theme = cx.theme();
    let link_bg = theme.link.opacity(0.14);
    let link_hover = theme.link.opacity(0.24);
    let link_fg = theme.link;
    let warn_bg = theme.warning.opacity(0.18);
    let warn_fg = theme.warning_foreground;
    let group_bg = theme.secondary.opacity(0.65);
    let group_fg = theme.foreground;

    let mut row = h_flex()
        .flex_wrap()
        .items_center()
        .gap_x(gpui_kit::gpui::px(3.));
    for (seg_ix, seg) in segments.into_iter().enumerate() {
        match seg {
            CanvasInlineSegment::Text(raw) => {
                let resolved = resolve_canvas_emoji(&raw);
                if !resolved.is_empty() {
                    row = row.child(div().child(resolved));
                }
            }
            CanvasInlineSegment::UserMention { user_id, label } => {
                let resolved_name = cbs
                    .user_lookup
                    .as_ref()
                    .and_then(|lookup| lookup(&user_id))
                    .map(|p| p.display_name)
                    .or(label)
                    .unwrap_or_else(|| user_id.clone());
                let uid = UserId::from(user_id.as_str());
                let on_open = cbs.on_open_profile.clone();
                let chip = div()
                    .id(SharedString::from(format!("{id_prefix}-um-{seg_ix}")))
                    .px_1()
                    .rounded_sm()
                    .bg(link_bg)
                    .text_color(link_fg)
                    .font_weight(FontWeight::MEDIUM)
                    .when_some(on_open, move |el, cb| {
                        el.cursor_pointer()
                            .hover(move |s| s.bg(link_hover))
                            .on_click(move |_, window, cx| {
                                cb(uid.clone(), window, cx);
                            })
                    })
                    .child(format!("@{resolved_name}"));
                row = row.child(chip);
            }
            CanvasInlineSegment::ChannelMention { channel_id, name } => {
                let resolved_name = cbs
                    .channel_lookup
                    .as_ref()
                    .and_then(|lookup| lookup(&channel_id))
                    .or(name)
                    .unwrap_or_else(|| channel_id.clone());
                let cid = ChannelId::from(channel_id.as_str());
                let on_open = cbs.on_open_channel.clone();
                let chip = div()
                    .id(SharedString::from(format!("{id_prefix}-cm-{seg_ix}")))
                    .px_1()
                    .rounded_sm()
                    .bg(link_bg)
                    .text_color(link_fg)
                    .font_weight(FontWeight::MEDIUM)
                    .when_some(on_open, move |el, cb| {
                        el.cursor_pointer()
                            .hover(move |s| s.bg(link_hover))
                            .on_click(move |_, window, cx| {
                                cb(cid.clone(), window, cx);
                            })
                    })
                    .child(format!("#{resolved_name}"));
                row = row.child(chip);
            }
            CanvasInlineSegment::UsergroupMention { handle, .. } => {
                row = row.child(
                    div()
                        .px_1()
                        .rounded_sm()
                        .bg(group_bg)
                        .text_color(group_fg)
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("@{handle}")),
                );
            }
            CanvasInlineSegment::BroadcastMention(kind) => {
                row = row.child(
                    div()
                        .px_1()
                        .rounded_sm()
                        .bg(warn_bg)
                        .text_color(warn_fg)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("@{kind}")),
                );
            }
        }
    }
    row
}
