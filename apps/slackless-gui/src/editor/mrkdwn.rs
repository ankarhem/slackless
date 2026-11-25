//! Slack mrkdwn, the text form of a document: the `text` fallback sent beside the rich_text
//! block, and the form a saved draft is kept in. [`Doc::to_mrkdwn`] writes it here and
//! `from_mrkdwn` (in `mrkdwn_read.rs`) reads it back.

use std::borrow::Cow;
use std::ops::Range;

use slackless_core::types::{ChannelId, UserId, UsergroupId};

use super::doc::Doc;
use super::marks::{Atom, LineKind, Style};
use super::parse::{ParsedDoc, Piece};

/// The names a draft's references show as, looked up by the application.
pub trait Directory {
    /// The name an @mention of `id` shows, without the `@`.
    fn mention(&self, id: &UserId) -> String;
    /// The name a #channel reference to `id` shows, without the `#`.
    fn channel_name(&self, id: &ChannelId) -> String;
    /// The handle an @group reference to `id` shows, without the `@`.
    fn usergroup_handle(&self, id: &UsergroupId) -> String {
        id.to_string()
    }
    /// How the emoji `shortcode` shows in a draft: its glyph, or `:name:` for a workspace
    /// custom emoji. `None` when there is no such emoji.
    fn emoji_text(&self, shortcode: &str) -> Option<String>;
}

pub(super) const FENCE: &str = "```";
/// The indent a list item's further lines are written with.
pub(super) const SOFT_INDENT: &str = "    ";
/// Formats written as a marker pair around text, outermost first.
pub(super) const WRAPPERS: [(Style, char); 3] = [
    (Style::BOLD, '*'),
    (Style::ITALIC, '_'),
    (Style::STRIKE, '~'),
];

impl Doc {
    /// The document as Slack mrkdwn, with `&`, `<` and `>` escaped in literal text.
    pub fn to_mrkdwn(&self) -> String {
        self.write_mrkdwn(false)
    }

    /// The document serialized for local draft persistence: identical to [`Self::to_mrkdwn`]
    /// except `Style::UNDERLINE` is preserved as `<u>...</u>` so underlined drafts survive
    /// channel switches and restarts without leaking raw `<u>` tags into Slack's wire fallback.
    pub fn to_draft(&self) -> String {
        self.write_mrkdwn(true)
    }

    fn write_mrkdwn(&self, keep_underline: bool) -> String {
        let parsed = self.parse();
        let mut out = String::with_capacity(self.text().len() + 16);
        let mut in_code = false;
        let mut emitted = 0usize;
        for line in &parsed.lines {
            if line.kind == Some(LineKind::Fence) {
                if in_code {
                    out.push_str(FENCE);
                    in_code = false;
                }
                continue;
            }
            let is_code = line.kind == Some(LineKind::Code);
            if in_code && !is_code {
                out.push_str(FENCE);
            }
            if emitted > 0 {
                out.push('\n');
            }
            if is_code && !in_code {
                out.push_str(FENCE);
            }
            in_code = is_code;
            emitted += 1;
            match line.kind {
                Some(LineKind::Code) => {
                    escape_into(&mut out, &self.text()[line.content.clone()]);
                    continue;
                }
                Some(LineKind::Divider) => {
                    out.push_str("---");
                    continue;
                }
                Some(LineKind::Bullet) => out.push_str("\u{2022} "),
                Some(LineKind::Ordered) => {
                    out.push_str(&self.text()[line.prefix.clone()]);
                }
                Some(LineKind::Soft) => out.push_str(SOFT_INDENT),
                Some(LineKind::Quote) => out.push_str("&gt; "),
                Some(LineKind::Heading(level)) => {
                    for _ in 0..level.max(1) {
                        out.push('#');
                    }
                    out.push(' ');
                }
                Some(LineKind::Fence) | None => {}
            }
            write_inline(
                self,
                &parsed,
                line.content.clone(),
                keep_underline,
                &mut out,
            );
        }
        if in_code {
            out.push_str(FENCE);
        }
        out
    }
}

fn write_inline(
    doc: &Doc,
    parsed: &ParsedDoc,
    range: Range<usize>,
    keep_underline: bool,
    out: &mut String,
) {
    // Wrappers currently open, outermost first.
    let mut open: Vec<Style> = Vec::new();
    for piece in parsed.pieces(doc, range) {
        match piece {
            Piece::Text { text, attrs } => {
                retarget(&mut open, attrs.style, keep_underline, out);
                let is_code = attrs.style.contains(Style::CODE);
                let label = |out: &mut String| {
                    if is_code {
                        out.push('`');
                    }
                    escape_into(out, text);
                    if is_code {
                        out.push('`');
                    }
                };
                match &attrs.link {
                    Some(url) if url.as_ref() == text => {
                        out.push('<');
                        escape_into(out, url);
                        out.push('>');
                    }
                    Some(url) => {
                        out.push('<');
                        escape_into(out, url);
                        out.push('|');
                        label(out);
                        out.push('>');
                    }
                    None => label(out),
                }
            }
            Piece::Atom { atom, style, .. } => {
                retarget(&mut open, style, keep_underline, out);
                let (before, name, after) = match atom {
                    Atom::User(id) => ("<@", id.as_str(), ">"),
                    Atom::Channel(id) => ("<#", id.as_str(), ">"),
                    Atom::Usergroup(id) => ("<!subteam^", id.as_str(), ">"),
                    Atom::Broadcast(range) => ("<!", range.as_str(), ">"),
                    Atom::Emoji(name) => (":", name.as_ref(), ":"),
                };
                out.extend([before, name, after]);
            }
        }
    }
    retarget(&mut open, Style::NONE, keep_underline, out);
}

/// Close and open wrapper markers so that exactly those of `want` are open.
fn retarget(open: &mut Vec<Style>, want: Style, keep_underline: bool, out: &mut String) {
    let open_tag = |style: Style| -> Option<&'static str> {
        match style {
            Style::BOLD => Some("*"),
            Style::ITALIC => Some("_"),
            Style::STRIKE => Some("~"),
            Style::UNDERLINE if keep_underline => Some("<u>"),
            _ => None,
        }
    };
    let close_tag = |style: Style| -> Option<&'static str> {
        match style {
            Style::BOLD => Some("*"),
            Style::ITALIC => Some("_"),
            Style::STRIKE => Some("~"),
            Style::UNDERLINE if keep_underline => Some("</u>"),
            _ => None,
        }
    };
    let keep = open
        .iter()
        .position(|style| !want.contains(*style))
        .unwrap_or(open.len());
    for style in open.drain(keep..).rev() {
        if let Some(tag) = close_tag(style) {
            out.push_str(tag);
        }
    }
    for style in [Style::BOLD, Style::ITALIC, Style::STRIKE, Style::UNDERLINE] {
        if want.contains(style)
            && !open.contains(&style)
            && let Some(tag) = open_tag(style)
        {
            open.push(style);
            out.push_str(tag);
        }
    }
}

fn escape_into(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
}

pub(super) fn unescape(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}
