//! Autocomplete: finding the `@name`, `#channel` or `:emoji` being typed at the caret, and
//! ranking candidates against it.

use std::ops::Range;

use super::doc::Doc;
use super::marks::{LineKind, Style};

/// What a trigger character asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lookup {
    People,
    Channels,
    Emoji,
}

/// A lookup being typed: the trigger character through the caret, and the query after it.
#[derive(Clone, PartialEq, Debug)]
pub struct Trigger<'a> {
    pub lookup: Lookup,
    pub range: Range<usize>,
    pub query: &'a str,
}

/// Characters an emoji shortcode needs before its menu opens, as in Slack.
const EMOJI_QUERY_MIN: usize = 2;

/// The lookup the caret is at the end of, if any. The trigger character must start a word,
/// so `12:30` and `user@example.com` open nothing; code never completes. A `#` at the very
/// start of a line (markdown heading syntax `# ` / `## ` / `### `) does not open channel
/// completion until a word character follows `#`, and `##` never opens channel completion.
pub fn trigger(doc: &Doc, caret: usize) -> Option<Trigger<'_>> {
    let text = doc.text();
    if caret > text.len() || !text.is_char_boundary(caret) {
        return None;
    }
    let parsed = doc.parse();
    let line = parsed.line_at(caret);
    if matches!(
        line.kind,
        Some(LineKind::Code | LineKind::Fence | LineKind::Divider)
    ) || caret < line.content.start
    {
        return None;
    }
    // A query never reaches back over a mention or an emoji.
    let floor = doc
        .atoms()
        .iter()
        .rev()
        .find(|placed| placed.range.end <= caret)
        .map_or(line.content.start, |placed| {
            placed.range.end.max(line.content.start)
        });
    if floor > caret || !text.is_char_boundary(floor) {
        return None;
    }
    let start = floor + text[floor..caret].rfind(['@', '#', ':'])?;
    let query = &text[start + 1..caret];
    let lookup = match &text[start..=start] {
        "@" => Lookup::People,
        "#" => Lookup::Channels,
        _ => Lookup::Emoji,
    };
    // Don't pop up the #channel menu when typing `##` / `###` markdown headings at line start.
    if lookup == Lookup::Channels
        && start > line.start
        && text[line.start..start].bytes().all(|b| b == b'#')
    {
        return None;
    }
    let starts_word = text[floor..start]
        .chars()
        .next_back()
        .is_none_or(|before| before.is_whitespace() || before == '(');
    let is_name = query
        .chars()
        .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | '+'));
    let is_long_enough = lookup != Lookup::Emoji || query.len() >= EMOJI_QUERY_MIN;
    let in_code = parsed.attrs_at(doc, start).style.contains(Style::CODE);
    (starts_word && is_name && is_long_enough && !in_code).then_some(Trigger {
        lookup,
        range: start..caret,
        query,
    })
}

/// How well `name` answers the lowercase `query`, lower being better: 0 when the name starts
/// with it, 1 when a later word does, 2 when it only contains it. `None` when it does not
/// match. Case-insensitive, without allocating.
pub fn rank(name: &str, query: &str) -> Option<u8> {
    let starts_with = |rest: &str| {
        let mut chars = rest.chars().flat_map(char::to_lowercase);
        query.chars().all(|wanted| chars.next() == Some(wanted))
    };
    if starts_with(name) {
        return Some(0);
    }
    let mut found = None;
    for (ix, _) in name.char_indices().skip(1) {
        if !starts_with(&name[ix..]) {
            continue;
        }
        let after_word_break = name[..ix]
            .chars()
            .next_back()
            .is_some_and(|before| !before.is_alphanumeric());
        if after_word_break {
            return Some(1);
        }
        found = Some(2);
    }
    found
}
