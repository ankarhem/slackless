//! Native read-only canvas reader (`F-COLLAB-125`, `F-SHELL-266`): renders both the sample
//! workspace's fixture canvas and live Slack canvases fetched via `files.info` -> `url_private`
//! HTML export (headings, paragraphs, bullets, checklists, code blocks, quotes, tables, dividers),
//! with an in-canvas Find bar and an "Open in browser to edit" button.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, Div, Entity, FontWeight, InteractiveElement as _, ParentElement as _, Stateful,
    StatefulInteractiveElement as _, Styled as _, Window, div, img, px,
};

/// One block of a canvas document, in reading order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanvasBlock {
    /// `level` 1-3, like the canvas editor's H1/H2/H3.
    Heading {
        level: u8,
        text: String,
    },
    Paragraph(String),
    Bullets(Vec<String>),
    /// `(done, item)` pairs.
    Checklist(Vec<(bool, String)>),
    CodeBlock(String),
    Quote(String),
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Embedded canvas image (`img[src]` with optional alt caption).
    Image {
        url: String,
        alt: String,
    },
    Divider,
}

impl CanvasBlock {
    /// Case-insensitive text search across the block's rendered text (`F-SHELL-266`).
    fn matches(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        let has = |s: &str| s.to_lowercase().contains(needle);
        match self {
            Self::Heading { text, .. }
            | Self::Paragraph(text)
            | Self::CodeBlock(text)
            | Self::Quote(text) => has(text),
            Self::Bullets(items) => items.iter().any(|s| has(s)),
            Self::Checklist(items) => items.iter().any(|(_, s)| has(s)),
            Self::Table { header, rows } => {
                header.iter().any(|s| has(s)) || rows.iter().any(|r| r.iter().any(|s| has(s)))
            }
            Self::Image { alt, url } => has(alt) || has(url),
            Self::Divider => false,
        }
    }
}

/// A parsed canvas document ready to render in the native reader.
#[derive(Clone, Debug)]
pub struct CanvasDoc {
    pub title: String,
    pub meta: String,
    pub blocks: Vec<CanvasBlock>,
}

impl CanvasDoc {
    /// Parse a live Slack canvas HTML export (`url_private`) into structured blocks.
    pub fn from_html(title: String, html: &str) -> Self {
        let blocks = parse_canvas_html(html);
        Self {
            title,
            meta: "Live canvas · read-only preview".to_owned(),
            blocks,
        }
    }

    /// Headings in document order (`(level, text)`) for the collapsible outline strip.
    pub fn outline_headings(&self) -> Vec<(u8, &str)> {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                CanvasBlock::Heading { level, text } if !text.is_empty() => {
                    Some((*level, text.as_str()))
                }
                _ => None,
            })
            .collect()
    }
}

/// The sample canvas every conversation shows in fixture mode.
fn fixture() -> CanvasDoc {
    CanvasDoc {
        title: "Q4 launch plan".into(),
        meta: "Canvas · last edited by Maya Chen".into(),
        blocks: vec![
            CanvasBlock::Heading {
                level: 1,
                text: "Q4 launch plan".into(),
            },
            CanvasBlock::Paragraph(
                "This canvas tracks everything we need to ship the desktop client before the \
                 end of the quarter. Owners update their rows; the channel gets a weekly digest."
                    .into(),
            ),
            CanvasBlock::Heading {
                level: 2,
                text: "Goals".into(),
            },
            CanvasBlock::Bullets(vec![
                "Native desktop client with feature parity for messaging".into(),
                "Offline search over the local archive".into(),
                "Huddles via the browser fallback until native media lands".into(),
            ]),
            CanvasBlock::Heading {
                level: 2,
                text: "Launch checklist".into(),
            },
            CanvasBlock::Checklist(vec![
                (true, "Sign the macOS build".into()),
                (true, "Fixture-driven screenshots for the docs".into()),
                (false, "Beta invite list reviewed by support".into()),
                (false, "Release notes drafted".into()),
            ]),
            CanvasBlock::Divider,
            CanvasBlock::Heading {
                level: 2,
                text: "Milestones".into(),
            },
            CanvasBlock::Table {
                header: vec![
                    "Milestone".into(),
                    "Owner".into(),
                    "Due".into(),
                    "Status".into(),
                ],
                rows: vec![
                    vec![
                        "Private beta".into(),
                        "Maya Chen".into(),
                        "Oct 14".into(),
                        "Done".into(),
                    ],
                    vec![
                        "Public beta".into(),
                        "Leo Park".into(),
                        "Nov 4".into(),
                        "In progress".into(),
                    ],
                    vec![
                        "General availability".into(),
                        "Sam Rivera".into(),
                        "Dec 2".into(),
                        "Not started".into(),
                    ],
                ],
            },
            CanvasBlock::Heading {
                level: 3,
                text: "Open questions".into(),
            },
            CanvasBlock::Paragraph(
                "Do we gate huddles behind a setting while they open in the browser?".into(),
            ),
        ],
    }
}

