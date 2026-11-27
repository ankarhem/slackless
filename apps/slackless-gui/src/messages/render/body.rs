//! Message bodies as theme-free models: a mrkdwn body's blocks, a Block Kit `rich_text`
//! block's containers, the body model that holds them for one message, and the cache the
//! transcript keeps between frames. Block Kit (`unknown`) blocks render live, in block order,
//! from [`BodyModel::parts`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::gpui::SharedString;
use slackless_core::domain::mrkdwn::{parse_mrkdwn_spans, unescape_mrkdwn};
use slackless_core::types::rich_text::{
    ListStyle, RichTextContainer, RichTextElement, RichTextList,
};
use slackless_core::types::{Block, Edited, Message};

use super::super::LinkTarget;
use super::super::time::date_label_absolute;
use super::CivilTime;
use super::CodeModel;
use super::inline::{flags, hex_color, unicode_glyph};
use super::model::{
    ParagraphBuilder, RunStyle, Segment, TextOpts, Tone, line_segments, mention_style,
    mrkdwn_fences, push_link, strip_quote_prefix,
};
use crate::fixtures::Fixtures;

/// One block of a mrkdwn body: its text split at ``` fences, then into lines.
pub(in crate::messages) enum MrkdwnBlock {
    Code(CodeModel),
    /// A run of blank lines: Slack's 8px `c-mrkdwn__br` block, not empty text lines.
    Break,
    Line(Vec<Segment>),
    /// Consecutive `>` lines behind one quote bar: `None` is an empty quoted line's gap.
    Quote(Vec<Option<Vec<Segment>>>),
    /// The edit marker on its own line (a body ending in a pre block).
    Edited,
}

/// mrkdwn `text` as the blocks Slack lays out. With `edited`, the marker rides the last line
/// that renders text, or its own line after a closing code block.
pub(in crate::messages) fn mrkdwn_blocks(
    text: &str,
    fixtures: &Fixtures,
    opts: TextOpts,
    edited: bool,
) -> Vec<MrkdwnBlock> {
    let pieces = mrkdwn_fences(text);
    let mut blocks = Vec::new();
    for (n, (fenced, piece)) in pieces.iter().enumerate() {
        if *fenced {
            blocks.push(MrkdwnBlock::Code(CodeModel::new(
                unescape_mrkdwn(piece).into_owned(),
                None,
            )));
        } else {
            mrkdwn_lines(
                &mut blocks,
                piece,
                fixtures,
                opts,
                edited && n + 1 == pieces.len(),
            );
        }
    }
    if edited && pieces.last().is_some_and(|(fenced, _)| *fenced) {
        blocks.push(MrkdwnBlock::Edited);
    }
    blocks
}

fn mrkdwn_lines(
    blocks: &mut Vec<MrkdwnBlock>,
    text: &str,
    fixtures: &Fixtures,
    opts: TextOpts,
    edited: bool,
) {
    let lines: Vec<&str> = text.split('\n').collect();
    let last_text_line = lines.iter().rposition(|line| !line.is_empty());
    let line =
        |line: &str, edited: bool| line_segments(&parse_mrkdwn_spans(line), fixtures, opts, edited);
    let mut blank = false;
    let mut i = 0;
    while i < lines.len() {
        if lines[i].is_empty() {
            if !blank {
                blocks.push(MrkdwnBlock::Break);
            }
            blank = true;
            i += 1;
            continue;
        }
        blank = false;
        if strip_quote_prefix(lines[i]).is_some() {
            let mut quoted = Vec::new();
            while i < lines.len() {
                let line = match strip_quote_prefix(lines[i]) {
                    Some("") => None,
                    Some(inner) => Some(line(inner, edited && Some(i) == last_text_line)),
                    None => break,
                };
                quoted.push(line);
                i += 1;
            }
            blocks.push(MrkdwnBlock::Quote(quoted));
            continue;
        }
        blocks.push(MrkdwnBlock::Line(line(
            lines[i],
            edited && Some(i) == last_text_line,
        )));
        i += 1;
    }
}

/// One container of a Block Kit `rich_text` block, as paragraph segments.
pub(in crate::messages) enum Container {
    Paragraph(Vec<Segment>),
    /// `(marker, text)` per item that renders; `indent` is the nesting depth.
    List {
        indent: u32,
        ordered: bool,
        items: Vec<(SharedString, Vec<Segment>)>,
    },
    Quote(Vec<Segment>),
    Code(CodeModel),
    Header {
        level: u8,
        /// A rule of space above, unless the header is the block's first container.
        space_above: bool,
        segments: Vec<Segment>,
    },
    Divider,
}

