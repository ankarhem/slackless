//! Block-level layout of a message body: rich-text containers, lists, the body itself (its
//! model cached per message in the transcript), and the `mrkdwn` entry point Block Kit text
//! objects render through.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, Div, FontWeight, IntoElement, ParentElement as _, Styled as _, div, px,
};
use slackless_core::types::{Block, Message};

use super::super::code_block::code_block;
use super::body::{
    BodyCache, BodyModel, BodyPart, Container, MrkdwnBlock, containers, memoized_mrkdwn,
};
use super::cards::render_huddle_thread_card;
use super::inline::{RunTheme, jumbomoji_elements, render_segments};
use super::model::TextOpts;
use crate::blockkit::{self, KitCx};
use crate::fixtures::Fixtures;
use crate::preferences::display_prefs;

/// The containers of one `rich_text` block: paragraphs, lists, quotes, headers and rules laid
/// out as Slack's `.p-rich_text_*` blocks.
pub(in crate::messages) fn render_containers(
    containers: &[Container],
    theme: &RunTheme,
) -> Vec<AnyElement> {
    containers
        .iter()
        .map(|container| match container {
            Container::Paragraph(segments) => render_segments(segments, theme).into_any_element(),
            Container::List {
                indent,
                ordered,
                items,
            } => render_list(*indent, *ordered, items, theme).into_any_element(),
            Container::Quote(segments) => div()
                .border_l_2()
                .border_color(theme.border)
                .pl_3()
                .child(render_segments(segments, theme))
                .into_any_element(),
            Container::Code(code) => code_block(code, theme).into_any_element(),
            Container::Header {
                level,
                segments,
                space_above,
            } => {
                // Slack's `.p-rich_text_header`: 18px Black for Big, 15px Black for
                // Medium and 15px Bold for Small, each on a 22.5px line, with 1em
                // above (none as the first container) and 4px below.
                let (size, weight) = match level {
                    1 => (18., FontWeight::BLACK),
                    2 => (15., FontWeight::BLACK),
                    _ => (15., FontWeight::BOLD),
                };
                div()
                    .when(*space_above, |header| header.mt(px(15.)))
                    .mb(px(4.))
                    .text_size(px(size))
                    .line_height(px(22.5))
                    .font_weight(weight)
                    .child(render_segments(segments, theme))
                    .into_any_element()
            }
            // Slack's `.p-rich_text_divider`: a full-width rule, 8px clear above and below.
            Container::Divider => div()
                .w_full()
                .my_2()
                .h(px(1.))
                .bg(theme.border)
                .into_any_element(),
        })
        .collect()
}

/// A rich-text list: Slack's disc sits 10px in, its text 28px in, nesting 22px a level.
fn render_list(
    indent: u32,
    ordered: bool,
    items: &[(gpui_kit::gpui::SharedString, Vec<super::model::Segment>)],
    theme: &RunTheme,
) -> Div {
    let indent = px(10. + 22. * indent as f32);
    let mut column = v_flex().gap_1();
    for (marker, segments) in items {
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
                        .child(marker.clone()),
                )
                .child(render_segments(segments, theme)),
        );
    }
    column
}

/// A mrkdwn body's blocks laid out: paragraphs, blank-line breaks, quote bars and code
/// blocks, the edit marker on its own line after a closing code block.
pub(in crate::messages) fn render_mrkdwn(blocks: &[MrkdwnBlock], theme: &RunTheme) -> Div {
    let mut column = v_flex().min_w_0();
    for block in blocks {
        column = match block {
            MrkdwnBlock::Code(code) => column.child(div().my_1().child(code_block(code, theme))),
            MrkdwnBlock::Break => column.child(div().h(px(8.))),
            MrkdwnBlock::Line(segments) if !segments.is_empty() => {
                column.child(render_segments(segments, theme))
            }
            MrkdwnBlock::Quote(lines) => {
                let mut quote_col = v_flex().flex_1().min_w_0();
                for line in lines {
                    quote_col = match line {
                        None => quote_col.child(div().h(px(6.))),
                        Some(segments) if !segments.is_empty() => {
                            quote_col.child(render_segments(segments, theme))
                        }
                        Some(_) => quote_col,
                    };
                }
                column.child(
                    h_flex()
                        .my_0p5()
                        .gap_2p5()
                        .items_stretch()
                        .min_w_0()
                        .child(div().flex_none().w(px(4.)).rounded_full().bg(theme.border))
                        .child(quote_col),
                )
            }
            MrkdwnBlock::Line(_) => column,
            MrkdwnBlock::Edited => {
                column.child(div().text_xs().text_color(theme.muted).child("(edited)"))
            }
        };
    }
    column
}

/// The body of `message` in the transcript: its [`BodyModel`] from the list's `cache`, so a
/// frame renders without parsing the message again.
pub(in crate::messages) fn message_body_cached(
    message: &Rc<Message>,
    cache: &RefCell<BodyCache>,
    fixtures: &Arc<Fixtures>,
    kcx: &KitCx,
    cx: &App,
) -> Div {
    let model =
        cache
            .borrow_mut()
            .get_or_insert(message, fixtures, display_prefs(cx).emoji_text_mode);
    render_body(message, &model, kcx, cx)
}

