//! Slack `mrkdwn` escaping, unescaping, shortcode parsing, plain-text conversion, inline span
//! parsing, and [`Doc`] <-> `mrkdwn` serialization/reading.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;

use crate::types::rich_text::BroadcastRange;
use crate::types::{ChannelId, UserId, UsergroupId};

use super::doc::{Atom, Doc, LineKind, Style};
use super::parse::{ParsedDoc, Piece};

pub(super) const FENCE: &str = "```";
pub(super) const SOFT_INDENT: &str = "    ";
const WRAPPERS: [(Style, char); 3] = [
    (Style::BOLD, '*'),
    (Style::ITALIC, '_'),
    (Style::STRIKE, '~'),
];

/// The names a draft's references show as, looked up by the front-end.
pub trait Directory {
    /// The name an `@mention` of `id` shows, without the `@`.
    fn mention(&self, id: &UserId) -> String;
    /// The name a `#channel` reference to `id` shows, without the `#`.
    fn channel_name(&self, id: &ChannelId) -> String;
    /// The handle an `@group` reference to `id` shows, without the `@`.
    fn usergroup_handle(&self, id: &UsergroupId) -> String {
        id.to_string()
    }
    /// How the emoji `shortcode` shows in a draft: its glyph, or `:name:` for custom emoji.
    fn emoji_text(&self, shortcode: &str) -> Option<String>;
}

/// Default [`Directory`] that echoes raw IDs and keeps `:shortcode:` literal.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlainDirectory;

impl Directory for PlainDirectory {
    fn mention(&self, id: &UserId) -> String {
        id.as_str().to_owned()
    }

    fn channel_name(&self, id: &ChannelId) -> String {
        id.as_str().to_owned()
    }

    fn emoji_text(&self, shortcode: &str) -> Option<String> {
        Some(format!(":{shortcode}:"))
    }
}

/// Escape Slack's three mrkdwn control characters (`&`, `<`, `>`).
pub fn escape_mrkdwn(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    escape_into(&mut out, text);
    out
}

/// Undo Slack's three mrkdwn HTML escapes (`&lt;`, `&gt;`, `&amp;`), zero-copy when no `&` is present.
pub fn unescape_mrkdwn(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}

/// Parse a `:shortcode:` (optionally followed by `:skin-tone-N:`) at the start of `rest`,
/// returning `(bytes_consumed, normalized_shortcode)`.
pub fn parse_shortcode(rest: &str) -> Option<(usize, String)> {
    let tail = rest.strip_prefix(':')?;
    let end = tail.find(':')?;
    let name = &tail[..end];
    let valid = !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '+' | '-'));
    if !valid {
        return None;
    }
    let len = end + 2;
    let after = &rest[len..];
    if let Some(tone) = after
        .strip_prefix(":skin-tone-")
        .and_then(|tone| tone.get(..2))
        .filter(|tone| tone.ends_with(':'))
    {
        return Some((
            len + ":skin-tone-".len() + 2,
            format!("{name}::skin-tone-{}", &tone[..1]),
        ));
    }
    Some((len, name.to_owned()))
}