/// Callback invoked when the user clicks "Open in browser" to edit a live canvas.
pub type OpenBrowserFn = Rc<dyn Fn(&mut Window, &mut App)>;

/// The Canvas tab body in fixture mode: scrollable, centered, read-only document.
pub fn canvas_view(query: &str, search: Option<&Entity<TextareaState>>, cx: &App) -> Stateful<Div> {
    let canvas = fixture();
    render_canvas_doc(&canvas, query, search, None, cx)
}

/// Native live Canvas reader (`F-COLLAB-125`, `F-SHELL-266`): shows a loading indicator,
/// error state with browser fallback, or the parsed live canvas with an in-canvas Find input
/// and an "Open in browser" action.
pub fn live_canvas_view(
    loaded: Option<&Result<CanvasDoc, String>>,
    query: &str,
    search: Option<&Entity<TextareaState>>,
    open_browser: Option<OpenBrowserFn>,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    match loaded {
        None => v_flex()
            .id("canvas-loading")
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child("Loading canvas…")
            .when_some(open_browser, |col, open| {
                col.child(
                    Button::new("canvas-loading-browser")
                        .outline()
                        .with_size(Size::Small)
                        .icon(IconName::ExternalLink)
                        .label("Open in browser")
                        .on_click(move |_, window, cx| open(window, cx)),
                )
            }),
        Some(Err(err)) => v_flex()
            .id("canvas-error")
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .px_8()
            .text_sm()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Could not load canvas preview"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(err.clone()),
            )
            .when_some(open_browser, |col, open| {
                col.child(
                    Button::new("canvas-err-browser")
                        .outline()
                        .with_size(Size::Small)
                        .icon(IconName::ExternalLink)
                        .label("Open in browser")
                        .on_click(move |_, window, cx| open(window, cx)),
                )
            }),
        Some(Ok(doc)) => render_canvas_doc(doc, query, search, open_browser, cx),
    }
}

