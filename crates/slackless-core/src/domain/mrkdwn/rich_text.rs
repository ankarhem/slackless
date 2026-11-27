//! Converting between [`Doc`] / Slack `mrkdwn` and Slack's `rich_text` Block Kit wire format.

use std::ops::Range;
use std::sync::Arc;

use crate::types::rich_text::{
    BroadcastElement, ChannelElement, EmojiElement, LinkElement, ListStyle, RichTextDivider,
    RichTextHeader, RichTextList, RichTextPreformatted, RichTextQuote, RichTextSection,
    TextElement, UserElement, UsergroupElement,
};
use crate::types::{Block, RichTextBlock, RichTextContainer, RichTextElement, TextStyle};

use super::doc::{Atom, Doc, LineKind, Style};
use super::parse::{ParsedDoc, Piece};
use super::spans::{Directory, SOFT_INDENT};

/// Convert Slack `mrkdwn` directly into a Slack [`RichTextBlock`] (tests only).
#[cfg(any(test, feature = "test-support"))]
pub fn mrkdwn_to_rich_text(text: &str, names: &dyn Directory) -> RichTextBlock {
    super::spans::from_mrkdwn(text, names).to_rich_text()
}

/// Convert Slack [`Block`]s containing `rich_text` directly into Slack `mrkdwn` (tests only).
#[cfg(any(test, feature = "test-support"))]
pub fn rich_text_to_mrkdwn(blocks: &[Block], names: &dyn Directory) -> Option<String> {
    from_rich_text(blocks, names).map(|doc| doc.to_mrkdwn())
}

/// Read a [`Doc`] back from a message's `rich_text` Block Kit blocks.
pub fn from_rich_text(blocks: &[Block], names: &dyn Directory) -> Option<Doc> {
    let mut reader = RichTextReader {
        doc: Doc::default(),
        names,
        lines: 0,
    };
    let mut saw_container = false;
    for block in blocks {
        let Block::RichText(rich_text) = block else {
            continue;
        };
        for container in &rich_text.elements {
            if reader.container(container) {
                saw_container = true;
            }
        }
    }
    saw_container.then_some(reader.doc)
}

struct RichTextReader<'a> {
    doc: Doc,
    names: &'a dyn Directory,
    lines: usize,
}