/// Replace known `:shortcode:` tokens in `text` using `resolve`.
pub fn replace_shortcodes_with(text: &str, resolve: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(':') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        match parse_shortcode(rest).and_then(|(len, code)| Some((len, resolve(&code)?))) {
            Some((len, glyph)) => {
                out.push_str(&glyph);
                rest = &rest[len..];
            }
            None => {
                out.push(':');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// One inline span parsed from raw Slack `mrkdwn`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MrkdwnSpan {
    Text {
        text: String,
        style: Style,
    },
    Link {
        url: String,
        label: Option<String>,
        style: Style,
    },
    UserMention {
        id: UserId,
        label: Option<String>,
        style: Style,
    },
    ChannelMention {
        id: ChannelId,
        name: Option<String>,
        style: Style,
    },
    UsergroupMention {
        id: UsergroupId,
        handle: Option<String>,
        style: Style,
    },
    Broadcast {
        range: BroadcastRange,
        style: Style,
    },
    Emoji {
        shortcode: String,
        style: Style,
    },
    Date {
        timestamp: i64,
        format: String,
        fallback: String,
    },
}

/// Parse inline Slack `mrkdwn` into structured [`MrkdwnSpan`]s (bold, italic, strike, code,
/// links, user/channel/usergroup/broadcast mentions, emoji shortcodes, and `<!date^...>` tokens).
pub fn parse_mrkdwn_spans(text: &str) -> Vec<MrkdwnSpan> {
    let mut out = Vec::new();
    parse_spans_range(text, 0..text.len(), Style::NONE, &mut out);
    out
}

/// Convert raw Slack `mrkdwn` into clean, readable plain text (for CLI output, previews, and search).
pub fn mrkdwn_to_plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for span in parse_mrkdwn_spans(text) {
        match span {
            MrkdwnSpan::Text { text, .. } => out.push_str(&text),
            MrkdwnSpan::Link { url, label, .. } => {
                out.push_str(label.as_deref().unwrap_or(&url));
            }
            MrkdwnSpan::UserMention { id, label, .. } => {
                out.push('@');
                out.push_str(label.as_deref().unwrap_or(id.as_str()));
            }
            MrkdwnSpan::ChannelMention { id, name, .. } => {
                out.push('#');
                out.push_str(name.as_deref().unwrap_or(id.as_str()));
            }
            MrkdwnSpan::UsergroupMention { id, handle, .. } => {
                let shown = handle.as_deref().unwrap_or(id.as_str());
                if !shown.starts_with('@') {
                    out.push('@');
                }
                out.push_str(shown);
            }
            MrkdwnSpan::Broadcast { range, .. } => {
                out.push('@');
                out.push_str(range.as_str());
            }
            MrkdwnSpan::Emoji { shortcode, .. } => {
                out.push(':');
                out.push_str(&shortcode);
                out.push(':');
            }
            MrkdwnSpan::Date { fallback, .. } => out.push_str(&fallback),
        }
    }
    out
}

fn push_span_text(out: &mut Vec<MrkdwnSpan>, chunk: &str, style: Style) {
    if chunk.is_empty() {
        return;
    }
    if let Some(MrkdwnSpan::Text { text, style: prev }) = out.last_mut()
        && *prev == style
    {
        text.push_str(chunk);
        return;
    }
    out.push(MrkdwnSpan::Text {
        text: chunk.to_owned(),
        style,
    });
}

fn parse_spans_range(line: &str, range: Range<usize>, active: Style, out: &mut Vec<MrkdwnSpan>) {
    let mut literal = range.start;
    let mut at = range.start;
    while at < range.end {
        let byte = line.as_bytes()[at];
        if !matches!(byte, b'<' | b'&' | b':' | b'*' | b'_' | b'~' | b'`') {
            at += 1;
            continue;
        }
        push_span_text(out, &line[literal..at], active);
        let here = at..range.end;
        let consumed = match byte {
            b'<' => parse_span_reference(line, here, active, out),
            b'&' => parse_span_entity(&line[here], active, out),
            b':' => parse_span_emoji(&line[here], active, out),
            b'`' => parse_span_code(&line[here], active, out),
            _ => parse_span_wrapped(line, here, active, out),
        };
        if let Some(len) = consumed {
            literal = at + len;
            at = literal;
        } else {
            literal = at;
            at += 1;
        }
    }
    push_span_text(out, &line[literal..range.end], active);
}

fn parse_span_reference(
    line: &str,
    range: Range<usize>,
    active: Style,
    out: &mut Vec<MrkdwnSpan>,
) -> Option<usize> {
    let close = line[range.clone()].find('>')?;
    let body = &line[range.start + 1..range.start + close];
    let (target, label) = match body.split_once('|') {
        Some((t, l)) => (t, Some(l)),
        None => (body, None),
    };
    if let Some(id) = target.strip_prefix('@') {
        out.push(MrkdwnSpan::UserMention {
            id: UserId::from(id),
            label: label.map(|l| unescape_mrkdwn(l).into_owned()),
            style: active,
        });
    } else if let Some(id) = target.strip_prefix('#') {
        out.push(MrkdwnSpan::ChannelMention {
            id: ChannelId::from(id),
            name: label.map(|l| unescape_mrkdwn(l).into_owned()),
            style: active,
        });
    } else if let Some(id) = target.strip_prefix("!subteam^") {
        out.push(MrkdwnSpan::UsergroupMention {
            id: UsergroupId::from(id),
            handle: label.map(|l| unescape_mrkdwn(l).into_owned()),
            style: active,
        });
    } else if let Some(rest) = target.strip_prefix("!date^") {
        let mut parts = rest.split('^');
        let timestamp = parts.next()?.parse::<i64>().ok()?;
        let format = parts.next().unwrap_or_default().to_owned();
        let fallback = label
            .map(|l| unescape_mrkdwn(l).into_owned())
            .unwrap_or_else(|| format.clone());
        out.push(MrkdwnSpan::Date {
            timestamp,
            format,
            fallback,
        });
    } else if let Some(name) = target.strip_prefix('!') {
        let br = match BroadcastRange::from(name) {
            BroadcastRange::Other(_) => return None,
            known => known,
        };
        out.push(MrkdwnSpan::Broadcast {
            range: br,
            style: active,
        });
    } else if target.contains("://") || target.starts_with("mailto:") {
        out.push(MrkdwnSpan::Link {
            url: unescape_mrkdwn(target).into_owned(),
            label: label.map(|l| unescape_mrkdwn(l).into_owned()),
            style: active,
        });
    } else {
        return None;
    }
    Some(close + 1)
}

fn parse_span_entity(rest: &str, active: Style, out: &mut Vec<MrkdwnSpan>) -> Option<usize> {
    let (entity, ch) = [("&amp;", "&"), ("&lt;", "<"), ("&gt;", ">")]
        .into_iter()
        .find(|(entity, _)| rest.starts_with(entity))?;
    push_span_text(out, ch, active);
    Some(entity.len())
}

fn parse_span_emoji(rest: &str, active: Style, out: &mut Vec<MrkdwnSpan>) -> Option<usize> {
    let (len, shortcode) = parse_shortcode(rest)?;
    out.push(MrkdwnSpan::Emoji {
        shortcode,
        style: active,
    });
    Some(len)
}

fn parse_span_code(rest: &str, active: Style, out: &mut Vec<MrkdwnSpan>) -> Option<usize> {
    let close = rest[1..].find('`').filter(|len| *len > 0)? + 1;
    let decoded = unescape_mrkdwn(&rest[1..close]);
    push_span_text(out, &decoded, active.with(Style::CODE));
    Some(close + 1)
}

fn parse_span_wrapped(
    line: &str,
    range: Range<usize>,
    active: Style,
    out: &mut Vec<MrkdwnSpan>,
) -> Option<usize> {
    let marker = line[range.clone()].chars().next()?;
    let (style, _) = WRAPPERS.into_iter().find(|(_, m)| *m == marker)?;
    let is_word = |ch: Option<char>| ch.is_some_and(char::is_alphanumeric);
    let inner_start = range.start + 1;
    let opens = !active.contains(style)
        && !is_word(line[..range.start].chars().next_back())
        && line[inner_start..range.end]
            .chars()
            .next()
            .is_some_and(|ch| !ch.is_whitespace() && ch != marker);
    if !opens {
        return None;
    }
    let close = line[inner_start..range.end]
        .match_indices(marker)
        .map(|(ix, _)| inner_start + ix)
        .find(|&at| {
            line[inner_start..at]
                .chars()
                .next_back()
                .is_some_and(|before| !before.is_whitespace())
                && !is_word(line[at + 1..range.end].chars().next())
        })?;
    parse_spans_range(line, inner_start..close, active.with(style), out);
    Some(close + 1 - range.start)
}

impl Doc {
    /// The document as Slack mrkdwn, with `&`, `<` and `>` escaped in literal text.
    pub fn to_mrkdwn(&self) -> String {
        self.write_mrkdwn(false)
    }

    /// The document serialized for local draft persistence (`Style::UNDERLINE` preserved as `<u>...</u>`).
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

/// Read a [`Doc`] back from Slack `mrkdwn` into clean editable Markdown + atoms.
pub fn from_mrkdwn(text: &str, names: &dyn Directory) -> Doc {
    let mut reader = Reader {
        doc: Doc::default(),
        names,
        lines: 0,
    };
    let mut in_code = false;
    let mut list = List::default();
    for raw in text.split('\n') {
        if in_code {
            let (continues, tail) = reader.code_line(raw);
            in_code = continues;
            if !in_code {
                reader.begin_line("```");
                if let Some(tail) = tail.filter(|t| !t.is_empty()) {
                    reader.begin_line("");
                    reader.inline(tail, 0..tail.len(), Style::NONE);
                }
            }
            continue;
        }
        let kind = line_kind(raw, &list);
        list = match kind {
            Some((LineKind::Ordered, _)) => List {
                is_open: true,
                number: list.number + 1,
            },
            Some((LineKind::Soft, _)) => list,
            Some((LineKind::Bullet, _)) => List {
                is_open: true,
                number: 0,
            },
            _ => List::default(),
        };
        match kind {
            Some((LineKind::Code, prefix)) => {
                reader.begin_line("```");
                let (continues, tail) = reader.code_line(&raw[prefix..]);
                in_code = continues;
                if !in_code {
                    reader.begin_line("```");
                    if let Some(tail) = tail.filter(|t| !t.is_empty()) {
                        reader.begin_line("");
                        reader.inline(tail, 0..tail.len(), Style::NONE);
                    }
                }
            }
            Some((LineKind::Divider, _)) => reader.begin_line("---"),
            Some((LineKind::Quote, prefix)) => {
                reader.begin_line("> ");
                reader.inline(raw, prefix..raw.len(), Style::NONE);
            }
            Some((LineKind::Heading(level), prefix)) => {
                reader.begin_line(LineKind::Heading(level).markdown_prefix());
                reader.inline(raw, prefix..raw.len(), Style::NONE);
            }
            Some((LineKind::Soft, prefix)) => {
                reader.begin_line(SOFT_INDENT);
                reader.inline(raw, prefix..raw.len(), Style::NONE);
            }
            Some((LineKind::Bullet, prefix)) => {
                reader.begin_line("- ");
                reader.inline(raw, prefix..raw.len(), Style::NONE);
            }
            Some((LineKind::Ordered, prefix)) => {
                reader.begin_line(&raw[..prefix]);
                reader.inline(raw, prefix..raw.len(), Style::NONE);
            }
            Some((LineKind::Fence, _)) | None => {
                reader.begin_line("");
                reader.inline(raw, 0..raw.len(), Style::NONE);
            }
        }
    }
    if in_code {
        reader.begin_line("```");
    }
    reader.doc
}

#[derive(Default)]
struct List {
    is_open: bool,
    number: u32,
}

fn line_kind(raw: &str, list: &List) -> Option<(LineKind, usize)> {
    if raw == "---" {
        return Some((LineKind::Divider, 3));
    }
    if raw.starts_with(FENCE) {
        return Some((LineKind::Code, FENCE.len()));
    }
    if raw.starts_with("&gt; ") {
        return Some((LineKind::Quote, "&gt; ".len()));
    }
    if raw.starts_with("> ") {
        return Some((LineKind::Quote, 2));
    }
    for (level, marker) in [(3u8, "### "), (2, "## "), (1, "# ")] {
        if raw.starts_with(marker) {
            return Some((LineKind::Heading(level), marker.len()));
        }
    }
    for bullet in ["\u{2022} ", "- "] {
        if raw.starts_with(bullet) {
            return Some((LineKind::Bullet, bullet.len()));
        }
    }
    if list.is_open && raw.starts_with(SOFT_INDENT) {
        return Some((LineKind::Soft, SOFT_INDENT.len()));
    }
    let digits = raw.bytes().take_while(u8::is_ascii_digit).count();
    let continues = raw[..digits].parse() == Ok(list.number + 1);
    (continues && raw[digits..].starts_with(". ")).then_some((LineKind::Ordered, digits + 2))
}

struct Reader<'a> {
    doc: Doc,
    names: &'a dyn Directory,
    lines: usize,
}

impl Reader<'_> {
    fn begin_line(&mut self, prefix: &str) {
        if self.lines > 0 {
            self.doc.append("\n");
        }
        self.lines += 1;
        self.doc.append(prefix);
    }

    fn code_line<'a>(&mut self, raw: &'a str) -> (bool, Option<&'a str>) {
        let (code, continues, tail) = raw
            .split_once(FENCE)
            .map_or((raw, true, None), |(code, rest)| (code, false, Some(rest)));
        self.begin_line("");
        self.doc.append(&unescape_mrkdwn(code));
        (continues, tail)
    }

    fn inline(&mut self, line: &str, range: Range<usize>, active: Style) {
        let mut literal = range.start;
        let mut at = range.start;
        while at < range.end {
            let byte = line.as_bytes()[at];
            if !matches!(byte, b'<' | b'&' | b':' | b'*' | b'_' | b'~' | b'`') {
                at += 1;
                continue;
            }
            self.doc.append(&line[literal..at]);
            let here = at..range.end;
            let consumed = match byte {
                b'<' => self
                    .underline(line, here.clone(), active)
                    .or_else(|| self.reference(line, here, active)),
                b'&' => self.entity(&line[here]),
                b':' => self.emoji(&line[here]),
                b'`' => self.code(&line[here]),
                _ => self.wrapped(line, here, active),
            };
            literal = at + consumed.unwrap_or(0);
            at = literal.max(at + 1);
        }
        self.doc.append(&line[literal..range.end]);
    }

    fn underline(&mut self, line: &str, range: Range<usize>, active: Style) -> Option<usize> {
        if active.contains(Style::UNDERLINE) || !line[range.clone()].starts_with("<u>") {
            return None;
        }
        let inner_start = range.start + 3;
        let rel_close = line[inner_start..range.end].find("</u>")?;
        if rel_close == 0 {
            return None;
        }
        let close = inner_start + rel_close;
        self.doc.append("<u>");
        self.inline(line, inner_start..close, active.with(Style::UNDERLINE));
        self.doc.append("</u>");
        Some(close + 4 - range.start)
    }

    fn reference(&mut self, line: &str, range: Range<usize>, active: Style) -> Option<usize> {
        let close = line[range.clone()].find('>')?;
        let inner = range.start + 1..range.start + close;
        let body = &line[inner.clone()];
        let (target, label) = match body.split_once('|') {
            Some((target, _)) => (target, Some(inner.start + target.len() + 1..inner.end)),
            None => (body, None),
        };
        if let Some(id) = target.strip_prefix('@') {
            let id = UserId::from(id);
            let name = self.names.mention(&id);
            self.doc.append_atom(Atom::User(id), &format!("@{name}"));
        } else if let Some(id) = target.strip_prefix('#') {
            let id = ChannelId::from(id);
            let name = self.names.channel_name(&id);
            self.doc.append_atom(Atom::Channel(id), &format!("#{name}"));
        } else if let Some(rest) = target.strip_prefix("!subteam^") {
            let id = UsergroupId::from(rest);
            let shown = match label {
                Some(label_range) => {
                    let text = &line[label_range];
                    if text.starts_with('@') {
                        text.to_owned()
                    } else {
                        format!("@{text}")
                    }
                }
                None => format!("@{}", self.names.usergroup_handle(&id)),
            };
            self.doc.append_atom(Atom::Usergroup(id), &shown);
        } else if let Some(name) = target.strip_prefix('!') {
            let range = match BroadcastRange::from(name) {
                BroadcastRange::Other(_) => return None,
                known => known,
            };
            self.doc
                .append_atom(Atom::Broadcast(range), &format!("@{name}"));
        } else if target.contains("://") || target.starts_with("mailto:") {
            let url = unescape_mrkdwn(target);
            match label {
                Some(label_range) => {
                    self.doc.append("[");
                    self.inline(line, label_range, active);
                    self.doc.append("](");
                    self.doc.append(&url);
                    self.doc.append(")");
                }
                None => {
                    self.doc.append("[");
                    self.doc.append(&url);
                    self.doc.append("](");
                    self.doc.append(&url);
                    self.doc.append(")");
                }
            }
        } else {
            return None;
        }
        Some(close + 1)
    }

    fn entity(&mut self, rest: &str) -> Option<usize> {
        let (entity, ch) = [("&amp;", "&"), ("&lt;", "<"), ("&gt;", ">")]
            .into_iter()
            .find(|(entity, _)| rest.starts_with(entity))?;
        self.doc.append(ch);
        Some(entity.len())
    }

    fn emoji(&mut self, rest: &str) -> Option<usize> {
        let (len, shortcode) = parse_shortcode(rest)?;
        let shown = self.names.emoji_text(&shortcode)?;
        self.doc
            .append_atom(Atom::Emoji(Arc::from(shortcode)), &shown);
        Some(len)
    }

    fn code(&mut self, rest: &str) -> Option<usize> {
        let close = rest[1..].find('`').filter(|len| *len > 0)? + 1;
        self.doc.append("`");
        self.doc.append(&unescape_mrkdwn(&rest[1..close]));
        self.doc.append("`");
        Some(close + 1)
    }

    fn wrapped(&mut self, line: &str, range: Range<usize>, active: Style) -> Option<usize> {
        let marker = line[range.clone()].chars().next()?;
        let (style, _) = WRAPPERS.into_iter().find(|(_, m)| *m == marker)?;
        let is_word = |ch: Option<char>| ch.is_some_and(char::is_alphanumeric);
        let inner_start = range.start + 1;
        let opens = !active.contains(style)
            && !is_word(line[..range.start].chars().next_back())
            && line[inner_start..range.end]
                .chars()
                .next()
                .is_some_and(|ch| !ch.is_whitespace() && ch != marker);
        if !opens {
            return None;
        }
        let close = line[inner_start..range.end]
            .match_indices(marker)
            .map(|(ix, _)| inner_start + ix)
            .find(|&at| {
                line[inner_start..at]
                    .chars()
                    .next_back()
                    .is_some_and(|before| !before.is_whitespace())
                    && !is_word(line[at + 1..range.end].chars().next())
            })?;
        let mut buf = [0u8; 4];
        let m = marker.encode_utf8(&mut buf);
        self.doc.append(m);
        self.inline(line, inner_start..close, active.with(style));
        self.doc.append(m);
        Some(close + 1 - range.start)
    }
}