/// The containers of a `rich_text` block. With `edited`, the final container's plain text (a
/// section, or a list's last item) carries the edit marker inline at its end; boxed text
/// (quotes) and code blocks leave it to the body's own trailing line.
pub(in crate::messages) fn containers(
    elements: &[RichTextContainer],
    fixtures: &Fixtures,
    opts: TextOpts,
    edited: bool,
) -> Vec<Container> {
    elements
        .iter()
        .enumerate()
        .filter_map(|(n, container)| {
            let edited = edited && n + 1 == elements.len();
            match container {
                RichTextContainer::Section(section) => {
                    let segments = paragraph(&section.elements, fixtures, opts, edited)?;
                    Some(Container::Paragraph(segments))
                }
                RichTextContainer::List(list) => {
                    let items = list_items(list, fixtures, opts, edited);
                    (!items.is_empty()).then_some(Container::List {
                        indent: list.indent.unwrap_or(0),
                        ordered: matches!(list.style, ListStyle::Ordered),
                        items,
                    })
                }
                RichTextContainer::Quote(quote) => {
                    let segments = paragraph(&quote.elements, fixtures, opts, false)?;
                    Some(Container::Quote(segments))
                }
                RichTextContainer::Preformatted(preformatted) => {
                    Some(Container::Code(CodeModel::preformatted(preformatted)))
                }
                RichTextContainer::Header(header) => {
                    let segments = paragraph(&header.elements, fixtures, opts, false)?;
                    Some(Container::Header {
                        level: header.level.unwrap_or(2).clamp(1, 3),
                        space_above: n > 0,
                        segments,
                    })
                }
                RichTextContainer::Divider(_) => Some(Container::Divider),
                RichTextContainer::Unknown { .. } => None,
            }
        })
        .collect()
}

/// A paragraph of rich-text elements as segments, `None` when nothing renders.
fn paragraph(
    elements: &[RichTextElement],
    fixtures: &Fixtures,
    opts: TextOpts,
    edited: bool,
) -> Option<Vec<Segment>> {
    let mut builder = ParagraphBuilder::new();
    for element in elements {
        push_element(&mut builder, element, fixtures, opts);
    }
    builder.trim_trailing_breaks();
    if edited {
        builder.push_edited();
    }
    let segments = builder.finish();
    (!segments.is_empty()).then_some(segments)
}

fn list_items(
    list: &RichTextList,
    fixtures: &Fixtures,
    opts: TextOpts,
    edited: bool,
) -> Vec<(SharedString, Vec<Segment>)> {
    let ordered = matches!(list.style, ListStyle::Ordered);
    let offset = list.offset.unwrap_or(0) as usize;
    let last = list.elements.len();
    list.elements
        .iter()
        .enumerate()
        .filter_map(|(n, item)| {
            let RichTextContainer::Section(section) = item else {
                return None;
            };
            let paragraph = paragraph(&section.elements, fixtures, opts, edited && n + 1 == last)?;
            let marker = if ordered {
                format!("{}.", offset.saturating_add(n).saturating_add(1))
            } else {
                "•".to_owned()
            };
            Some((marker.into(), paragraph))
        })
        .collect()
}

