//! Block-level rich-text containers, lists, message body layout, and top-level `mrkdwn` / `render_block`.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, Div, FontWeight, IntoElement, ParentElement as _, Styled as _, div, px,
};
use slackless_core::types::rich_text::{
    ListStyle, RichTextContainer, RichTextElement, RichTextList,
};
use slackless_core::types::{Block, Message};

use super::super::LinkTarget;
use super::cards::render_huddle_thread_card;
use super::inline::{
    ParagraphBuilder, RunFlags, RunTheme, flags, hex_color, jumbomoji_elements, mention_flags,
    mrkdwn_fences, mrkdwn_lines, render_segments, unescape, unicode_glyph,
};
use super::{CivilTime, code_block, date_label_absolute, render_code};
use crate::blockkit::{self, KitCx};
use crate::emoji;
use crate::fixtures::Fixtures;

pub(in crate::messages) fn paragraph(
    elements: &[RichTextElement],
    fixtures: &Fixtures,
    theme: &RunTheme,
    edited: bool,
) -> Option<Div> {
    let mut builder = ParagraphBuilder::new();
    for element in elements {
        push_element(&mut builder, element, fixtures, theme);
    }
    builder.trim_trailing_breaks();
    if edited {
        builder.push_edited(theme);
    }
    let segments = builder.finish();
    if segments.is_empty() {
        return None;
    }
    Some(render_segments(segments, theme))
}

pub(in crate::messages) fn push_element(
    builder: &mut ParagraphBuilder,
    element: &RichTextElement,
    fixtures: &Fixtures,
    theme: &RunTheme,
) {
    match element {
        RichTextElement::Text(el) => {
            // Inline code gets the same wash as mrkdwn's backticks.
            let run = flags(el.style.as_ref());
            let bg = run.code.then_some(theme.code_bg);
            builder.push_text(&el.text, theme, run, None, bg);
        }
        RichTextElement::Link(el) => {
            let label = el.text.clone().unwrap_or_else(|| el.url.clone());
            // Slack draws a link inside backticks as plain code, not a link.
            let run = flags(el.style.as_ref());
            if run.code {
                builder.push_text(&label, theme, run, None, Some(theme.code_bg));
            } else {
                builder.push_link(el.url.clone(), label, run, theme);
            }
        }
        RichTextElement::User(el) => {
            let label = format!("@{}", fixtures.mention_name(&el.user_id));
            builder.push_mention(
                &label,
                LinkTarget::Profile(el.user_id.clone()),
                theme,
                Some(theme.mention_bg),
            );
        }
        RichTextElement::Channel(el) => {
            let label = fixtures
                .conversation(&el.channel_id)
                .map(|conversation| fixtures.conversation_label(conversation))
                .unwrap_or_else(|| format!("#{}", el.channel_id));
            builder.push_mention(
                &label,
                LinkTarget::Channel(el.channel_id.clone()),
                theme,
                None,
            );
        }
        RichTextElement::Usergroup(el) => {
            let label = fixtures
                .usergroup_handle(&el.usergroup_id)
                .map(|handle| format!("@{handle}"))
                .unwrap_or_else(|| "@group".to_owned());
            builder.push_text(
                &label,
                theme,
                mention_flags(),
                Some(theme.link),
                Some(theme.mention_bg),
            );
        }
        RichTextElement::Team(_) => {
            builder.push_text(
                "@workspace",
                theme,
                RunFlags::default(),
                Some(theme.muted),
                None,
            );
        }
        RichTextElement::Broadcast(el) => {
            let label = format!("@{}", el.range.as_str());
            builder.push_text(
                &label,
                theme,
                mention_flags(),
                Some(theme.link),
                Some(theme.mention_bg),
            );
        }
        RichTextElement::Emoji(el) => {
            if theme.emoji_text_mode {
                let literal = format!(":{}:", el.name);
                builder.push_text(&literal, theme, flags(el.style.as_ref()), None, None);
            } else {
                let glyph = el
                    .unicode
                    .as_deref()
                    .and_then(unicode_glyph)
                    .or_else(|| emoji::display_glyph(&el.name));
                match (glyph, fixtures.custom_emoji(&el.name)) {
                    (Some(glyph), _) => {
                        builder.push_text(&glyph, theme, flags(el.style.as_ref()), None, None);
                    }
                    (None, Some(image)) => builder.push_image(image.source()),
                    (None, None) => {
                        let literal = format!(":{}:", el.name);
                        builder.push_text(&literal, theme, flags(el.style.as_ref()), None, None);
                    }
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
            builder.push_text(&text, theme, RunFlags::default(), None, None);
        }
        RichTextElement::Color(el) => {
            if let Some(color) = hex_color(&el.value) {
                builder.push_color_swatch(el.value.clone(), color);
            } else {
                builder.push_text(&el.value, theme, RunFlags::default(), None, None);
            }
        }
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
                None => {
                    builder.push_text(&label, theme, RunFlags::default(), Some(theme.link), None)
                }
            }
        }
        RichTextElement::AttachmentMention(el) => {
            builder.push_attachment(el.url.clone(), el.text.as_deref().unwrap_or(&el.url), theme);
        }
        RichTextElement::Canvas(_) => {
            builder.push_text(
                "[canvas]",
                theme,
                RunFlags::default(),
                Some(theme.muted),
                None,
            );
        }
        RichTextElement::Unknown { .. } => {}
    }
}

