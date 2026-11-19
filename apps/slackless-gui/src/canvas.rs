//! Read-only canvas view: the conversation's Canvas tab renders a Slack canvas document
//! (headings, paragraphs, bullets, a checklist, a table, dividers) from a fixture
//! (docs/research/api-canvases-lists.md). Editing is deferred: canvases are rendered, never
//! mutated, and the banner says so.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, Div, FontWeight, InteractiveElement as _, ParentElement as _, Stateful,
    StatefulInteractiveElement as _, Styled as _, div, px,
};

/// One block of a canvas document, in reading order.
enum CanvasBlock {
    /// `level` 1-3, like the canvas editor's H1/H2/H3.
    Heading {
        level: u8,
        text: &'static str,
    },
    Paragraph(&'static str),
    Bullets(&'static [&'static str]),
    /// `(done, item)` pairs.
    Checklist(&'static [(bool, &'static str)]),
    Table {
        header: &'static [&'static str],
        rows: &'static [&'static [&'static str]],
    },
    Divider,
}

struct Canvas {
    title: &'static str,
    meta: &'static str,
    blocks: Vec<CanvasBlock>,
}

/// The sample canvas every conversation shows in fixture mode.
fn fixture() -> Canvas {
    Canvas {
        title: "Q4 launch plan",
        meta: "Canvas · last edited by Maya Chen",
        blocks: vec![
            CanvasBlock::Heading {
                level: 1,
                text: "Q4 launch plan",
            },
            CanvasBlock::Paragraph(
                "This canvas tracks everything we need to ship the desktop client before the \
                 end of the quarter. Owners update their rows; the channel gets a weekly digest.",
            ),
            CanvasBlock::Heading {
                level: 2,
                text: "Goals",
            },
            CanvasBlock::Bullets(&[
                "Native desktop client with feature parity for messaging",
                "Offline search over the local archive",
                "Huddles via the browser fallback until native media lands",
            ]),
            CanvasBlock::Heading {
                level: 2,
                text: "Launch checklist",
            },
            CanvasBlock::Checklist(&[
                (true, "Sign the macOS build"),
                (true, "Fixture-driven screenshots for the docs"),
                (false, "Beta invite list reviewed by support"),
                (false, "Release notes drafted"),
            ]),
            CanvasBlock::Divider,
            CanvasBlock::Heading {
                level: 2,
                text: "Milestones",
            },
            CanvasBlock::Table {
                header: &["Milestone", "Owner", "Due", "Status"],
                rows: &[
                    &["Private beta", "Maya Chen", "Oct 14", "Done"],
                    &["Public beta", "Leo Park", "Nov 4", "In progress"],
                    &["General availability", "Sam Rivera", "Dec 2", "Not started"],
                ],
            },
            CanvasBlock::Heading {
                level: 3,
                text: "Open questions",
            },
            CanvasBlock::Paragraph(
                "Do we gate huddles behind a setting while they open in the browser?",
            ),
        ],
    }
}

/// The Canvas tab body: a scrollable, centered, read-only document.
pub fn canvas_view(cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let canvas = fixture();

    let header = h_flex()
        .items_center()
        .gap_2()
        .child(
            Icon::new(IconName::LayoutTemplate)
                .text_color(theme.link)
                .size(px(18.)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(canvas.title))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(canvas.meta),
                ),
        )
        .child(read_only_badge(cx));

    let document = canvas.blocks.iter().fold(v_flex().gap_3(), |doc, block| {
        doc.child(render_block(block, cx))
    });

    div()
        .id("canvas-scroll")
        .size_full()
        .overflow_y_scroll()
        .child(
            v_flex()
                .w_full()
                .max_w(px(760.))
                .mx_auto()
                .px_8()
                .py_6()
                .gap_5()
                .child(header)
                .child(document),
        )
}

/// "Read-only" pill shared by the canvas and list headers.
pub fn read_only_badge(cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .flex_none()
        .items_center()
        .gap_1()
        .px_2()
        .py_0p5()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(
            Icon::new(IconName::Lock)
                .text_color(theme.muted_foreground)
                .size(px(12.)),
        )
        .child("Read-only · editing not supported yet")
}

fn render_block(block: &CanvasBlock, cx: &App) -> Div {
    let theme = cx.theme();
    match block {
        CanvasBlock::Heading { level, text } => {
            let heading = div()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.foreground);
            let heading = match level {
                1 => heading.text_2xl(),
                2 => heading.text_lg().pt_2(),
                _ => heading.text_base(),
            };
            heading.child(*text)
        }
        CanvasBlock::Paragraph(text) => div().text_sm().line_height(px(22.)).child(*text),
        CanvasBlock::Bullets(items) => v_flex().gap_1().pl_2().children(items.iter().map(|item| {
            h_flex()
                .gap_2()
                .items_start()
                .text_sm()
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.muted_foreground)
                        .child("•"),
                )
                .child(div().flex_1().child(*item))
        })),
        CanvasBlock::Checklist(items) => {
            v_flex()
                .gap_1p5()
                .pl_1()
                .children(items.iter().map(|(done, item)| {
                    let (icon, color) = if *done {
                        (IconName::SquareCheck, theme.success)
                    } else {
                        (IconName::Square, theme.muted_foreground)
                    };
                    h_flex()
                        .gap_2()
                        .items_center()
                        .text_sm()
                        .child(Icon::new(icon).text_color(color).size(px(16.)))
                        .child(
                            div()
                                .when(*done, |text| {
                                    text.line_through().text_color(theme.muted_foreground)
                                })
                                .child(*item),
                        )
                }))
        }
        CanvasBlock::Table { header, rows } => {
            let cell = |text: &'static str| {
                div()
                    .flex_1()
                    .min_w_0()
                    .px_3()
                    .py_2()
                    .truncate()
                    .child(text)
            };
            let header_row = h_flex()
                .bg(theme.muted)
                .font_weight(FontWeight::SEMIBOLD)
                .children(header.iter().map(|text| cell(text)));
            v_flex()
                .w_full()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .overflow_hidden()
                .text_sm()
                .child(header_row)
                .children(rows.iter().map(|row| {
                    h_flex()
                        .border_t_1()
                        .border_color(theme.border)
                        .children(row.iter().map(|text| cell(text)))
                }))
        }
        CanvasBlock::Divider => div().h(px(1.)).w_full().bg(theme.border),
    }
}
