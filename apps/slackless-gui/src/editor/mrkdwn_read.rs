//! Reading Slack mrkdwn (saved drafts, fallback message text) into an Obsidian Live Preview
//! markdown [`Doc`]: Slack-specific wire syntax (`<@U1>`, `<#C1|name>`, `<!here>`,
//! `<url|label>`, `&gt; `, `• `, `:emoji:`, inline ```` ```code``` ````) becomes clean
//! editable markdown plus semantic atoms.

use std::ops::Range;
use std::sync::Arc;

use slackless_core::types::rich_text::BroadcastRange;
use slackless_core::types::{ChannelId, UserId, UsergroupId};

use super::doc::Doc;
use super::marks::{Atom, LineKind, Style};
use super::mrkdwn::{Directory, FENCE, SOFT_INDENT, WRAPPERS, unescape};

/// Read a document back from Slack mrkdwn into clean editable Markdown + atoms. Anything
/// that is not valid markup stays literal text.
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

    /// A code-block line, up to the closing fence when it has one. Returns `(continues, tail)`
    /// where `tail` is any text following a closing fence on the same line.
    fn code_line<'a>(&mut self, raw: &'a str) -> (bool, Option<&'a str>) {
        let (code, continues, tail) = raw
            .split_once(FENCE)
            .map_or((raw, true, None), |(code, rest)| (code, false, Some(rest)));
        self.begin_line("");
        self.doc.append(&unescape(code));
        (continues, tail)
    }

    /// Read `line[range]` as Slack mrkdwn inline markup and emit clean Markdown + atoms.
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
            let url = unescape(target);
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
        let (len, shortcode) = crate::emoji::parse_shortcode(rest)?;
        let shown = self.names.emoji_text(&shortcode)?;
        self.doc
            .append_atom(Atom::Emoji(Arc::from(shortcode)), &shown);
        Some(len)
    }

    fn code(&mut self, rest: &str) -> Option<usize> {
        let close = rest[1..].find('`').filter(|len| *len > 0)? + 1;
        self.doc.append("`");
        self.doc.append(&unescape(&rest[1..close]));
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