impl RichTextReader<'_> {
    fn begin_line(&mut self, prefix: &str) {
        if self.lines > 0 {
            self.doc.append("\n");
        }
        self.lines += 1;
        self.doc.append(prefix);
    }

    fn container(&mut self, container: &RichTextContainer) -> bool {
        match container {
            RichTextContainer::Section(section) => {
                self.begin_line("");
                self.elements(&section.elements, "");
                true
            }
            RichTextContainer::Header(header) => {
                let level = header.level.unwrap_or(2).clamp(1, 3);
                let prefix = LineKind::Heading(level).markdown_prefix();
                self.begin_line(prefix);
                self.elements(&header.elements, prefix);
                true
            }
            RichTextContainer::Quote(quote) => {
                self.begin_line("> ");
                self.elements(&quote.elements, "> ");
                true
            }
            RichTextContainer::Preformatted(code) => {
                self.begin_line("```");
                self.begin_line("");
                self.code_elements(&code.elements);
                self.begin_line("```");
                true
            }
            RichTextContainer::Divider(_) => {
                self.begin_line("---");
                true
            }
            RichTextContainer::List(list) => {
                let mut number = u64::from(list.offset.unwrap_or(0));
                let mut any = false;
                for item in &list.elements {
                    let elements = match item {
                        RichTextContainer::Section(sec) => &sec.elements,
                        RichTextContainer::Quote(q) => &q.elements,
                        RichTextContainer::Header(h) => &h.elements,
                        RichTextContainer::Preformatted(p) => &p.elements,
                        RichTextContainer::List(_)
                        | RichTextContainer::Divider(_)
                        | RichTextContainer::Unknown { .. } => continue,
                    };
                    match list.style {
                        ListStyle::Ordered => {
                            number = number.saturating_add(1);
                            self.begin_line(&format!("{number}. "));
                        }
                        ListStyle::Bullet | ListStyle::Other(_) => {
                            self.begin_line("- ");
                        }
                    }
                    self.elements(elements, SOFT_INDENT);
                    any = true;
                }
                any
            }
            RichTextContainer::Unknown { .. } => false,
        }
    }

    fn code_elements(&mut self, elements: &[RichTextElement]) {
        for element in elements {
            let text = match element {
                RichTextElement::Text(t) => t.text.as_str(),
                RichTextElement::Link(l) => l.text.as_deref().unwrap_or(l.url.as_str()),
                _ => continue,
            };
            let mut chunks = text.split('\n');
            if let Some(first) = chunks.next() {
                self.doc.append(first);
            }
            for rest in chunks {
                self.begin_line("");
                self.doc.append(rest);
            }
        }
    }

    fn elements(&mut self, elements: &[RichTextElement], continuation: &str) {
        for element in elements {
            match element {
                RichTextElement::Text(t) => {
                    let style = from_text_style(t.style);
                    self.append_styled_multiline(&t.text, style, None, continuation);
                }
                RichTextElement::Link(link) => {
                    let style = from_text_style(link.style);
                    let label = link.text.as_deref().filter(|s| !s.is_empty());
                    match label {
                        Some(label) => {
                            self.append_styled_multiline(
                                label,
                                style,
                                Some(link.url.as_str()),
                                continuation,
                            );
                        }
                        None => {
                            self.append_styled_multiline(&link.url, style, None, continuation);
                        }
                    }
                }
                RichTextElement::User(user) => {
                    let name = self.names.mention(&user.user_id);
                    let style = from_text_style(user.style);
                    self.append_styled_atom(
                        Atom::User(user.user_id.clone()),
                        &format!("@{name}"),
                        style,
                    );
                }
                RichTextElement::Channel(channel) => {
                    let name = self.names.channel_name(&channel.channel_id);
                    let style = from_text_style(channel.style);
                    self.append_styled_atom(
                        Atom::Channel(channel.channel_id.clone()),
                        &format!("#{name}"),
                        style,
                    );
                }
                RichTextElement::Broadcast(broadcast) => {
                    let style = from_text_style(broadcast.style);
                    self.append_styled_atom(
                        Atom::Broadcast(broadcast.range.clone()),
                        &format!("@{}", broadcast.range.as_str()),
                        style,
                    );
                }
                RichTextElement::Emoji(emoji) => {
                    let shortcode = match emoji.skin_tone {
                        Some(tone) => format!("{}::skin-tone-{tone}", emoji.name),
                        None => emoji.name.clone(),
                    };
                    let shown = self
                        .names
                        .emoji_text(&shortcode)
                        .unwrap_or_else(|| format!(":{shortcode}:"));
                    let style = from_text_style(emoji.style);
                    self.append_styled_atom(Atom::Emoji(Arc::from(shortcode)), &shown, style);
                }
                RichTextElement::Date(date) => {
                    let text = date.fallback.as_deref().unwrap_or(date.format.as_str());
                    self.append_styled_multiline(text, Style::NONE, None, continuation);
                }
                RichTextElement::Color(color) => {
                    self.doc.append(&color.value);
                }
                RichTextElement::AttachmentMention(mention) => {
                    let style = from_text_style(mention.style);
                    let label = mention
                        .text
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .unwrap_or(mention.url.as_str());
                    let url = (!mention.url.is_empty()).then_some(mention.url.as_str());
                    self.append_styled_multiline(label, style, url, continuation);
                }
                RichTextElement::MessageMention(mention) => {
                    let label = mention
                        .url
                        .as_deref()
                        .or(mention.text.as_deref())
                        .unwrap_or_default();
                    let url = mention.url.as_deref().filter(|u| *u != label);
                    self.append_styled_multiline(label, Style::NONE, url, continuation);
                }
                RichTextElement::Usergroup(group) => {
                    let handle = self.names.usergroup_handle(&group.usergroup_id);
                    let style = from_text_style(group.style);
                    self.append_styled_atom(
                        Atom::Usergroup(group.usergroup_id.clone()),
                        &format!("@{handle}"),
                        style,
                    );
                }
                RichTextElement::Team(team) => {
                    let style = from_text_style(team.style);
                    self.append_styled_multiline(team.team_id.as_str(), style, None, continuation);
                }
                RichTextElement::Canvas(_) | RichTextElement::Unknown { .. } => {}
            }
        }
    }

    fn append_styled_atom(&mut self, atom: Atom, shown: &str, style: Style) {
        let (open, close) = style_wrappers(style);
        self.doc.append(&open);
        self.doc.append_atom(atom, shown);
        self.doc.append(&close);
    }

    fn append_styled_multiline(
        &mut self,
        text: &str,
        style: Style,
        link: Option<&str>,
        continuation: &str,
    ) {
        let mut chunks = text.split('\n');
        if let Some(first) = chunks.next() {
            self.append_styled_chunk(first, style, link);
        }
        for rest in chunks {
            self.begin_line(continuation);
            self.append_styled_chunk(rest, style, link);
        }
    }

    fn append_styled_chunk(&mut self, chunk: &str, style: Style, link: Option<&str>) {
        if chunk.is_empty() {
            return;
        }
        if style.is_plain() && link.is_none() {
            self.doc.append(chunk);
            return;
        }
        let trimmed = chunk.trim();
        if trimmed.is_empty() {
            self.doc.append(chunk);
            return;
        }
        let lead_len = chunk.len() - chunk.trim_start().len();
        let trail_start = lead_len + trimmed.len();
        let (lead, body, trail) = (
            &chunk[..lead_len],
            &chunk[lead_len..trail_start],
            &chunk[trail_start..],
        );
        self.doc.append(lead);
        let prev_char = self.doc.text().chars().next_back();
        let abuts_word = prev_char.is_some_and(char::is_alphanumeric);
        let has_code = style.contains(Style::CODE);
        let outer_style = if link.is_some() && has_code {
            Style::NONE
                .with(if style.contains(Style::BOLD) {
                    Style::BOLD
                } else {
                    Style::NONE
                })
                .with(if style.contains(Style::ITALIC) {
                    Style::ITALIC
                } else {
                    Style::NONE
                })
                .with(if style.contains(Style::STRIKE) {
                    Style::STRIKE
                } else {
                    Style::NONE
                })
                .with(if style.contains(Style::UNDERLINE) {
                    Style::UNDERLINE
                } else {
                    Style::NONE
                })
        } else {
            style
        };
        let (open, close) = style_wrappers_for(outer_style, abuts_word, prev_char);
        self.doc.append(&open);
        match link {
            Some(url) => {
                self.doc.append("[");
                if has_code {
                    self.doc.append("`");
                }
                self.doc.append(body);
                if has_code {
                    self.doc.append("`");
                }
                self.doc.append("](");
                self.doc.append(url);
                self.doc.append(")");
            }
            None => self.doc.append(body),
        }
        self.doc.append(&close);
        self.doc.append(trail);
    }
}