fn push_element(
    builder: &mut ParagraphBuilder,
    element: &RichTextElement,
    fixtures: &Fixtures,
    opts: TextOpts,
) {
    let style = |style: Option<&slackless_core::types::TextStyle>| {
        RunStyle::plain().with_flags(flags(style))
    };
    let muted = RunStyle {
        tone: Tone::Muted,
        ..RunStyle::plain()
    };
    match element {
        // Inline code gets the same wash as mrkdwn's backticks.
        RichTextElement::Text(el) => builder.push_text(&el.text, style(el.style.as_ref())),
        RichTextElement::Link(el) => {
            let label = el.text.clone().unwrap_or_else(|| el.url.clone());
            let run = flags(el.style.as_ref());
            // Slack draws a link inside backticks as plain code, not a link.
            if run.code {
                builder.push_text(&label, RunStyle::plain().with_flags(run));
            } else {
                push_link(builder, el.url.clone(), label, run, opts);
            }
        }
        RichTextElement::User(el) => {
            let label = format!("@{}", fixtures.mention_name(&el.user_id));
            builder.push_link(
                &label,
                LinkTarget::Profile(el.user_id.clone()),
                mention_style(true),
            );
        }
        RichTextElement::Channel(el) => {
            let label = fixtures
                .conversation(&el.channel_id)
                .and_then(|conversation| conversation.name.as_deref())
                .map(|name| format!("#{name}"))
                .unwrap_or_else(|| format!("#{}", el.channel_id));
            builder.push_link(
                &label,
                LinkTarget::Channel(el.channel_id.clone()),
                mention_style(false),
            );
        }
        RichTextElement::Usergroup(el) => {
            let label = fixtures
                .usergroup_handle(&el.usergroup_id)
                .map(|handle| format!("@{handle}"))
                .unwrap_or_else(|| "@group".to_owned());
            builder.push_text(&label, mention_style(true));
        }
        RichTextElement::Team(_) => builder.push_text("@workspace", muted),
        RichTextElement::Broadcast(el) => {
            builder.push_text(&format!("@{}", el.range.as_str()), mention_style(true));
        }
        RichTextElement::Emoji(el) => {
            let run = style(el.style.as_ref());
            if opts.emoji_text {
                builder.push_text(&format!(":{}:", el.name), run);
            } else {
                let glyph = el.unicode.as_deref().and_then(unicode_glyph).or_else(|| {
                    el.skin_tone
                        .and_then(|tone| {
                            crate::emoji::display_glyph(&format!("{}::skin-tone-{tone}", el.name))
                        })
                        .or_else(|| crate::emoji::display_glyph(&el.name))
                });
                match (glyph, fixtures.custom_emoji(&el.name)) {
                    (Some(glyph), _) => builder.push_text(&glyph, run),
                    (None, Some(image)) => builder.push_image(image.source()),
                    (None, None) => builder.push_text(&format!(":{}:", el.name), run),
                }
            }
        }
        RichTextElement::Date(el) => {
            let text = el.fallback.clone().unwrap_or_else(|| {
                if el.timestamp > 0 {
                    date_label_absolute(&CivilTime::from_unix(el.timestamp))
                } else {
                    el.format.clone()
                }
            });
            builder.push_text(&text, RunStyle::plain());
        }
        RichTextElement::Color(el) => match hex_color(&el.value) {
            Some(color) => builder.push_color_swatch(el.value.clone(), color),
            None => builder.push_text(&el.value, RunStyle::plain()),
        },
        RichTextElement::MessageMention(el) => {
            // Slack's chip for a link to a message: a speech bubble and "Author in #channel",
            // opening the message (its thread, for a reply).
            let author = el
                .author_id
                .as_ref()
                .and_then(|user| fixtures.user(user))
                .map(|user| fixtures.name_of(user).to_owned());
            let place = fixtures
                .conversation(&el.channel_id)
                .and_then(|conversation| conversation.name.as_ref())
                .map(|name| format!("#{name}"));
            let label = match (author, place) {
                (Some(author), Some(place)) => format!("{author} in {place}"),
                (Some(label), None) | (None, Some(label)) => label,
                (None, None) => el.text.clone().unwrap_or_else(|| "a message".to_owned()),
            };
            match &el.url {
                Some(url) => {
                    builder.push_chip(url.clone().into(), label, IconName::MessageCircle, 15.)
                }
                None => builder.push_text(
                    &label,
                    RunStyle {
                        tone: Tone::Reference,
                        ..RunStyle::plain()
                    },
                ),
            }
        }
        RichTextElement::AttachmentMention(el) => {
            builder.push_attachment(el.url.clone(), el.text.as_deref().unwrap_or(&el.url));
        }
        RichTextElement::Canvas(_) => builder.push_text("[canvas]", muted),
        RichTextElement::Unknown { .. } => {}
    }
}

/// Whether `message` is a locally preserved deleted message.
pub(crate) fn is_deleted_ghost(message: &Message) -> bool {
    message
        .extra
        .get(crate::bridge::DELETED_EXTRA_KEY)
        .and_then(|v| v.as_bool())
        == Some(true)
}

/// Whether the body's final rendered paragraph is plain text: a rich-text section or list
/// at the end of the blocks, or the mrkdwn fallback when no block renders anything. Slack's
/// edit marker rides inline at that paragraph's end; any other body (Block Kit block,
/// attachment, image) puts the marker on its own line after the body.
pub(in crate::messages) fn body_ends_in_text(message: &Message) -> bool {
    if !message.attachments.is_empty() {
        return false;
    }
    let fallback_text = message
        .blocks
        .iter()
        .all(|block| matches!(block, Block::RichText(rich) if rich.elements.is_empty()))
        && !message.text.trim().is_empty();
    let ends_in_paragraph = matches!(
        message.blocks.last(),
        Some(Block::RichText(rich))
            if matches!(
                rich.elements.last(),
                // After a list Slack puts the edit marker on its own line.
                Some(RichTextContainer::Section(_))
            )
    );
    fallback_text || ends_in_paragraph
}

