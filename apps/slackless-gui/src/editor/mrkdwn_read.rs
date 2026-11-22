//! Reading a draft back from the mrkdwn `Doc::to_mrkdwn` writes.

use std::ops::Range;
use std::sync::Arc;

use slackless_core::types::rich_text::BroadcastRange;
use slackless_core::types::{ChannelId, UserId};

use super::doc::Doc;
use super::marks::{Atom, Attrs, LineKind, PLAIN, Style};
use super::mrkdwn::{Directory, FENCE, SOFT_INDENT, WRAPPERS, unescape};

/// Read a document back from the mrkdwn [`Doc::to_mrkdwn`] writes. Anything that is not
/// valid markup stays literal text.
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
            in_code = reader.code_line(raw);
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
            Some((LineKind::Quote | LineKind::Code | LineKind::Heading(_), _)) | None => {
                List::default()
            }
        };
        match kind {
            Some((LineKind::Code, prefix)) => in_code = reader.code_line(&raw[prefix..]),
            Some((LineKind::Quote, prefix)) => {
                reader.begin_line(Some(LineKind::Quote), LineKind::Quote.marker_text());
                reader.inline(raw, prefix..raw.len(), &PLAIN);
            }
            Some((LineKind::Heading(level), prefix)) => {
                reader.begin_line(
                    Some(LineKind::Heading(level)),
                    LineKind::Heading(level).marker_text(),
                );
                reader.inline(raw, prefix..raw.len(), &PLAIN);
            }
            Some((LineKind::Soft, prefix)) => {
                reader.begin_line(Some(LineKind::Soft), LineKind::Soft.marker_text());
                reader.inline(raw, prefix..raw.len(), &PLAIN);
            }
            Some((kind @ (LineKind::Bullet | LineKind::Ordered), prefix)) => {
                reader.begin_line(Some(kind), &raw[..prefix]);
                reader.inline(raw, prefix..raw.len(), &PLAIN);
            }
            None => {
                reader.begin_line(None, "");
                reader.inline(raw, 0..raw.len(), &PLAIN);
            }
        }
    }
    reader.doc
}

/// The list the lines read so far have left open.
#[derive(Default)]
struct List {
    is_open: bool,
    /// The number of the last ordered item; zero outside an ordered list.
    number: u32,
}

