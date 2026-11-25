//! Pure, deterministic Obsidian / Slack Markdown parser for [`Doc`].
//!
//! Every visual feature of Live Preview (block prefixes, inline delimiters to hide or mute,
//! text decorations, toolbar active state) and every export (`to_rich_text`, `to_mrkdwn`)
//! derives from [`ParsedDoc`], so what the user edits and what Slack receives can never
//! disagree.

use std::ops::Range;
use std::sync::Arc;

use super::doc::Doc;
use super::marks::{Atom, Attrs, LineKind, PLAIN, Placed, Style};

/// What an [`InlineConstruct`] applies to its inner range.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum InlineKind {
    Style(Style),
    Link(Arc<str>),
}

/// One matched inline markdown construct (`*bold*`, `**bold**`, `_italic_`, `~strike~`,
/// `~~strike~~`, `` `code` ``, `<u>underline</u>`, or `[label](url)`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InlineConstruct {
    /// Full byte range including opening and closing syntax.
    pub outer: Range<usize>,
    /// Opening delimiter range (`*`, `**`, `_`, `~`, `~~`, `` ` ``, `<u>`, `[`).
    pub open: Range<usize>,
    /// Formatted content range inside the delimiters.
    pub inner: Range<usize>,
    /// Closing delimiter range (`*`, `**`, `_`, `~`, `~~`, `` ` ``, `</u>`, `](url)`).
    pub close: Range<usize>,
    pub kind: InlineKind,
}

/// One parsed line of a [`Doc`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParsedLine {
    /// Byte offset where this line starts.
    pub start: usize,
    /// Byte offset where this line ends (newline excluded).
    pub end: usize,
    /// Block kind (`None` for a plain paragraph).
    pub kind: Option<LineKind>,
    /// Byte range of the markdown block prefix (`# `, `- `, `1. `, `> `, `    `, `---`,
    /// ```` ``` ````); empty for plain paragraphs and code-block content lines.
    pub prefix: Range<usize>,
    /// Byte range of the line's content after `prefix` (empty for `Divider` / `Fence`).
    pub content: Range<usize>,
    /// Inline markdown constructs inside `content`, outer before inner.
    pub inlines: Vec<InlineConstruct>,
}

/// A uniform stretch of rendered content (delimiters stripped): styled text or one atom.
#[derive(Clone, PartialEq, Debug)]
pub enum Piece<'a> {
    Text {
        text: &'a str,
        attrs: Attrs,
    },
    Atom {
        atom: &'a Atom,
        text: &'a str,
        style: Style,
    },
}

/// A styled subrange of the source text (used to build decorations and read active style).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StyledRun {
    pub range: Range<usize>,
    pub attrs: Attrs,
}

/// Complete parsed view of a [`Doc`].
#[derive(Clone, Debug)]
pub struct ParsedDoc {
    pub lines: Vec<ParsedLine>,
}

impl Doc {
    /// Parse this document's markdown blocks and inline constructs.
    pub fn parse(&self) -> ParsedDoc {
        let text = self.text();
        let atoms = self.atoms();
        let mut lines = Vec::new();
        let mut in_code = false;
        let mut list = ListState::default();
        let mut offset = 0usize;

        for raw in text.split('\n') {
            let start = offset;
            let end = start + raw.len();
            offset = end + 1;

            if is_fence_line(raw, start, atoms) {
                in_code = !in_code;
                list = ListState::default();
                lines.push(ParsedLine {
                    start,
                    end,
                    kind: Some(LineKind::Fence),
                    prefix: start..end,
                    content: end..end,
                    inlines: Vec::new(),
                });
                continue;
            }

            if in_code {
                lines.push(ParsedLine {
                    start,
                    end,
                    kind: Some(LineKind::Code),
                    prefix: start..start,
                    content: start..end,
                    inlines: Vec::new(),
                });
                continue;
            }

            let (kind, prefix_len) = detect_block(raw, start, atoms, &list);
            list = match kind {
                Some(LineKind::Ordered) => ListState {
                    is_open: true,
                    ordered_num: list.ordered_num + 1,
                },
                Some(LineKind::Bullet) => ListState {
                    is_open: true,
                    ordered_num: 0,
                },
                Some(LineKind::Soft) => list,
                _ => ListState::default(),
            };

            let prefix = start..start + prefix_len;
            let content = if kind == Some(LineKind::Divider) {
                end..end
            } else {
                prefix.end..end
            };
            let mut inlines = Vec::new();
            if kind != Some(LineKind::Divider) && !content.is_empty() {
                parse_inlines(
                    text,
                    content.clone(),
                    atoms,
                    Style::NONE,
                    false,
                    &mut inlines,
                );
            }
            lines.push(ParsedLine {
                start,
                end,
                kind,
                prefix,
                content,
                inlines,
            });
        }

        ParsedDoc { lines }
    }
}