/// Block-level rich text containers: paragraphs, lists, quotes and code blocks. With
/// `edited`, the final container's plain text (a section, or a list's last item) carries the
/// edit marker inline at its end; boxed text (quotes) and code blocks leave it to the body's
/// own trailing line.
pub(in crate::messages) fn render_containers(
    containers: &[RichTextContainer],
    fixtures: &Fixtures,
    theme: &RunTheme,
    edited: bool,
) -> Vec<AnyElement> {
    containers
        .iter()
        .enumerate()
        .filter_map(|(n, container)| {
            let edited = edited && n + 1 == containers.len();
            match container {
                RichTextContainer::Section(section) => {
                    paragraph(&section.elements, fixtures, theme, edited)
                        .map(IntoElement::into_any_element)
                }
                RichTextContainer::List(list) if !list.elements.is_empty() => {
                    Some(render_list(list, fixtures, theme, edited).into_any_element())
                }
                RichTextContainer::Quote(quote) => {
                    paragraph(&quote.elements, fixtures, theme, false).map(|paragraph| {
                        div()
                            .border_l_2()
                            .border_color(theme.border)
                            .pl_3()
                            .child(paragraph)
                            .into_any_element()
                    })
                }
                RichTextContainer::Preformatted(preformatted) => {
                    Some(render_code(preformatted, theme).into_any_element())
                }
                RichTextContainer::Header(header) => {
                    paragraph(&header.elements, fixtures, theme, false).map(|paragraph| {
                        // Slack's `.p-rich_text_header`: 18px Black for Big, 15px Black for
                        // Medium and 15px Bold for Small, each on a 22.5px line, with 1em
                        // above (none as the first container) and 4px below.
                        let (size, weight) = match header.level.unwrap_or(2).clamp(1, 3) {
                            1 => (18., FontWeight::BLACK),
                            2 => (15., FontWeight::BLACK),
                            _ => (15., FontWeight::BOLD),
                        };
                        div()
                            .when(n > 0, |d| d.mt(px(15.)))
                            .mb(px(4.))
                            .text_size(px(size))
                            .line_height(px(22.5))
                            .font_weight(weight)
                            .child(paragraph)
                            .into_any_element()
                    })
                }
                RichTextContainer::Divider(_) => {
                    // Slack's `.p-rich_text_divider`: a full-width rule, 8px clear above and
                    // below.
                    Some(
                        div()
                            .w_full()
                            .my_2()
                            .h(px(1.))
                            .bg(theme.border)
                            .into_any_element(),
                    )
                }
                _ => None,
            }
        })
        .collect()
}