/// One `rich_text` block's containers, or a Block Kit block (rendered live, from the message).
pub(in crate::messages) enum BodyPart {
    Rich(Vec<Container>),
    Kit,
}

/// A message's text modeled once: one part per block, the mrkdwn fallback (modeled when
/// first needed: only a body no block renders anything falls back to it) and whether the
/// edit marker rides the body's last paragraph.
pub(in crate::messages) struct BodyModel {
    pub(in crate::messages) parts: Vec<BodyPart>,
    pub(in crate::messages) edited_inline: bool,
    /// The message was edited (the marker rides inline or on its own line).
    pub(in crate::messages) edited: bool,
    fallback: RefCell<Option<Rc<Vec<MrkdwnBlock>>>>,
    text: String,
}

impl BodyModel {
    pub(in crate::messages) fn new(message: &Message, fixtures: &Fixtures, opts: TextOpts) -> Self {
        let edited = message.edited.is_some();
        let edited_inline = edited && body_ends_in_text(message);
        let last_block = message.blocks.len().saturating_sub(1);
        Self {
            parts: message
                .blocks
                .iter()
                .enumerate()
                .map(|(ix, block)| match block {
                    Block::RichText(rich) => BodyPart::Rich(containers(
                        &rich.elements,
                        fixtures,
                        opts,
                        edited_inline && ix == last_block,
                    )),
                    Block::Unknown { .. } => BodyPart::Kit,
                })
                .collect(),
            edited_inline,
            fallback: RefCell::new(None),
            text: message.text.clone(),
            edited,
        }
    }

    /// The mrkdwn fallback, carrying the edit marker when the body does not put it on its
    /// own trailing line.
    pub(in crate::messages) fn fallback(
        &self,
        fixtures: &Fixtures,
        opts: TextOpts,
    ) -> Rc<Vec<MrkdwnBlock>> {
        self.fallback
            .borrow_mut()
            .get_or_insert_with(|| {
                Rc::new(mrkdwn_blocks(
                    &self.text,
                    fixtures,
                    opts,
                    self.edited_inline,
                ))
            })
            .clone()
    }
}

thread_local! {
    /// The Block Kit text modeled this session, by text and `verbatim`, against the
    /// workspace data and emoji preference the memo holds; a full memo starts over.
    static MRKDWN: RefCell<Option<MrkdwnMemo>> = const { RefCell::new(None) };
}

struct MrkdwnMemo {
    fixtures: Arc<Fixtures>,
    emoji_text: bool,
    /// Modeled blocks by text, one table per `verbatim` value.
    blocks: [HashMap<String, Rc<Vec<MrkdwnBlock>>>; 2],
}

/// Texts kept per `verbatim` value before the memo starts over.
const MRKDWN_MEMO_CAP: usize = 512;

/// Block Kit text objects (sections, contexts, attachment text) re-render every frame: their
/// mrkdwn is modeled once per text (and `verbatim`) while the workspace data and the emoji
/// preference hold, the way [`BodyCache`] models a message body once.
pub(in crate::messages) fn memoized_mrkdwn(
    text: &str,
    verbatim: bool,
    fixtures: &Arc<Fixtures>,
    emoji_text: bool,
) -> Rc<Vec<MrkdwnBlock>> {
    MRKDWN.with(|memo| {
        let mut memo = memo.borrow_mut();
        // Names and custom emoji resolve through the workspace data and the preference.
        let stale = memo.as_ref().is_some_and(|known| {
            !known.fixtures.shares_data(fixtures) || known.emoji_text != emoji_text
        });
        if stale {
            *memo = None;
        }
        let known = memo.get_or_insert_with(|| MrkdwnMemo {
            fixtures: fixtures.clone(),
            emoji_text,
            blocks: Default::default(),
        });
        let texts = &mut known.blocks[usize::from(verbatim)];
        if let Some(blocks) = texts.get(text) {
            return blocks.clone();
        }
        let blocks = Rc::new(mrkdwn_blocks(
            text,
            fixtures,
            TextOpts {
                emoji_text,
                verbatim,
            },
            false,
        ));
        if texts.len() >= MRKDWN_MEMO_CAP {
            texts.clear();
        }
        texts.insert(text.to_owned(), blocks.clone());
        blocks
    })
}