impl ParsedDoc {
    /// The parsed line containing `offset` (clamped to the document's bounds).
    pub fn line_at(&self, offset: usize) -> &ParsedLine {
        let ix = self
            .lines
            .partition_point(|line| line.end < offset)
            .min(self.lines.len().saturating_sub(1));
        &self.lines[ix]
    }

    /// Attributes active at `offset` (including when the caret sits inside a construct or
    /// between an empty pair like `*|*`).
    pub fn attrs_at(&self, doc: &Doc, offset: usize) -> Attrs {
        let line = self.line_at(offset);
        if matches!(
            line.kind,
            Some(LineKind::Code | LineKind::Fence | LineKind::Divider)
        ) {
            return PLAIN.clone();
        }
        let mut attrs = Attrs::default();
        for c in &line.inlines {
            if offset >= c.inner.start && offset <= c.inner.end {
                match &c.kind {
                    InlineKind::Style(style) => attrs.style = attrs.style.with(*style),
                    InlineKind::Link(url) => attrs.link = Some(url.clone()),
                }
            }
        }
        // Empty pair right around the caret (e.g. `*|*`, `_|_`, `~|~`, `` `|` ``, `<u>|</u>`)
        // does not parse as a non-empty construct, yet the user just armed that format.
        if attrs.style.is_plain()
            && let Some((_, style)) = empty_pair_at(doc.text(), line, offset)
        {
            attrs.style = style;
        }
        attrs
    }

    /// Formats shared by all non-delimiter text (or styled atoms when no text is selected)
    /// inside `range`.
    pub fn common_style(&self, doc: &Doc, range: Range<usize>) -> Style {
        let mut text_styles = Vec::new();
        let mut atom_styles = Vec::new();
        for line in &self.lines {
            if line.end < range.start || line.start > range.end {
                continue;
            }
            let slice = line.content.start.max(range.start)..line.content.end.min(range.end);
            if slice.is_empty() {
                continue;
            }
            for piece in self.pieces(doc, slice) {
                match piece {
                    Piece::Text { text, attrs } => {
                        if !text.trim_matches('\n').is_empty() {
                            text_styles.push(attrs.style);
                        }
                    }
                    Piece::Atom { style, .. } => atom_styles.push(style),
                }
            }
        }
        let styles = if text_styles.is_empty() {
            atom_styles
        } else {
            text_styles
        };
        styles
            .into_iter()
            .reduce(Style::common)
            .unwrap_or(Style::NONE)
    }

    /// The `Link` construct touched by `selection` (across any selected line), if any.
    pub fn link_at(&self, selection: &Range<usize>) -> Option<&InlineConstruct> {
        self.lines
            .iter()
            .filter(|line| line.end >= selection.start && line.start <= selection.end)
            .flat_map(|line| line.inlines.iter())
            .find(|c| {
                matches!(c.kind, InlineKind::Link(_))
                    && selection.end >= c.outer.start
                    && selection.start <= c.outer.end
            })
    }