pub(in crate::messages) fn render_list(
    list: &RichTextList,
    fixtures: &Fixtures,
    theme: &RunTheme,
    edited: bool,
) -> Div {
    let depth = list
        .indent
        .unwrap_or(0)
        .saturating_add(list.offset.unwrap_or(0));
    // Slack's disc sits 10px in, its text 28px in, nesting 22px a level.
    let indent = px(10. + 22. * depth as f32);
    let mut column = v_flex().gap_1();
    for (n, item) in list.elements.iter().enumerate() {
        let elements: &[RichTextElement] = match item {
            RichTextContainer::Section(section) => &section.elements,
            _ => &[],
        };
        if elements.is_empty() {
            continue;
        }
        let ordered = matches!(list.style, ListStyle::Ordered);
        let marker = match &list.style {
            ListStyle::Ordered => format!("{}.", n + 1),
            _ => "•".to_owned(),
        };
        let last = n + 1 == list.elements.len();
        if let Some(item_row) = paragraph(elements, fixtures, theme, edited && last) {
            column = column.child(
                h_flex()
                    .items_start()
                    .pl(indent)
                    .child(
                        div()
                            .flex_none()
                            .w(px(18.))
                            .when(!ordered, |marker| {
                                marker.text_size(px(20.)).line_height(px(22.))
                            })
                            .text_color(theme.foreground)
                            .child(marker),
                    )
                    .child(item_row),
            );
        }
    }
    column
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

/// Whether `message` is a locally preserved deleted message.
pub(crate) fn is_deleted_ghost(message: &Message) -> bool {
    message
        .extra
        .get(crate::bridge::DELETED_EXTRA_KEY)
        .and_then(|v| v.as_bool())
        == Some(true)
}

pub(in crate::messages) fn message_body(message: &Message, kcx: &KitCx, cx: &App) -> Div {
    let has_kit = message
        .blocks
        .iter()
        .any(|block| matches!(block, Block::Unknown { .. }));
    let mut column = v_flex()
        .min_w_0()
        .gap_1()
        .text_sm()
        .when(has_kit || !message.attachments.is_empty(), |column| {
            column.gap_2()
        });
    let edited = message.edited.is_some();
    let inline_edited = edited && body_ends_in_text(message);
    if message.subtype == Some(slackless_core::types::MessageSubtype::HuddleThread)
        && let Some(card) = render_huddle_thread_card(message, kcx, cx)
    {
        return column.child(card);
    }
    let attachments = blockkit::attachments(message, kcx, cx);
    // Jumbomoji: when enabled (and not in plain-text emoji mode) and the message is 1..=5 emoji only, render them large (32px).
    let display = crate::preferences::display_prefs(cx);
    if display.jumbomoji
        && !display.emoji_text_mode
        && !has_kit
        && message.attachments.is_empty()
        && message.files.is_empty()
        && let Some(jumbos) = jumbomoji_elements(&message.text, kcx.fixtures)
    {
        column = column.child(h_flex().items_center().gap_1p5().children(jumbos));
        if edited {
            let muted = cx.theme().muted_foreground;
            column = column.child(div().text_xs().text_color(muted).child("(edited)"));
        }
        return column;
    }
    let mut rendered = 0;
    let last_block = message.blocks.len().saturating_sub(1);
    for (ix, block) in message.blocks.iter().enumerate() {
        let elements = render_block(block, ix, kcx, inline_edited && ix == last_block, cx);
        rendered += elements.len();
        column = column.children(elements);
    }
    if rendered == 0 && !message.text.is_empty() {
        column = column.child(mrkdwn(&message.text, kcx, false, inline_edited, cx));
    }
    if edited && !inline_edited {
        let muted = cx.theme().muted_foreground;
        column = column.child(div().text_xs().text_color(muted).child("(edited)"));
    }
    column.children(attachments)
}

/// One block of a message (or attachment): `rich_text` here, everything else through
/// [`blockkit::block`].
pub(crate) fn render_block(
    block: &Block,
    ix: usize,
    kcx: &KitCx,
    edited: bool,
    cx: &App,
) -> Vec<AnyElement> {
    match block {
        Block::RichText(rich) => {
            let (key, order) = kcx.text_theme();
            render_containers(
                &rich.elements,
                kcx.fixtures,
                &RunTheme::body(cx, key.into(), order, kcx.links.clone()),
                edited,
            )
        }
        Block::Unknown { r#type, raw } => blockkit::block(r#type, raw, ix, kcx, cx)
            .into_iter()
            .collect(),
    }
}

/// Renders Slack mrkdwn (fallback text, Block Kit text objects, attachment text), one line
/// per paragraph: `*bold*`, `_italic_`, `~strike~`, `` `code` ``, `<@user>` / `<#channel>` /
/// `<!here>` chips, `<url|label>` links and `:emoji:` (custom emoji as images). `muted`
/// renders plain runs in the muted color (Block Kit context blocks). Unknown markup stays
/// literal. With a link handler (see [`KitCx`]) the text is selectable and its links route
/// through it.
pub(crate) fn mrkdwn(text: &str, kcx: &KitCx, muted: bool, edited: bool, cx: &App) -> Div {
    mrkdwn_with_verbatim(text, kcx, muted, edited, false, cx)
}

/// Render Slack `mrkdwn`, honoring Block Kit's `verbatim` flag (`F-COLLAB-758`): when `verbatim`
/// is `true`, raw un-bracketed URLs stay literal instead of being auto-linked.
pub(crate) fn mrkdwn_with_verbatim(
    text: &str,
    kcx: &KitCx,
    muted: bool,
    edited: bool,
    verbatim: bool,
    cx: &App,
) -> Div {
    let (key, order) = kcx.text_theme();
    let mut theme = RunTheme::body(cx, key.into(), order, kcx.links.clone());
    theme.verbatim = verbatim;
    if muted {
        theme.foreground = theme.muted;
    }
    let pieces = mrkdwn_fences(text);
    let mut column = v_flex().min_w_0();
    for (n, (fenced, piece)) in pieces.iter().enumerate() {
        column = if *fenced {
            column.child(
                div()
                    .my_1()
                    .child(code_block(unescape(piece), None, &theme)),
            )
        } else {
            mrkdwn_lines(
                column,
                piece,
                kcx.fixtures,
                &theme,
                edited && n + 1 == pieces.len(),
            )
        };
    }
    // A body ending in a pre block puts the edit marker on its own line after it.
    if edited && pieces.last().is_some_and(|(fenced, _)| *fenced) {
        column = column.child(div().text_xs().text_color(theme.muted).child("(edited)"));
    }
    column
}