fn style_wrappers(style: Style) -> (String, String) {
    style_wrappers_for(style, false, None)
}

fn style_wrappers_for(style: Style, abuts_word: bool, prev: Option<char>) -> (String, String) {
    let mut open = String::new();
    let mut close = String::new();
    let bold_delim = if abuts_word || prev == Some('*') {
        "**"
    } else {
        "*"
    };
    let strike_delim = if abuts_word || prev == Some('~') {
        "~~"
    } else {
        "~"
    };
    for (flag, o, c) in [
        (Style::BOLD, bold_delim, bold_delim),
        (Style::ITALIC, "_", "_"),
        (Style::STRIKE, strike_delim, strike_delim),
        (Style::UNDERLINE, "<u>", "</u>"),
        (Style::CODE, "`", "`"),
    ] {
        if style.contains(flag) {
            open.push_str(o);
            close.insert_str(0, c);
        }
    }
    (open, close)
}

fn from_text_style(style: Option<TextStyle>) -> Style {
    let Some(style) = style else {
        return Style::NONE;
    };
    let mut out = Style::NONE;
    if style.bold {
        out = out.with(Style::BOLD);
    }
    if style.italic {
        out = out.with(Style::ITALIC);
    }
    if style.strike {
        out = out.with(Style::STRIKE);
    }
    if style.underline {
        out = out.with(Style::UNDERLINE);
    }
    if style.code {
        out = out.with(Style::CODE);
    }
    out
}