    /// Uniform non-delimiter content runs inside `line` (used by decorations).
    pub fn styled_runs(&self, line: &ParsedLine) -> Vec<StyledRun> {
        if line.content.is_empty() || line.inlines.is_empty() {
            return Vec::new();
        }
        let delimiters = delimiter_ranges(&line.inlines);
        let mut boundaries = vec![line.content.start, line.content.end];
        for c in &line.inlines {
            boundaries.push(c.inner.start);
            boundaries.push(c.inner.end);
            boundaries.push(c.open.start);
            boundaries.push(c.open.end);
            boundaries.push(c.close.start);
            boundaries.push(c.close.end);
        }
        boundaries.sort_unstable();
        boundaries.dedup();

        let mut runs: Vec<StyledRun> = Vec::new();
        for window in boundaries.windows(2) {
            let seg = window[0]..window[1];
            if seg.is_empty() || seg.start < line.content.start || seg.end > line.content.end {
                continue;
            }
            if is_in_ranges(&delimiters, seg.start) {
                continue;
            }
            let mut attrs = Attrs::default();
            for c in &line.inlines {
                if seg.start >= c.inner.start && seg.end <= c.inner.end {
                    match &c.kind {
                        InlineKind::Style(style) => attrs.style = attrs.style.with(*style),
                        InlineKind::Link(url) => attrs.link = Some(url.clone()),
                    }
                }
            }
            if !attrs.is_plain() {
                if let Some(last) = runs.last_mut()
                    && last.range.end == seg.start
                    && last.attrs == attrs
                {
                    last.range.end = seg.end;
                    continue;
                }
                runs.push(StyledRun { range: seg, attrs });
            }
        }
        runs
    }

    /// Cut `range` (within one line's content) into uniform rendered pieces, stripping all
    /// inline markdown delimiters while preserving atoms and merged styles.
    pub fn pieces<'a>(&self, doc: &'a Doc, range: Range<usize>) -> Vec<Piece<'a>> {
        if range.is_empty() {
            return Vec::new();
        }
        let line = self.line_at(range.start);
        let delimiters = delimiter_ranges(&line.inlines);
        let runs = self.styled_runs(line);
        let text = doc.text();
        let atoms = doc.atoms();

        let mut pieces = Vec::new();
        let mut at = range.start;
        let mut atom_ix = atoms.partition_point(|a| a.range.end <= at);

        while at < range.end {
            // Skip any inline delimiter covering `at`.
            if let Some(delim) = delimiters.iter().find(|d| d.start <= at && at < d.end) {
                at = delim.end.min(range.end);
                continue;
            }
            // Emit atom if one covers `at`.
            if let Some(placed) = atoms.get(atom_ix).filter(|a| a.range.start <= at) {
                let end = placed.range.end.min(range.end);
                let style = run_attrs_at(&runs, at).style;
                pieces.push(Piece::Atom {
                    atom: &placed.atom,
                    text: &text[at..end],
                    style,
                });
                at = end;
                atom_ix += 1;
                continue;
            }
            // Next stopping point: next atom, next delimiter, or end of current styled run.
            let mut end = range.end;
            if let Some(placed) = atoms.get(atom_ix) {
                end = end.min(placed.range.start);
            }
            if let Some(next_delim) = delimiters.iter().find(|d| d.start > at) {
                end = end.min(next_delim.start);
            }
            let attrs = match runs.iter().find(|r| r.range.end > at) {
                Some(run) if run.range.start <= at => {
                    end = end.min(run.range.end);
                    run.attrs.clone()
                }
                Some(run) => {
                    end = end.min(run.range.start);
                    Attrs::default()
                }
                None => Attrs::default(),
            };
            if end > at {
                pieces.push(Piece::Text {
                    text: &text[at..end],
                    attrs,
                });
                at = end;
            } else {
                break;
            }
        }
        pieces
    }
}

/// Detect an armed empty inline marker pair directly around `offset` (`*|*`, `**|**`,
/// `_|_`, `~|~`, `~~|~~`, `` `|` ``, `<u>|</u>`), returning its full byte range and style.
/// Returns `None` if either adjacent byte already belongs to a parsed construct's delimiter
/// (preventing false matches on `*a*|*b*` or `*|*bold**`).
pub fn empty_pair_at(
    text: &str,
    line: &ParsedLine,
    offset: usize,
) -> Option<(Range<usize>, Style)> {
    if offset < line.content.start || offset > line.content.end {
        return None;
    }
    let delimiters = delimiter_ranges(&line.inlines);
    let touches_existing_delim =
        |pos: usize| delimiters.iter().any(|d| d.start <= pos && pos < d.end);
    if (offset > line.content.start && touches_existing_delim(offset - 1))
        || (offset < line.content.end && touches_existing_delim(offset))
    {
        return None;
    }
    let before = &text[line.content.start..offset];
    let after = &text[offset..line.content.end];
    for (open, close, style) in [
        ("<u>", "</u>", Style::UNDERLINE),
        ("**", "**", Style::BOLD),
        ("~~", "~~", Style::STRIKE),
        ("*", "*", Style::BOLD),
        ("_", "_", Style::ITALIC),
        ("~", "~", Style::STRIKE),
        ("`", "`", Style::CODE),
    ] {
        if before.ends_with(open) && after.starts_with(close) {
            return Some((offset - open.len()..offset + close.len(), style));
        }
    }
    None
}