/// The body of `message` outside the transcript (cards, pop-outs): modeled for this render.
pub(in crate::messages) fn message_body(message: &Message, kcx: &KitCx, cx: &App) -> Div {
    let model = BodyModel::new(
        message,
        kcx.fixtures,
        TextOpts {
            emoji_text: display_prefs(cx).emoji_text_mode,
            verbatim: false,
        },
    );
    render_body(message, &model, kcx, cx)
}

fn render_body(message: &Message, model: &BodyModel, kcx: &KitCx, cx: &App) -> Div {
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
    if message.subtype == Some(slackless_core::types::MessageSubtype::HuddleThread)
        && let Some(card) = render_huddle_thread_card(message, kcx, cx)
    {
        return column.child(card);
    }
    let attachments = blockkit::attachments(message, kcx, cx);
    // Jumbomoji: when enabled (and not in plain-text emoji mode) and the message is 1..=5 emoji only, render them large (32px).
    let display = display_prefs(cx);
    if display.jumbomoji
        && !display.emoji_text_mode
        && !has_kit
        && message.attachments.is_empty()
        && message.files.is_empty()
        && let Some(jumbos) = jumbomoji_elements(&message.text, kcx.fixtures)
    {
        column = column.child(h_flex().items_center().gap_1p5().children(jumbos));
        if model.edited {
            let muted = cx.theme().muted_foreground;
            column = column.child(div().text_xs().text_color(muted).child("(edited)"));
        }
        return column;
    }
    let opts = TextOpts {
        emoji_text: display.emoji_text_mode,
        verbatim: false,
    };
    let mut rendered = 0;
    for (ix, (block, part)) in message.blocks.iter().zip(&model.parts).enumerate() {
        let elements: Vec<AnyElement> = match part {
            BodyPart::Rich(containers) => {
                let (key, order) = kcx.text_theme();
                render_containers(
                    containers,
                    &RunTheme::body(cx, key.into(), order, kcx.links.clone()),
                )
            }
            BodyPart::Kit => match block {
                Block::Unknown { r#type, raw } => blockkit::block(r#type, raw, ix, kcx, cx)
                    .into_iter()
                    .collect(),
                Block::RichText(_) => Vec::new(),
            },
        };
        rendered += elements.len();
        column = column.children(elements);
    }
    if rendered == 0 && !message.text.is_empty() {
        let (key, order) = kcx.text_theme();
        let theme = RunTheme::body(cx, key.into(), order, kcx.links.clone());
        column = column.child(render_mrkdwn(&model.fallback(kcx.fixtures, opts), &theme));
    }
    if model.edited && !model.edited_inline {
        let muted = cx.theme().muted_foreground;
        column = column.child(div().text_xs().text_color(muted).child("(edited)"));
    }
    column.children(attachments)
}

/// One block of a message (or attachment): `rich_text` here, everything else through
/// [`blockkit::block`]. Modeled for this render (attachments and nested blocks have no cache).
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
                &containers(
                    &rich.elements,
                    kcx.fixtures,
                    TextOpts {
                        emoji_text: display_prefs(cx).emoji_text_mode,
                        verbatim: false,
                    },
                    edited,
                ),
                &RunTheme::body(cx, key.into(), order, kcx.links.clone()),
            )
        }
        Block::Unknown { r#type, raw } => blockkit::block(r#type, raw, ix, kcx, cx)
            .into_iter()
            .collect(),
    }
}

/// Renders Slack mrkdwn (Block Kit text objects, attachment text), one line per paragraph:
/// `*bold*`, `_italic_`, `~strike~`, `` `code` ``, `<@user>` / `<#channel>` / `<!here>` chips,
/// `<url|label>` links and `:emoji:` (custom emoji as images). `muted` renders plain runs in
/// the muted color (Block Kit context blocks). Unknown markup stays literal. With a link
/// handler (see [`KitCx`]) the text is selectable and its links route through it. The text is
/// modeled once per text (see [`memoized_mrkdwn`]), so a frame re-renders without parsing.
pub(crate) fn mrkdwn(text: &str, kcx: &KitCx, muted: bool, cx: &App) -> Div {
    mrkdwn_with_verbatim(text, kcx, muted, false, cx)
}

/// Render Slack `mrkdwn`, honoring Block Kit's `verbatim` flag (`F-COLLAB-758`): when `verbatim`
/// is `true`, raw un-bracketed URLs stay literal instead of being auto-linked.
pub(crate) fn mrkdwn_with_verbatim(
    text: &str,
    kcx: &KitCx,
    muted: bool,
    verbatim: bool,
    cx: &App,
) -> Div {
    let (key, order) = kcx.text_theme();
    let mut theme = RunTheme::body(cx, key.into(), order, kcx.links.clone());
    if muted {
        theme.foreground = theme.muted;
    }
    let blocks = memoized_mrkdwn(
        text,
        verbatim,
        kcx.fixtures,
        display_prefs(cx).emoji_text_mode,
    );
    render_mrkdwn(&blocks, &theme)
}
