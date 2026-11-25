//! Inline mention tokenizer & chip renderer for Canvas text blocks (`F-COLLAB-150`).
//!
//! Recognizes Slack mrkdwn & Quip inline tokens:
//! - `@user`: `<@U...>`, `<@U...|label>`, `<control><a>@U...</a></control>`, `<control><a ...>@U...</a></control>`
//! - `#channel`: `<#C...|name>`, `<#C...>`, `<control><a>#C...</a></control>`
//! - `@usergroup`: `<!subteam^S...|@team>`, `<!subteam^S...>`
//! - Broadcasts: `<!here>`, `<!channel>`, `<!everyone>`, `@here`, `@channel`

use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, Div, FontWeight, InteractiveElement as _, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div,
};
use slackless_core::types::{ChannelId, UserId};

use crate::canvas::blocks::BlockCallbacks;
use crate::canvas::parse::resolve_canvas_emoji;

/// One parsed inline segment inside a canvas text block (`F-COLLAB-150`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanvasInlineSegment {
    Text(String),
    UserMention {
        user_id: String,
        label: Option<String>,
    },
    ChannelMention {
        channel_id: String,
        name: Option<String>,
    },
    UsergroupMention {
        group_id: String,
        handle: String,
    },
    BroadcastMention(String),
}

pub(super) fn is_upper_alnum_id(s: &str, prefixes: &[u8]) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 3
        && prefixes.contains(&bytes[0])
        && bytes
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

fn push_text(out: &mut Vec<CanvasInlineSegment>, chunk: &str) {
    if chunk.is_empty() {
        return;
    }
    if let Some(CanvasInlineSegment::Text(prev)) = out.last_mut() {
        prev.push_str(chunk);
    } else {
        out.push(CanvasInlineSegment::Text(chunk.to_owned()));
    }
}

/// Try to parse `<control><a ...>@U...</a></control>` or `<control><a ...>#C...</a></control>`
/// starting at index 0 of `rest`. Returns `(segment, bytes_consumed)`.
fn try_parse_control_mention(rest: &str) -> Option<(CanvasInlineSegment, usize)> {
    let lower = rest.to_ascii_lowercase();
    if !lower.starts_with("<control") {
        return None;
    }
    let close_tag = "</control>";
    let end_rel = lower.find(close_tag)?;
    let consumed = end_rel + close_tag.len();
    let open_gt = rest[..end_rel].find('>')?;
    let inner = &rest[open_gt + 1..end_rel];

    // Extract inner anchor text (and optional href for ID fallback).
    let inner_lower = inner.to_ascii_lowercase();
    let a_start = inner_lower.find("<a")?;
    let a_after = &inner[a_start..];
    let a_gt = a_after.find('>')?;
    let a_open = &a_after[..a_gt];
    let a_body = &a_after[a_gt + 1..];
    let a_end = a_body.to_ascii_lowercase().find("</a>")?;
    let anchor_text = a_body[..a_end].trim();

    if let Some(raw_uid) = anchor_text.strip_prefix('@') {
        let candidate = raw_uid.trim();
        if is_upper_alnum_id(candidate, b"UW") {
            return Some((
                CanvasInlineSegment::UserMention {
                    user_id: candidate.to_owned(),
                    label: None,
                },
                consumed,
            ));
        }
    }
    if let Some(raw_cid) = anchor_text.strip_prefix('#') {
        let candidate = raw_cid.trim();
        if is_upper_alnum_id(candidate, b"CG") {
            return Some((
                CanvasInlineSegment::ChannelMention {
                    channel_id: candidate.to_owned(),
                    name: None,
                },
                consumed,
            ));
        }
    }

    // Fallback: inspect href="/team/U..." or href="/archives/C...".
    if let Some(pos) = a_open.find("/team/") {
        let tail = &a_open[pos + "/team/".len()..];
        let uid: String = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if is_upper_alnum_id(&uid, b"UW") {
            let label = anchor_text.trim_start_matches('@').trim().to_owned();
            let label = (!label.is_empty() && label != uid).then_some(label);
            return Some((
                CanvasInlineSegment::UserMention {
                    user_id: uid,
                    label,
                },
                consumed,
            ));
        }
    }
    if let Some(pos) = a_open.find("/archives/") {
        let tail = &a_open[pos + "/archives/".len()..];
        let cid: String = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if is_upper_alnum_id(&cid, b"CG") {
            let name = anchor_text.trim_start_matches('#').trim().to_owned();
            let name = (!name.is_empty() && name != cid).then_some(name);
            return Some((
                CanvasInlineSegment::ChannelMention {
                    channel_id: cid,
                    name,
                },
                consumed,
            ));
        }
    }

    None
}