fn render_canvas_doc(
    canvas: &CanvasDoc,
    query: &str,
    search: Option<&Entity<TextareaState>>,
    open_browser: Option<OpenBrowserFn>,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let needle = query.trim().to_lowercase();
    let filtered: Vec<&CanvasBlock> = canvas
        .blocks
        .iter()
        .filter(|b| b.matches(&needle))
        .collect();
    let match_note = (!needle.is_empty()).then(|| {
        format!(
            "{} matching {}",
            filtered.len(),
            if filtered.len() == 1 {
                "section"
            } else {
                "sections"
            }
        )
    });

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
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(canvas.title.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(canvas.meta.clone()),
                ),
        )
        .child(read_only_badge(cx))
        .when_some(open_browser, |row, open| {
            row.child(
                Button::new("canvas-open-browser")
                    .outline()
                    .with_size(Size::XSmall)
                    .icon(IconName::ExternalLink)
                    .label("Open in browser")
                    .on_click(move |_, window, cx| open(window, cx)),
            )
        });

    let find_bar = search.map(|input| {
        h_flex()
            .h(px(32.))
            .px_2p5()
            .gap_2()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .child(
                Icon::new(IconName::Search)
                    .size(px(14.))
                    .text_color(theme.muted_foreground),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Textarea::new(input).appearance(false).bordered(false)),
            )
            .children(match_note.map(|note| {
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(note)
            }))
            .when(!needle.is_empty(), |row| {
                let clear_input = input.clone();
                row.child(
                    Button::new("canvas-find-clear")
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::X)
                        .on_click(move |_, window, cx| {
                            clear_input.update(cx, |state, cx| {
                                state.set_value(String::new(), window, cx);
                            });
                        }),
                )
            })
    });

    let headings = canvas.outline_headings();
    let outline_bar = (headings.len() >= 2).then(|| {
        let search_handle = search.cloned();
        h_flex()
            .flex_wrap()
            .items_center()
            .gap_1p5()
            .px_2p5()
            .py_1p5()
            .rounded_md()
            .bg(theme.secondary.opacity(0.35))
            .border_1()
            .border_color(theme.border)
            .text_xs()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child("Outline:"),
            )
            .children(headings.into_iter().enumerate().map(|(ix, (level, text))| {
                let label = text.to_owned();
                let search_h = search_handle.clone();
                let fg = theme.foreground;
                let hover = theme.accent;
                let muted = theme.muted_foreground;
                let prefix = if level >= 3 { "· " } else { "" };
                div()
                    .id(("canvas-outline", ix))
                    .px_1p5()
                    .py_0p5()
                    .rounded_sm()
                    .text_color(if level == 1 { fg } else { muted })
                    .when(level == 1, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .when_some(search_h, |d, input| {
                        let jump = label.clone();
                        d.cursor_pointer()
                            .hover(move |el| el.bg(hover).text_color(fg))
                            .on_click(move |_, window, cx| {
                                input.update(cx, |state, cx| {
                                    state.set_value(jump.clone(), window, cx);
                                });
                            })
                    })
                    .child(format!("{prefix}{label}"))
            }))
    });

    let document = if filtered.is_empty() {
        v_flex().py_8().items_center().child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(if needle.is_empty() {
                    "This canvas is empty."
                } else {
                    "No matching sections in this canvas."
                }),
        )
    } else {
        filtered.into_iter().fold(v_flex().gap_3(), |doc, block| {
            doc.child(render_block(block, cx))
        })
    };

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
                .gap_4()
                .child(header)
                .children(find_bar)
                .children(outline_bar)
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
        .child("Read-only")
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
            heading.child(text.clone())
        }
        CanvasBlock::Paragraph(text) => div().text_sm().line_height(px(22.)).child(text.clone()),
        CanvasBlock::CodeBlock(code) => div()
            .w_full()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary.opacity(0.45))
            .font_family(theme.mono_font_family.clone())
            .text_xs()
            .child(code.clone()),
        CanvasBlock::Quote(text) => div()
            .pl_3()
            .py_0p5()
            .border_l_2()
            .border_color(theme.link)
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(text.clone()),
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
                .child(div().flex_1().child(item.clone()))
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
                                .child(item.clone()),
                        )
                }))
        }
        CanvasBlock::Table { header, rows } => {
            let cell = |text: &str| {
                div()
                    .flex_1()
                    .min_w_0()
                    .px_3()
                    .py_2()
                    .truncate()
                    .child(text.to_owned())
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
        CanvasBlock::Image { url, alt } => v_flex()
            .gap_1()
            .child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .overflow_hidden()
                    .child(img(url.clone()).max_w(px(640.)).max_h(px(360.))),
            )
            .when(!alt.is_empty(), |col| {
                col.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(alt.clone()),
                )
            }),
        CanvasBlock::Divider => div().h(px(1.)).w_full().bg(theme.border),
    }
}