#[derive(Default)]
struct ListState {
    is_open: bool,
    ordered_num: u32,
}

fn overlaps_atom(atoms: &[Placed], range: Range<usize>) -> bool {
    let ix = atoms.partition_point(|a| a.range.end <= range.start);
    atoms.get(ix).is_some_and(|a| a.range.start < range.end)
}

pub(super) fn is_fence_line(raw: &str, start: usize, atoms: &[Placed]) -> bool {
    if !raw.starts_with("```") || overlaps_atom(atoms, start..start + 3) {
        return false;
    }
    let rest = raw[3..].trim();
    // A fence line is ``` alone or ```lang (word characters), never ```code``` inline text.
    !rest.contains('`') && !rest.contains(char::is_whitespace)
}

fn detect_block(
    raw: &str,
    start: usize,
    atoms: &[Placed],
    list: &ListState,
) -> (Option<LineKind>, usize) {
    if raw == "---" && !overlaps_atom(atoms, start..start + 3) {
        return (Some(LineKind::Divider), 3);
    }
    for (level, prefix) in [(3u8, "### "), (2, "## "), (1, "# ")] {
        if raw.starts_with(prefix) && !overlaps_atom(atoms, start..start + prefix.len()) {
            return (Some(LineKind::Heading(level)), prefix.len());
        }
    }
    for prefix in ["- ", "* ", "\u{2022} "] {
        if raw.starts_with(prefix) && !overlaps_atom(atoms, start..start + prefix.len()) {
            return (Some(LineKind::Bullet), prefix.len());
        }
    }
    if raw.starts_with("> ") && !overlaps_atom(atoms, start..start + 2) {
        return (Some(LineKind::Quote), 2);
    }
    if list.is_open && raw.starts_with("    ") && !overlaps_atom(atoms, start..start + 4) {
        return (Some(LineKind::Soft), 4);
    }
    let digits = raw.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0
        && raw[digits..].starts_with(". ")
        && !overlaps_atom(atoms, start..start + digits + 2)
        && let Ok(num) = raw[..digits].parse::<u32>()
    {
        // An ordered list opens at `1. ` (so `3. not a list` stays a paragraph), and once
        // open continues for any positive number (so deleting `2. b` keeps `3. c` in the
        // list).
        if (list.ordered_num > 0 && num >= 1) || num == 1 {
            return (Some(LineKind::Ordered), digits + 2);
        }
    }
    (None, 0)
}

fn parse_inlines(
    text: &str,
    range: Range<usize>,
    atoms: &[Placed],
    enclosing: Style,
    in_link: bool,
    out: &mut Vec<InlineConstruct>,
) {
    let mut at = range.start;
    while at < range.end {
        if let Some(atom) = atom_covering(atoms, at) {
            at = atom.range.end.min(range.end);
            continue;
        }
        let byte = text.as_bytes()[at];
        let matched = match byte {
            b'`' if !enclosing.contains(Style::CODE) => parse_code(text, at..range.end, atoms),
            b'<' if !enclosing.contains(Style::UNDERLINE) => {
                parse_underline(text, at..range.end, atoms)
            }
            b'[' if !in_link => parse_link(text, at..range.end, atoms),
            b'*' | b'~' if !enclosing.contains(Style::CODE) => {
                parse_double_or_single(text, at, range.end, atoms, enclosing, in_link)
            }
            b'_' if !enclosing.contains(Style::CODE) && !enclosing.contains(Style::ITALIC) => {
                parse_single(text, at, range.end, atoms, '_', Style::ITALIC, in_link)
            }
            _ => None,
        };
        if let Some(construct) = matched {
            let inner = construct.inner.clone();
            let next = construct.outer.end;
            let kind = construct.kind.clone();
            out.push(construct);
            match kind {
                InlineKind::Style(Style::CODE) => {}
                InlineKind::Style(style) => {
                    parse_inlines(text, inner, atoms, enclosing.with(style), in_link, out);
                }
                InlineKind::Link(_) => {
                    parse_inlines(text, inner, atoms, enclosing, true, out);
                }
            }
            at = next;
        } else {
            at += text[at..].chars().next().map_or(1, char::len_utf8);
        }
    }
}