/// Try to parse `<@U...>`, `<#C...|name>`, `<!subteam^S...|@team>`, `<!here>`, `<!channel>`,
/// `<!everyone>` starting at index 0 of `rest`. Returns `(segment, bytes_consumed)`.
fn try_parse_angle_mention(rest: &str) -> Option<(CanvasInlineSegment, usize)> {
    if !rest.starts_with('<') {
        return None;
    }
    let gt = rest.find('>')?;
    let inner = &rest[1..gt];
    let consumed = gt + 1;

    if let Some(body) = inner.strip_prefix('@') {
        let mut parts = body.splitn(2, '|');
        let uid = parts.next()?.trim();
        if is_upper_alnum_id(uid, b"UW") {
            let label = parts
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_start_matches('@').to_owned());
            return Some((
                CanvasInlineSegment::UserMention {
                    user_id: uid.to_owned(),
                    label,
                },
                consumed,
            ));
        }
    } else if let Some(body) = inner.strip_prefix('#') {
        let mut parts = body.splitn(2, '|');
        let cid = parts.next()?.trim();
        if is_upper_alnum_id(cid, b"CG") {
            let name = parts
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_start_matches('#').to_owned());
            return Some((
                CanvasInlineSegment::ChannelMention {
                    channel_id: cid.to_owned(),
                    name,
                },
                consumed,
            ));
        }
    } else if let Some(body) = inner.strip_prefix("!subteam^") {
        let mut parts = body.splitn(2, '|');
        let gid = parts.next()?.trim();
        if is_upper_alnum_id(gid, b"S") {
            let handle = parts
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_start_matches('@').to_owned())
                .unwrap_or_else(|| gid.to_owned());
            return Some((
                CanvasInlineSegment::UsergroupMention {
                    group_id: gid.to_owned(),
                    handle,
                },
                consumed,
            ));
        }
    } else if let Some(body) = inner.strip_prefix('!') {
        let kind = body.split('|').next().unwrap_or(body).trim();
        if matches!(kind, "here" | "channel" | "everyone") {
            return Some((
                CanvasInlineSegment::BroadcastMention(kind.to_owned()),
                consumed,
            ));
        }
    }

    None
}

/// Try to parse bare `@here` or `@channel` at a word boundary starting at `idx`.
fn try_parse_bare_broadcast(text: &str, idx: usize) -> Option<(CanvasInlineSegment, usize)> {
    let rest = &text[idx..];
    if !rest.starts_with('@') {
        return None;
    }
    // Must be at start of string or preceded by non-word character (not inside an email).
    if idx > 0 {
        let prev = text[..idx].chars().next_back()?;
        if prev.is_ascii_alphanumeric() || prev == '_' || prev == '.' {
            return None;
        }
    }
    for kw in ["channel", "here"] {
        let candidate = &rest[1..];
        if candidate.len() >= kw.len() && candidate[..kw.len()].eq_ignore_ascii_case(kw) {
            let after = &candidate[kw.len()..];
            let boundary = after
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
            if boundary {
                return Some((
                    CanvasInlineSegment::BroadcastMention(kw.to_owned()),
                    1 + kw.len(),
                ));
            }
        }
    }
    None
}

/// Pure inline tokenizer splitting canvas text into plain runs and mention tokens (`F-COLLAB-150`).
pub fn parse_inline_canvas_segments(text: &str) -> Vec<CanvasInlineSegment> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cursor = 0;
    let mut plain_start = 0;
    let bytes = text.as_bytes();

    while cursor < bytes.len() {
        let b = bytes[cursor];
        if b == b'<' {
            let rest = &text[cursor..];
            if let Some((seg, consumed)) =
                try_parse_control_mention(rest).or_else(|| try_parse_angle_mention(rest))
            {
                push_text(&mut out, &text[plain_start..cursor]);
                out.push(seg);
                cursor += consumed;
                plain_start = cursor;
                continue;
            }
        } else if b == b'@'
            && let Some((seg, consumed)) = try_parse_bare_broadcast(text, cursor)
        {
            push_text(&mut out, &text[plain_start..cursor]);
            out.push(seg);
            cursor += consumed;
            plain_start = cursor;
            continue;
        }

        // Advance one UTF-8 character safely.
        let ch_len = text[cursor..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
        cursor += ch_len;
    }

    push_text(&mut out, &text[plain_start..]);
    out
}

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