/// Lightweight parser converting Slack's Quip/Canvas HTML export into structured [`CanvasBlock`]s.
pub fn parse_canvas_html(html: &str) -> Vec<CanvasBlock> {
    let body = extract_tag_inner(html, "body").unwrap_or(html);
    let mut blocks = Vec::new();
    let mut rest = body;
    while let Some(open_pos) = rest.find('<') {
        let before = clean_text(&rest[..open_pos]);
        if !before.is_empty() {
            blocks.push(CanvasBlock::Paragraph(before));
        }
        rest = &rest[open_pos..];
        let Some(gt) = rest.find('>') else {
            break;
        };
        let raw_tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        let trimmed = raw_tag.trim();
        if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.starts_with('!') {
            continue;
        }
        let tag_name = trimmed
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if tag_name == "hr" {
            blocks.push(CanvasBlock::Divider);
            continue;
        }
        if tag_name == "img" {
            if let Some(src) = extract_attr(trimmed, "src") {
                if src.starts_with("https://") || src.starts_with("http://") {
                    let alt = extract_attr(trimmed, "alt").unwrap_or_default();
                    blocks.push(CanvasBlock::Image {
                        url: decode_entities(&src),
                        alt: clean_text(&alt),
                    });
                }
            }
            continue;
        }
        if matches!(tag_name.as_str(), "style" | "script" | "head") {
            let close = format!("</{tag_name}>");
            if let Some(end) = find_case_insensitive(rest, &close) {
                rest = &rest[end + close.len()..];
            }
            continue;
        }
        if matches!(
            tag_name.as_str(),
            "h1" | "h2" | "h3" | "h4" | "p" | "pre" | "blockquote" | "ul" | "ol" | "table"
        ) {
            let close = format!("</{tag_name}>");
            let Some(end) = find_case_insensitive(rest, &close) else {
                continue;
            };
            let inner = &rest[..end];
            rest = &rest[end + close.len()..];
            match tag_name.as_str() {
                "h1" | "h2" | "h3" | "h4" => {
                    let text = clean_text(&strip_tags(inner));
                    if !text.is_empty() {
                        let level = match tag_name.as_str() {
                            "h1" => 1,
                            "h2" => 2,
                            _ => 3,
                        };
                        blocks.push(CanvasBlock::Heading { level, text });
                    }
                }
                "p" => {
                    let text = clean_text(&strip_tags(inner));
                    if !text.is_empty() {
                        blocks.push(CanvasBlock::Paragraph(text));
                    }
                }
                "pre" => {
                    let code = decode_entities(&strip_tags(inner)).trim().to_owned();
                    if !code.is_empty() {
                        blocks.push(CanvasBlock::CodeBlock(code));
                    }
                }
                "blockquote" => {
                    let text = clean_text(&strip_tags(inner));
                    if !text.is_empty() {
                        blocks.push(CanvasBlock::Quote(text));
                    }
                }
                "ul" | "ol" => {
                    let is_checklist = trimmed.contains("checklist")
                        || inner.contains("data-checked")
                        || inner.contains("type=\"checkbox\"");
                    let items = extract_list_items(inner);
                    if !items.is_empty() {
                        if is_checklist {
                            blocks.push(CanvasBlock::Checklist(items));
                        } else {
                            blocks.push(CanvasBlock::Bullets(
                                items.into_iter().map(|(_, t)| t).collect(),
                            ));
                        }
                    }
                }
                "table" => {
                    if let Some((header, rows)) = parse_html_table(inner) {
                        blocks.push(CanvasBlock::Table { header, rows });
                    }
                }
                _ => {}
            }
        }
    }
    let tail = clean_text(&strip_tags(rest));
    if !tail.is_empty() {
        blocks.push(CanvasBlock::Paragraph(tail));
    }
    blocks
}

fn find_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .to_ascii_lowercase()
        .find(&needle.to_ascii_lowercase())
}

fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let needle = format!("{attr}=");
    let pos = lower.find(&needle)?;
    let rest = tag[pos + needle.len()..].trim_start();
    if let Some(quoted) = rest.strip_prefix('"') {
        let end = quoted.find('"')?;
        Some(quoted[..end].to_owned())
    } else if let Some(quoted) = rest.strip_prefix('\'') {
        let end = quoted.find('\'')?;
        Some(quoted[..end].to_owned())
    } else {
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .unwrap_or(rest.len());
        Some(rest[..end].to_owned())
    }
}

fn extract_tag_inner<'a>(html: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let start = find_case_insensitive(html, &open)?;
    let after_open = &html[start..];
    let gt = after_open.find('>')?;
    let content = &after_open[gt + 1..];
    let close = format!("</{tag}>");
    let end = find_case_insensitive(content, &close)?;
    Some(&content[..end])
}

fn extract_list_items(inner: &str) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    let mut rest = inner;
    while let Some(pos) = find_case_insensitive(rest, "<li") {
        rest = &rest[pos..];
        let Some(gt) = rest.find('>') else { break };
        let open_tag = &rest[..gt];
        rest = &rest[gt + 1..];
        let end = find_case_insensitive(rest, "</li>").unwrap_or(rest.len());
        let body = &rest[..end];
        let checked = open_tag.contains("data-checked=\"true\"")
            || open_tag.contains("class=\"checked\"")
            || body.contains("checked");
        let text = clean_text(&strip_tags(body));
        if !text.is_empty() {
            out.push((checked, text));
        }
        rest = if end + 5 <= rest.len() {
            &rest[end + 5..]
        } else {
            ""
        };
    }
    out
}