fn atom_covering(atoms: &[Placed], offset: usize) -> Option<&Placed> {
    let ix = atoms.partition_point(|a| a.range.end <= offset);
    atoms.get(ix).filter(|a| a.range.start <= offset)
}

fn parse_code(text: &str, range: Range<usize>, atoms: &[Placed]) -> Option<InlineConstruct> {
    let start = range.start;
    let mut at = start + 1;
    while at < range.end {
        if let Some(atom) = atom_covering(atoms, at) {
            at = atom.range.end.min(range.end);
            continue;
        }
        if text.as_bytes()[at] == b'`' {
            if at == start + 1 {
                return None;
            }
            return Some(InlineConstruct {
                outer: start..at + 1,
                open: start..start + 1,
                inner: start + 1..at,
                close: at..at + 1,
                kind: InlineKind::Style(Style::CODE),
            });
        }
        at += 1;
    }
    None
}

fn parse_underline(text: &str, range: Range<usize>, atoms: &[Placed]) -> Option<InlineConstruct> {
    let start = range.start;
    if !text[range.clone()].starts_with("<u>") || overlaps_atom(atoms, start..start + 3) {
        return None;
    }
    let mut at = start + 3;
    while at + 4 <= range.end {
        if let Some(atom) = atom_covering(atoms, at) {
            at = atom.range.end.min(range.end);
            continue;
        }
        if text[at..range.end].starts_with("</u>") && !overlaps_atom(atoms, at..at + 4) {
            if at == start + 3 {
                return None;
            }
            return Some(InlineConstruct {
                outer: start..at + 4,
                open: start..start + 3,
                inner: start + 3..at,
                close: at..at + 4,
                kind: InlineKind::Style(Style::UNDERLINE),
            });
        }
        at += text[at..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

fn parse_link(text: &str, range: Range<usize>, atoms: &[Placed]) -> Option<InlineConstruct> {
    let start = range.start;
    // Find `](` closing the label without crossing `[` or an atom.
    let mut at = start + 1;
    let mut label_end = None;
    while at + 1 < range.end {
        if let Some(atom) = atom_covering(atoms, at) {
            at = atom.range.end.min(range.end);
            continue;
        }
        match text.as_bytes()[at] {
            b'[' => return None,
            b']' if text.as_bytes()[at + 1] == b'(' && !overlaps_atom(atoms, at..at + 2) => {
                if at == start + 1 {
                    return None;
                }
                label_end = Some(at);
                break;
            }
            _ => at += 1,
        }
    }
    let label_end = label_end?;
    let url_start = label_end + 2;
    let mut depth = 0u32;
    let mut pos = url_start;
    let mut url_end = None;
    while pos < range.end {
        if overlaps_atom(atoms, pos..pos + 1) {
            return None;
        }
        let ch = text[pos..].chars().next()?;
        if ch.is_whitespace() || ch.is_control() {
            return None;
        }
        match ch {
            '(' => depth += 1,
            ')' if depth == 0 => {
                url_end = Some(pos);
                break;
            }
            ')' => depth -= 1,
            _ => {}
        }
        pos += ch.len_utf8();
    }
    let url_end = url_end?;
    let url = &text[url_start..url_end];
    if url.is_empty() {
        return None;
    }
    Some(InlineConstruct {
        outer: start..url_end + 1,
        open: start..start + 1,
        inner: start + 1..label_end,
        close: label_end..url_end + 1,
        kind: InlineKind::Link(Arc::from(url)),
    })
}

fn parse_double_or_single(
    text: &str,
    start: usize,
    end: usize,
    atoms: &[Placed],
    enclosing: Style,
    in_link: bool,
) -> Option<InlineConstruct> {
    let ch = text.as_bytes()[start] as char;
    let style = if ch == '*' {
        Style::BOLD
    } else {
        Style::STRIKE
    };
    if enclosing.contains(style) {
        return None;
    }
    let double = if ch == '*' { "**" } else { "~~" };
    if text[start..end].starts_with(double) && !overlaps_atom(atoms, start..start + 2) {
        if let Some(found) = parse_double(text, start, end, atoms, double, ch, style, in_link) {
            return Some(found);
        }
        // Don't fall through to single `*` on `**` — would misparse empty `*` pair.
        return None;
    }
    parse_single(text, start, end, atoms, ch, style, in_link)
}

/// Skip over a nested inline code span `` `...` `` or markdown link `[label](url)` while
/// scanning for a closing delimiter, so characters inside URLs or code cannot close outer
/// formatting prematurely.
fn skip_nested_opaque(
    text: &str,
    at: usize,
    end: usize,
    atoms: &[Placed],
    in_link: bool,
) -> Option<usize> {
    match text.as_bytes()[at] {
        b'`' => parse_code(text, at..end, atoms).map(|c| c.outer.end),
        b'[' if !in_link => parse_link(text, at..end, atoms).map(|c| c.outer.end),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn parse_double(
    text: &str,
    start: usize,
    end: usize,
    atoms: &[Placed],
    delim: &str,
    ch: char,
    style: Style,
    in_link: bool,
) -> Option<InlineConstruct> {
    let inner_start = start + 2;
    let first = text[inner_start..end].chars().next()?;
    if first.is_whitespace() || first == ch {
        return None;
    }
    let mut at = inner_start;
    while at + 2 <= end {
        if let Some(atom) = atom_covering(atoms, at) {
            at = atom.range.end.min(end);
            continue;
        }
        if let Some(next) = skip_nested_opaque(text, at, end, atoms, in_link) {
            at = next;
            continue;
        }
        if text[at..end].starts_with(delim) && !overlaps_atom(atoms, at..at + 2) {
            let prev = text[inner_start..at].chars().next_back();
            if prev.is_some_and(|c| !c.is_whitespace() && c != ch) {
                return Some(InlineConstruct {
                    outer: start..at + 2,
                    open: start..inner_start,
                    inner: inner_start..at,
                    close: at..at + 2,
                    kind: InlineKind::Style(style),
                });
            }
        }
        at += text[at..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

fn parse_single(
    text: &str,
    start: usize,
    end: usize,
    atoms: &[Placed],
    marker: char,
    style: Style,
    in_link: bool,
) -> Option<InlineConstruct> {
    let is_word = |ch: Option<char>| ch.is_some_and(char::is_alphanumeric);
    if is_word(text[..start].chars().next_back()) {
        return None;
    }
    let inner_start = start + 1;
    let first = text[inner_start..end].chars().next()?;
    if first.is_whitespace() || first == marker {
        return None;
    }
    let mut at = inner_start;
    while at < end {
        if let Some(atom) = atom_covering(atoms, at) {
            at = atom.range.end.min(end);
            continue;
        }
        if let Some(next) = skip_nested_opaque(text, at, end, atoms, in_link) {
            at = next;
            continue;
        }
        let ch = text[at..].chars().next()?;
        if ch == marker {
            let prev = text[inner_start..at].chars().next_back();
            let after = text[at + 1..end].chars().next();
            let closes = prev.is_some_and(|c| !c.is_whitespace() && c != marker) && !is_word(after);
            if closes {
                return Some(InlineConstruct {
                    outer: start..at + 1,
                    open: start..inner_start,
                    inner: inner_start..at,
                    close: at..at + 1,
                    kind: InlineKind::Style(style),
                });
            }
        }
        at += ch.len_utf8();
    }
    None
}

pub(crate) fn delimiter_ranges(inlines: &[InlineConstruct]) -> Vec<Range<usize>> {
    let mut ranges = Vec::with_capacity(inlines.len() * 2);
    for c in inlines {
        ranges.push(c.open.clone());
        ranges.push(c.close.clone());
    }
    ranges.sort_by_key(|r| r.start);
    ranges
}

fn is_in_ranges(sorted: &[Range<usize>], offset: usize) -> bool {
    let ix = sorted.partition_point(|r| r.end <= offset);
    sorted.get(ix).is_some_and(|r| r.start <= offset)
}

fn run_attrs_at(runs: &[StyledRun], offset: usize) -> Attrs {
    let ix = runs.partition_point(|r| r.range.end <= offset);
    match runs.get(ix) {
        Some(run) if run.range.start <= offset => run.attrs.clone(),
        _ => Attrs::default(),
    }
}