impl Doc {
    /// Convert this document into Slack's `rich_text` Block Kit representation.
    pub fn to_rich_text(&self) -> RichTextBlock {
        let parsed = self.parse();
        let mut containers: Vec<RichTextContainer> = Vec::new();
        let mut above: Option<Option<LineKind>> = None;
        for line in &parsed.lines {
            if line.kind == Some(LineKind::Fence) {
                above = Some(Some(LineKind::Fence));
                continue;
            }
            let elements = line_elements(self, &parsed, line.kind, line.content.clone());
            let last = containers.last_mut();
            match (line.kind, last) {
                (None, Some(RichTextContainer::Section(section))) if above == Some(None) => {
                    extend_line(&mut section.elements, elements);
                }
                (Some(LineKind::Quote), Some(RichTextContainer::Quote(quote)))
                    if above == Some(Some(LineKind::Quote)) =>
                {
                    extend_line(&mut quote.elements, elements);
                }
                (Some(LineKind::Code), Some(RichTextContainer::Preformatted(code)))
                    if above == Some(Some(LineKind::Code)) =>
                {
                    extend_line(&mut code.elements, elements);
                }
                (Some(LineKind::Heading(level)), Some(RichTextContainer::Header(header)))
                    if above == Some(Some(LineKind::Heading(level)))
                        && header.level == Some(level) =>
                {
                    extend_line(&mut header.elements, elements);
                }
                (Some(LineKind::Soft), Some(RichTextContainer::List(list))) => {
                    if let Some(RichTextContainer::Section(item)) = list.elements.last_mut() {
                        extend_line(&mut item.elements, elements);
                    }
                }
                (
                    Some(kind @ (LineKind::Bullet | LineKind::Ordered)),
                    Some(RichTextContainer::List(list)),
                ) if list.style == list_style(kind)
                    && matches!(
                        above,
                        Some(Some(LineKind::Bullet | LineKind::Ordered | LineKind::Soft))
                    ) =>
                {
                    list.elements.push(section(elements));
                }
                (Some(kind @ (LineKind::Bullet | LineKind::Ordered)), _) => {
                    containers.push(RichTextContainer::List(RichTextList {
                        style: list_style(kind),
                        elements: vec![section(elements)],
                        indent: Some(0),
                        offset: None,
                        border: Some(0),
                    }));
                }
                (Some(LineKind::Quote), _) => {
                    containers.push(RichTextContainer::Quote(RichTextQuote {
                        elements,
                        border: None,
                    }));
                }
                (Some(LineKind::Code), _) => {
                    containers.push(RichTextContainer::Preformatted(RichTextPreformatted {
                        elements,
                        border: Some(0),
                        language: None,
                    }));
                }
                (Some(LineKind::Heading(level)), _) => {
                    containers.push(RichTextContainer::Header(RichTextHeader {
                        level: Some(level),
                        elements,
                    }));
                }
                (Some(LineKind::Divider), _) => {
                    containers.push(RichTextContainer::Divider(RichTextDivider::default()));
                }
                (None | Some(LineKind::Soft) | Some(LineKind::Fence), _) => {
                    containers.push(section(elements));
                }
            }
            above = Some(line.kind);
        }
        prune(&mut containers);
        RichTextBlock {
            block_id: None,
            elements: containers,
        }
    }
}

fn line_elements(
    doc: &Doc,
    parsed: &ParsedDoc,
    kind: Option<LineKind>,
    range: Range<usize>,
) -> Vec<RichTextElement> {
    let mut elements = Vec::new();
    if kind == Some(LineKind::Code) {
        push(&mut elements, text(&doc.text()[range], None));
        return elements;
    }
    for piece in parsed.pieces(doc, range) {
        let element = match piece {
            Piece::Text { text: run, attrs } => match &attrs.link {
                Some(url) => RichTextElement::Link(LinkElement {
                    url: url.to_string(),
                    text: (url.as_ref() != run).then(|| run.to_owned()),
                    style: text_style(attrs.style),
                }),
                None if attrs.style.contains(Style::CODE) => text(run, text_style(attrs.style)),
                None => {
                    for element in linkified(run, text_style(attrs.style)) {
                        push(&mut elements, element);
                    }
                    continue;
                }
            },
            Piece::Atom { atom, style, .. } => {
                let style = text_style(style);
                match atom {
                    Atom::User(id) => RichTextElement::User(UserElement {
                        user_id: id.clone(),
                        style,
                    }),
                    Atom::Channel(id) => RichTextElement::Channel(ChannelElement {
                        channel_id: id.clone(),
                        style,
                    }),
                    Atom::Usergroup(id) => RichTextElement::Usergroup(UsergroupElement {
                        usergroup_id: id.clone(),
                        style,
                    }),
                    Atom::Broadcast(range) => RichTextElement::Broadcast(BroadcastElement {
                        range: range.clone(),
                        style,
                    }),
                    Atom::Emoji(shortcode) => emoji(shortcode, style),
                }
            }
        };
        push(&mut elements, element);
    }
    elements
}