fn parse_html_table(inner: &str) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut rest = inner;
    while let Some(pos) = find_case_insensitive(rest, "<tr") {
        rest = &rest[pos..];
        let Some(gt) = rest.find('>') else { break };
        rest = &rest[gt + 1..];
        let end = find_case_insensitive(rest, "</tr>").unwrap_or(rest.len());
        let tr = &rest[..end];
        let mut cells = Vec::new();
        let mut cell_rest = tr;
        while let Some(cpos) = cell_rest.find('<') {
            cell_rest = &cell_rest[cpos..];
            let Some(cgt) = cell_rest.find('>') else {
                break;
            };
            let tag = cell_rest[1..cgt]
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            cell_rest = &cell_rest[cgt + 1..];
            if tag == "th" || tag == "td" {
                let close = format!("</{tag}>");
                let cend = find_case_insensitive(cell_rest, &close).unwrap_or(cell_rest.len());
                cells.push(clean_text(&strip_tags(&cell_rest[..cend])));
                cell_rest = if cend + close.len() <= cell_rest.len() {
                    &cell_rest[cend + close.len()..]
                } else {
                    ""
                };
            }
        }
        if !cells.is_empty() {
            rows.push(cells);
        }
        rest = if end + 5 <= rest.len() {
            &rest[end + 5..]
        } else {
            ""
        };
    }
    if rows.is_empty() {
        return None;
    }
    let header = rows.remove(0);
    Some((header, rows))
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
}

fn clean_text(s: &str) -> String {
    decode_entities(s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canvas_html_blocks_and_filters_by_query() {
        let html = r#"<html><head><style>body{color:red}</style></head><body>
            <h1>Architecture &amp; Plan</h1>
            <p>Overview of the <b>native</b> reader.</p>
            <ul class="checklist">
                <li data-checked="true">Fetch HTML</li>
                <li data-checked="false">Add find bar</li>
            </ul>
            <pre><code>cargo test</code></pre>
            <blockquote>Keep it fast.</blockquote>
            <hr/>
            <table><tr><th>Owner</th><th>Task</th></tr><tr><td>Ana</td><td>Ship</td></tr></table>
        </body></html>"#;
        let blocks = parse_canvas_html(html);
        assert_eq!(
            blocks,
            vec![
                CanvasBlock::Heading {
                    level: 1,
                    text: "Architecture & Plan".into(),
                },
                CanvasBlock::Paragraph("Overview of the native reader.".into()),
                CanvasBlock::Checklist(vec![
                    (true, "Fetch HTML".into()),
                    (false, "Add find bar".into()),
                ]),
                CanvasBlock::CodeBlock("cargo test".into()),
                CanvasBlock::Quote("Keep it fast.".into()),
                CanvasBlock::Divider,
                CanvasBlock::Table {
                    header: vec!["Owner".into(), "Task".into()],
                    rows: vec![vec!["Ana".into(), "Ship".into()]],
                },
            ]
        );
        assert!(blocks[0].matches("architecture"));
        assert!(!blocks[0].matches("cargo"));
        assert!(blocks[3].matches("cargo"));
    }

    #[test]
    fn parses_canvas_images_and_extracts_heading_outline() {
        let html = r#"<html><body>
            <h1>Launch Plan</h1>
            <img src="https://files.slack.com/files-pri/T1-F1/arch.png" alt="Architecture diagram" />
            <h2>Milestones</h2>
            <h3>Risks</h3>
        </body></html>"#;
        let doc = CanvasDoc::from_html("Launch Plan".into(), html);
        assert!(doc.blocks.iter().any(|b| matches!(
            b,
            CanvasBlock::Image { url, alt }
                if url == "https://files.slack.com/files-pri/T1-F1/arch.png"
                && alt == "Architecture diagram"
        )));
        assert_eq!(
            doc.outline_headings(),
            vec![(1, "Launch Plan"), (2, "Milestones"), (3, "Risks")]
        );
    }
}
