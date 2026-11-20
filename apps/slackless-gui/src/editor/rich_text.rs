//! The document as the `rich_text` block Slack's own client posts: paragraphs become
//! sections, list items a list, quote lines a quote, code lines one preformatted block.
//! Formatting is carried by element styles, so no character has to be escaped.

use std::ops::Range;

use slackless_core::types::rich_text::{
    BroadcastElement, ChannelElement, EmojiElement, LinkElement, ListStyle, RichTextList,
    RichTextPreformatted, RichTextQuote, RichTextSection, TextElement, UserElement,
};
use slackless_core::types::{RichTextBlock, RichTextContainer, RichTextElement, TextStyle};

use super::doc::Doc;
use super::marks::{Atom, LineKind, Style};
use super::read::Piece;

impl Doc {
    pub fn to_rich_text(&self) -> RichTextBlock {
        let mut containers: Vec<RichTextContainer> = Vec::new();
        // The previous line's kind; the outer `None` is "no line yet".
        let mut above: Option<Option<LineKind>> = None;
        for line in self.lines() {
            let elements = self.elements(line.kind, line.content.clone());
            let last = containers.last_mut();
            match (line.kind, last) {
                (None, Some(RichTextContainer::Section(section))) if above == Some(None) => {
                    extend_line(&mut section.elements, elements);
                }
                (Some(LineKind::Quote), Some(RichTextContainer::Quote(quote))) => {
                    extend_line(&mut quote.elements, elements);
                }
                (Some(LineKind::Code), Some(RichTextContainer::Preformatted(code))) => {
                    extend_line(&mut code.elements, elements);
                }
                (Some(LineKind::Soft), Some(RichTextContainer::List(list))) => {
                    if let Some(RichTextContainer::Section(item)) = list.elements.last_mut() {
                        extend_line(&mut item.elements, elements);
                    }
                }
                (
                    Some(kind @ (LineKind::Bullet | LineKind::Ordered)),
                    Some(RichTextContainer::List(list)),
                ) if list.style == list_style(kind) => {
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
                // A soft line with no item above it reads as a paragraph.
                (None | Some(LineKind::Soft), _) => containers.push(section(elements)),
            }
            above = Some(line.kind);
        }
        prune(&mut containers);
        RichTextBlock {
            block_id: None,
            elements: containers,
        }
    }

    /// The inline elements of `range`, the content of a line of `kind`. Code is literal text.
    fn elements(&self, kind: Option<LineKind>, range: Range<usize>) -> Vec<RichTextElement> {
        let mut elements = Vec::new();
        if kind == Some(LineKind::Code) {
            push(&mut elements, text(&self.text()[range], None));
            return elements;
        }
        for piece in self.pieces(range) {
            let element = match piece {
                Piece::Text { text: run, attrs } => match &attrs.link {
                    Some(url) => RichTextElement::Link(LinkElement {
                        url: url.to_string(),
                        text: Some(run.to_owned()),
                        style: text_style(attrs.style),
                    }),
                    None => text(run, text_style(attrs.style)),
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
                        Atom::Broadcast(range) => RichTextElement::Broadcast(BroadcastElement {
                            range: range.clone(),
                            style,
                        }),
                        Atom::Emoji(shortcode) => emoji(shortcode, style),
                        Atom::Marker(_) => continue,
                    }
                }
            };
            push(&mut elements, element);
        }
        elements
    }
}

fn list_style(kind: LineKind) -> ListStyle {
    match kind {
        LineKind::Ordered => ListStyle::Ordered,
        LineKind::Bullet | LineKind::Quote | LineKind::Code | LineKind::Soft => ListStyle::Bullet,
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
        code: style.contains(Style::CODE),
        ..TextStyle::default()
    })
}

/// `wave::skin-tone-3` as Slack's emoji element: the base name and the tone beside it.
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

/// Drop what Slack rejects as an empty block: containers and list items without elements.
/// (A blank line inside a paragraph, quote or code block is a newline in its text.)
fn prune(containers: &mut Vec<RichTextContainer>) {
    containers.retain_mut(|container| match container {
        RichTextContainer::Section(section) => !section.elements.is_empty(),
        RichTextContainer::Quote(quote) => !quote.elements.is_empty(),
        RichTextContainer::Preformatted(code) => !code.elements.is_empty(),
        RichTextContainer::List(list) => {
            prune(&mut list.elements);
            !list.elements.is_empty()
        }
        RichTextContainer::Unknown { .. } => true,
    });
}

/// Add `element`, joining it onto the previous text run when both carry the same style. An
/// empty text run is not an element.
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

/// Continue a container with a further line.
fn extend_line(elements: &mut Vec<RichTextElement>, line: Vec<RichTextElement>) {
    push(elements, text("\n", None));
    for element in line {
        push(elements, element);
    }
}