/// Bodies modeled so far, by message allocation: a hit is the same `Rc<Message>` with the
/// same edit, modeled against the same workspace data and emoji preference.
#[derive(Default)]
pub(in crate::messages) struct BodyCache {
    fixtures: Option<Arc<Fixtures>>,
    emoji_text: Option<bool>,
    entries: HashMap<usize, CachedBody>,
}

/// One cached body: the message it was modeled from (held, so its allocation cannot be
/// reused for another message while the entry lives), the edit it carried, and the model.
struct CachedBody {
    message: Rc<Message>,
    edited: Option<Edited>,
    model: Rc<BodyModel>,
}

/// Entries kept before the cache starts over (a long transcript renders far fewer rows).
const MAX_BODIES: usize = 256;

impl BodyCache {
    /// The message's body model, modeled on a miss. Every entry is dropped when the
    /// workspace data or the emoji preference changes, since names and emoji resolve through
    /// them.
    pub(in crate::messages) fn get_or_insert(
        &mut self,
        message: &Rc<Message>,
        fixtures: &Arc<Fixtures>,
        emoji_text: bool,
    ) -> Rc<BodyModel> {
        if !self
            .fixtures
            .as_ref()
            .is_some_and(|known| known.shares_data(fixtures))
            || self.emoji_text != Some(emoji_text)
        {
            self.fixtures = Some(fixtures.clone());
            self.emoji_text = Some(emoji_text);
            self.entries.clear();
        }
        let key = Rc::as_ptr(message) as usize;
        if let Some(cached) = self.entries.get(&key)
            && Rc::ptr_eq(&cached.message, message)
            && cached.edited == message.edited
        {
            return cached.model.clone();
        }
        if self.entries.len() >= MAX_BODIES {
            self.entries.clear();
        }
        let model = Rc::new(BodyModel::new(
            message,
            fixtures,
            TextOpts {
                emoji_text,
                verbatim: false,
            },
        ));
        self.entries.insert(
            key,
            CachedBody {
                message: message.clone(),
                edited: message.edited.clone(),
                model: model.clone(),
            },
        );
        model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slackless_core::types::rich_text::{EmojiElement, RichTextSection, TextElement};

    #[test]
    fn rich_text_emoji_catalog_fallback_and_list_offset_numbering() {
        assert_eq!(unicode_glyph(""), None);
        assert_eq!(unicode_glyph("d800"), None);
        assert_eq!(unicode_glyph("110000"), None);
        assert_eq!(unicode_glyph("1f389").as_deref(), Some("🎉"));

        let fixtures = crate::fixtures::load();
        let elements = vec![
            RichTextContainer::Section(RichTextSection {
                elements: vec![RichTextElement::Emoji(EmojiElement {
                    name: "wave".into(),
                    unicode: None,
                    skin_tone: Some(3),
                    style: None,
                })],
            }),
            RichTextContainer::List(RichTextList {
                style: ListStyle::Ordered,
                indent: Some(1),
                offset: Some(4),
                border: None,
                elements: vec![
                    RichTextContainer::Section(RichTextSection {
                        elements: vec![RichTextElement::Text(TextElement {
                            text: "fifth".into(),
                            style: None,
                        })],
                    }),
                    RichTextContainer::Section(RichTextSection {
                        elements: vec![RichTextElement::Text(TextElement {
                            text: "sixth".into(),
                            style: None,
                        })],
                    }),
                ],
            }),
        ];

        let containers = containers(
            &elements,
            &fixtures,
            TextOpts {
                emoji_text: false,
                verbatim: false,
            },
            false,
        );
        assert_eq!(containers.len(), 2);
        match &containers[0] {
            Container::Paragraph(segments) => {
                let joined: String = segments
                    .iter()
                    .filter_map(|s| match s {
                        Segment::Text { pieces, .. } => {
                            Some(pieces.iter().map(|p| p.text.as_ref()).collect::<String>())
                        }
                        _ => None,
                    })
                    .collect();
                assert_eq!(joined, "👋🏼");
            }
            _ => panic!("expected Paragraph for emoji section"),
        }
        match &containers[1] {
            Container::List {
                indent,
                ordered,
                items,
            } => {
                assert_eq!(*indent, 1, "list offset must not inflate horizontal indent");
                assert!(*ordered);
                let markers: Vec<&str> = items.iter().map(|(m, _)| m.as_ref()).collect();
                assert_eq!(markers, vec!["5.", "6."]);
            }
            _ => panic!("expected List"),
        }
    }
}