/// The block a mrkdwn line opens with and the length of its prefix. A numbered line is an
/// ordered item only when its number continues `list`, so a paragraph that happens to start
/// with `3. ` stays a paragraph.
fn line_kind(raw: &str, list: &List) -> Option<(LineKind, usize)> {
    if raw.starts_with(FENCE) {
        return Some((LineKind::Code, FENCE.len()));
    }
    if raw.starts_with("&gt; ") {
        return Some((LineKind::Quote, "&gt; ".len()));
    }
    for level in 1..=3 {
        let marker = "#".repeat(level as usize) + " ";
        if raw.starts_with(&marker) {
            return Some((LineKind::Heading(level), marker.len()));
        }
    }
    if raw.starts_with(LineKind::Bullet.marker_text()) {
        return Some((LineKind::Bullet, LineKind::Bullet.marker_text().len()));
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
    fn begin_line(&mut self, kind: Option<LineKind>, marker: &str) {
        if self.lines > 0 {
            self.doc.append("\n", &PLAIN);
        }
        self.lines += 1;
        if let Some(kind) = kind {
            self.doc.append_atom(Atom::Marker(kind), marker);
        }
    }

    /// A code-block line, up to the closing fence when it has one. Returns whether the block
    /// continues on the next line.
    fn code_line(&mut self, raw: &str) -> bool {
        let (code, continues) = raw
            .split_once(FENCE)
            .map_or((raw, true), |(code, _)| (code, false));
        self.begin_line(Some(LineKind::Code), LineKind::Code.marker_text());
        self.doc.append(&unescape(code), &PLAIN);
        continues
    }

    /// Read `line[range]` as inline markup carrying `attrs`.
    fn inline(&mut self, line: &str, range: Range<usize>, attrs: &Attrs) {
        let mut literal = range.start;
        let mut at = range.start;
        while at < range.end {
            let byte = line.as_bytes()[at];
            if !matches!(byte, b'<' | b'&' | b':' | b'*' | b'_' | b'~' | b'`') {
                at += 1;
                continue;
            }
            self.doc.append(&line[literal..at], attrs);
            let here = at..range.end;
            let consumed = match byte {
                b'<' => self.reference(line, here, attrs),
                b'&' => self.entity(&line[here], attrs),
                b':' => self.emoji(&line[here]),
                b'`' => self.code(&line[here], attrs),
                _ => self.wrapped(line, here, attrs),
            };
            literal = at + consumed.unwrap_or(0);
            at = literal.max(at + 1);
        }
        self.doc.append(&line[literal..range.end], attrs);
    }

    /// `<@U1>`, `<#C1|name>`, `<!here>` or `<url|label>` at the start of `line[range]`.
    fn reference(&mut self, line: &str, range: Range<usize>, attrs: &Attrs) -> Option<usize> {
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
        } else if let Some(name) = target.strip_prefix('!') {
            let range = match BroadcastRange::from(name) {
                BroadcastRange::Other(_) => return None,
                known => known,
            };
            self.doc
                .append_atom(Atom::Broadcast(range), &format!("@{name}"));
        } else if target.contains("://") || target.starts_with("mailto:") {
            let url = unescape(target);
            let linked = Attrs {
                style: attrs.style,
                link: Some(Arc::from(url.as_ref())),
            };
            match label {
                Some(label) => self.inline(line, label, &linked),
                None => self.doc.append(&url, &linked),
            }
        } else {
            return None;
        }
        Some(close + 1)
    }

    fn entity(&mut self, rest: &str, attrs: &Attrs) -> Option<usize> {
        let (entity, ch) = [("&amp;", "&"), ("&lt;", "<"), ("&gt;", ">")]
            .into_iter()
            .find(|(entity, _)| rest.starts_with(entity))?;
        self.doc.append(ch, attrs);
        Some(entity.len())
    }

    fn emoji(&mut self, rest: &str) -> Option<usize> {
        let (len, shortcode) = crate::emoji::parse_shortcode(rest)?;
        let shown = self.names.emoji_text(&shortcode)?;
        self.doc
            .append_atom(Atom::Emoji(Arc::from(shortcode)), &shown);
        Some(len)
    }

    fn code(&mut self, rest: &str, attrs: &Attrs) -> Option<usize> {
        let close = rest[1..].find('`').filter(|len| *len > 0)? + 1;
        let code = Attrs {
            style: attrs.style.with(Style::CODE),
            link: attrs.link.clone(),
        };
        self.doc.append(&unescape(&rest[1..close]), &code);
        Some(close + 1)
    }

    /// A `*bold*`, `_italic_` or `~strike~` pair opening at the start of `line[range]`. The
    /// opening marker must start a word and the closing one end it, as Slack reads mrkdwn.
    fn wrapped(&mut self, line: &str, range: Range<usize>, attrs: &Attrs) -> Option<usize> {
        let marker = line[range.clone()].chars().next()?;
        let (style, _) = WRAPPERS.into_iter().find(|(_, m)| *m == marker)?;
        let is_word = |ch: Option<char>| ch.is_some_and(char::is_alphanumeric);
        let inner_start = range.start + 1;
        let opens = !attrs.style.contains(style)
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
            .find(|close| {
                !line[..*close].ends_with(char::is_whitespace)
                    && !is_word(line[close + 1..].chars().next())
            })?;
        let inner = Attrs {
            style: attrs.style.with(style),
            link: attrs.link.clone(),
        };
        self.inline(line, inner_start..close, &inner);
        Some(close + 1 - range.start)
    }
}