fn linkified(run: &str, style: Option<TextStyle>) -> Vec<RichTextElement> {
    let mut elements = Vec::new();
    let mut rest = run;
    while let Some(start) = url_start(rest) {
        let mut end = rest[start..]
            .find(char::is_whitespace)
            .map_or(rest.len(), |len| start + len);
        while let Some(last) = rest[start..end].chars().last() {
            let url = &rest[start..end];
            let unmatched = (last == ')' && url.matches('(').count() < url.matches(')').count())
                || (last == ']' && url.matches('[').count() < url.matches(']').count())
                || (last == '>' && url.matches('<').count() < url.matches('>').count());
            if !(".,;:!?'\"".contains(last) || unmatched) {
                break;
            }
            end -= last.len_utf8();
        }
        let url = &rest[start..end];
        let min_len = if url.starts_with("https://") {
            "https://".len()
        } else if url.starts_with("http://") {
            "http://".len()
        } else {
            "www.".len()
        };
        if url.len() <= min_len {
            push(&mut elements, text(&rest[..end], style));
            rest = &rest[end..];
            continue;
        }
        push(&mut elements, text(&rest[..start], style));
        elements.push(RichTextElement::Link(LinkElement {
            url: if url.starts_with("www.") {
                format!("http://{url}")
            } else {
                url.to_owned()
            },
            text: None,
            style,
        }));
        rest = &rest[end..];
    }
    push(&mut elements, text(rest, style));
    elements
}

fn url_start(text: &str) -> Option<usize> {
    ["https://", "http://", "www."]
        .into_iter()
        .flat_map(|prefix| text.match_indices(prefix).map(|(at, _)| at))
        .filter(|&at| {
            text[..at]
                .chars()
                .last()
                .is_none_or(|c| c.is_whitespace() || "(<[".contains(c))
        })
        .min()
}

fn list_style(kind: LineKind) -> ListStyle {
    match kind {
        LineKind::Ordered => ListStyle::Ordered,
        LineKind::Bullet
        | LineKind::Quote
        | LineKind::Code
        | LineKind::Fence
        | LineKind::Soft
        | LineKind::Heading(_)
        | LineKind::Divider => ListStyle::Bullet,
    }
}

fn section(elements: Vec<RichTextElement>) -> RichTextContainer {
    RichTextContainer::Section(RichTextSection { elements })
}

fn text(text: &str, style: Option<TextStyle>) -> RichTextElement {
    RichTextElement::Text(TextElement {
        text: text.to_owned(),
        style,
    })
}

fn text_style(style: Style) -> Option<TextStyle> {
    (!style.is_plain()).then(|| TextStyle {
        bold: style.contains(Style::BOLD),
        italic: style.contains(Style::ITALIC),
        strike: style.contains(Style::STRIKE),
        underline: style.contains(Style::UNDERLINE),
        code: style.contains(Style::CODE),
        ..TextStyle::default()
    })
}

fn emoji(shortcode: &str, style: Option<TextStyle>) -> RichTextElement {
    let (name, tone) = match shortcode.split_once("::skin-tone-") {
        Some((name, tone)) => (name, tone.parse().ok()),
        None => (shortcode, None),
    };
    RichTextElement::Emoji(EmojiElement {
        name: name.to_owned(),
        unicode: None,
        skin_tone: tone,
        style,
    })
}

fn prune(containers: &mut Vec<RichTextContainer>) {
    containers.retain_mut(|container| match container {
        RichTextContainer::Section(section) => !section.elements.is_empty(),
        RichTextContainer::Header(header) => !header.elements.is_empty(),
        RichTextContainer::Quote(quote) => !quote.elements.is_empty(),
        RichTextContainer::Preformatted(code) => !code.elements.is_empty(),
        RichTextContainer::Divider(_) => true,
        RichTextContainer::List(list) => {
            prune(&mut list.elements);
            !list.elements.is_empty()
        }
        RichTextContainer::Unknown { .. } => true,
    });
}

fn push(elements: &mut Vec<RichTextElement>, element: RichTextElement) {
    match (elements.last_mut(), &element) {
        (_, RichTextElement::Text(next)) if next.text.is_empty() => {}
        (Some(RichTextElement::Text(last)), RichTextElement::Text(next))
            if last.style == next.style =>
        {
            last.text.push_str(&next.text);
        }
        _ => elements.push(element),
    }
}

fn extend_line(elements: &mut Vec<RichTextElement>, line: Vec<RichTextElement>) {
    push(elements, text("\n", None));
    for element in line {
        push(elements, element);
    }
}
