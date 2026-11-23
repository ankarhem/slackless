//! Native Slack Canvas reader & section editor (`F-COLLAB-125`, `F-COLLAB-128`, `F-COLLAB-129`,
//! `F-COLLAB-132`, `F-SHELL-261`, `F-SHELL-264`, `F-SHELL-266`, `F-SHELL-268`, `F-SHELL-277`):
//! renders both the sample workspace's fixture canvas and live Slack canvases fetched via
//! `files.info` -> `url_private` HTML export (keeping `temp:C:...` section IDs), with an
//! in-canvas Find bar, collapsible Outline, interactive checklist toggling, markdown section
//! editor with formatting toolbar and undo/redo, Request edit access, Pop-out window, and
//! "Open in browser".

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, Size, h_flex, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Div, Entity, FontWeight, Global, InteractiveElement as _, ObjectFit,
    ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Window, div, img, px,
};
use slackless_core::api::CanvasEditOp;

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
    /// Embedded canvas image (`img[src]` with optional alt caption and reserved dimensions).
    Image {
        url: String,
        alt: String,
        width: Option<u32>,
        height: Option<u32>,
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
            Self::Image { alt, url, .. } => has(alt) || has(url),
            Self::Divider => false,
        }
    }
    /// Serialize this block to Slack canvas markdown (`canvases.edit`).
    pub fn to_markdown(&self) -> String {
        match self {
            Self::Heading { level, text } => {
                let hashes = "#".repeat((*level).clamp(1, 3) as usize);
                format!("{hashes} {text}")
            }
            Self::Paragraph(text) => text.clone(),
            Self::Bullets(items) => items
                .iter()
                .map(|item| format!("- {item}"))
                .collect::<Vec<_>>()
                .join("\n"),
            Self::Checklist(items) => items
                .iter()
                .map(|(done, item)| {
                    if *done {
                        format!("- [x] {item}")
                    } else {
                        format!("- [ ] {item}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Self::CodeBlock(code) => format!("```\n{code}\n```"),
            Self::Quote(text) => format!("> {text}"),
            Self::Table { header, rows } => {
                let mut lines = Vec::new();
                lines.push(format!("| {} |", header.join(" | ")));
                lines.push(format!(
                    "| {} |",
                    header.iter().map(|_| "---").collect::<Vec<_>>().join(" | ")
                ));
                for row in rows {
                    lines.push(format!("| {} |", row.join(" | ")));
                }
                lines.join("\n")
            }
            Self::Image { url, alt, .. } => format!("![{alt}]({url})"),
            Self::Divider => "---".to_owned(),
        }
    }
}

/// A parsed canvas document ready to render in the native reader/editor.
#[derive(Clone, Debug)]
pub struct CanvasDoc {
    pub title: String,
    pub meta: String,
    pub blocks: Vec<CanvasBlock>,
    /// Optional Quip section IDs (`temp:C:...`) aligned 1:1 with `blocks` when parsed from HTML.
    pub section_ids: Vec<Option<String>>,
}

impl CanvasDoc {
    /// Parse a live Slack canvas HTML export (`url_private`) into structured blocks + section IDs.
    pub fn from_html(title: String, html: &str) -> Self {
        let (blocks, section_ids) = parse_canvas_html_with_ids(html);
        Self {
            title,
            meta: "Live canvas".to_owned(),
            blocks,
            section_ids,
        }
    }

    /// Parse lightweight markdown back into [`CanvasBlock`]s (for local preview & fixture edits).
    pub fn from_markdown(title: String, meta: String, markdown: &str) -> Self {
        let blocks = parse_canvas_markdown(markdown);
        let section_ids = vec![None; blocks.len()];
        Self {
            title,
            meta,
            blocks,
            section_ids,
        }
    }

    /// Serialize the entire canvas back into Slack canvas markdown (`canvases.edit`).
    pub fn to_markdown(&self) -> String {
        self.blocks
            .iter()
            .map(CanvasBlock::to_markdown)
            .collect::<Vec<_>>()
            .join("\n\n")
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
    let blocks = vec![
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
    ];
    let section_ids = vec![None; blocks.len()];
    CanvasDoc {
        title: "Q4 launch plan".into(),
        meta: "Canvas · last edited by Maya Chen".into(),
        blocks,
        section_ids,
    }
}

/// Callback invoked when the user clicks "Open in browser" to edit a live canvas.
pub type OpenBrowserFn = Rc<dyn Fn(&mut Window, &mut App)>;
/// Callback invoked when the user edits a live canvas (`canvases.edit`).
pub type CanvasEditFn = Rc<dyn Fn(CanvasEditOp, &mut Window, &mut App)>;
/// Callback invoked when the user pops a canvas out into its own window (`F-COLLAB-128`).
pub type CanvasPopOutFn = Rc<dyn Fn(CanvasDoc, &mut Window, &mut App)>;

/// Optional interactive callbacks wired by the host window (`app/mod.rs` or `popout.rs`).
#[derive(Clone, Default)]
pub struct CanvasActions {
    pub open_browser: Option<OpenBrowserFn>,
    pub on_edit: Option<CanvasEditFn>,
    pub on_request_access: Option<OpenBrowserFn>,
    pub on_pop_out: Option<CanvasPopOutFn>,
    /// Pin this canvas as a conversation tab (`conversations.addTab`, `F-COLLAB-121`).
    pub on_pin_tab: Option<OpenBrowserFn>,
    /// Canonical canvas URL used when copying heading anchor links (`F-SHELL-267`).
    pub canvas_url: Option<String>,
}

#[derive(Clone, Default)]
struct CanvasUiState {
    editing: bool,
    editor: Option<Entity<TextareaState>>,
    undo_stack: Vec<String>,
    redo_stack: Vec<String>,
    /// Local copy for fixture mode or optimistic live edits.
    local_doc: Option<CanvasDoc>,
    access_requested: bool,
    /// Distraction-free Reader View (`F-COLLAB-198`): hides find bar & outline chrome.
    reader_mode: bool,
    /// Docked Canvas Comments drawer (`F-COLLAB-197`, `F-COLLAB-199`).
    comments_open: bool,
    comments: Vec<(String, String)>,
    comment_input: Option<Entity<TextareaState>>,
    pinned_tab: bool,
}

#[derive(Default)]
struct CanvasUiStore {
    panels: HashMap<String, CanvasUiState>,
}

impl Global for CanvasUiStore {}

fn ui_state(key: &str, cx: &App) -> CanvasUiState {
    cx.try_global::<CanvasUiStore>()
        .and_then(|s| s.panels.get(key))
        .cloned()
        .unwrap_or_default()
}

fn mutate_ui(key: &str, f: impl FnOnce(&mut CanvasUiState), window: &mut Window, cx: &mut App) {
    let entry = cx
        .default_global::<CanvasUiStore>()
        .panels
        .entry(key.to_owned())
        .or_default();
    f(entry);
    window.refresh();
}

/// The Canvas tab body in fixture mode: scrollable, centered, editable document.
pub fn canvas_view(
    query: &str,
    search: Option<&Entity<TextareaState>>,
    actions: CanvasActions,
    cx: &App,
) -> Stateful<Div> {
    const KEY: &str = "fixture";
    let state = ui_state(KEY, cx);
    let canvas = state.local_doc.unwrap_or_else(fixture);
    render_canvas_doc(KEY, &canvas, query, search, actions, true, cx)
}

/// Native live Canvas reader & editor (`F-COLLAB-125`, `F-COLLAB-132`, `F-SHELL-264`, `F-SHELL-266`):
/// shows loading, error state with browser fallback, or the parsed live canvas with Find, Outline,
/// interactive checklists, markdown section editor with formatting toolbar & undo/redo, Request edit
/// access, Pop-out window, and "Open in browser".
pub fn live_canvas_view(
    file_id: &str,
    loaded: Option<&Result<CanvasDoc, String>>,
    query: &str,
    search: Option<&Entity<TextareaState>>,
    actions: CanvasActions,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let open_browser = actions.open_browser.clone();
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
        Some(Ok(doc)) => render_canvas_doc(file_id, doc, query, search, actions, false, cx),
    }
}

/// Standalone render helper for a popped-out Canvas window (`F-COLLAB-128`, `F-SHELL-148`).
pub fn render_popout_canvas(
    key: &str,
    doc: &CanvasDoc,
    query: &str,
    search: Option<&Entity<TextareaState>>,
    actions: CanvasActions,
    is_fixture: bool,
    cx: &App,
) -> Stateful<Div> {
    let state = ui_state(key, cx);
    let active = state.local_doc.as_ref().unwrap_or(doc);
    render_canvas_doc(key, active, query, search, actions, is_fixture, cx)
}

fn render_canvas_doc(
    key: &str,
    canvas: &CanvasDoc,
    query: &str,
    search: Option<&Entity<TextareaState>>,
    actions: CanvasActions,
    is_fixture: bool,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let ui = ui_state(key, cx);
    let can_edit = is_fixture || actions.on_edit.is_some();
    let needle = query.trim().to_lowercase();
    let filtered: Vec<(usize, &CanvasBlock)> = canvas
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| b.matches(&needle))
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

    let key_edit = key.to_owned();
    let canvas_for_edit = canvas.clone();
    let edit_toggle = can_edit.then(|| {
        Button::new(SharedString::from(format!("canvas-edit-toggle-{key}")))
            .outline()
            .with_size(Size::XSmall)
            .icon(if ui.editing {
                IconName::Eye
            } else {
                IconName::Pencil
            })
            .label(if ui.editing { "Preview" } else { "Edit" })
            .tooltip("Toggle canvas reader / markdown editor")
            .on_click(move |_, window, cx| {
                let md = canvas_for_edit.to_markdown();
                let should_open = !ui_state(&key_edit, cx).editing;
                let editor = if should_open {
                    Some(cx.new(|cx| {
                        TextareaState::new(window, cx)
                            .auto_grow(10, 24)
                            .default_value(md.clone())
                            .placeholder("Write canvas markdown…")
                    }))
                } else {
                    None
                };
                mutate_ui(
                    &key_edit,
                    |s| {
                        s.editing = should_open;
                        if let Some(ed) = editor {
                            s.undo_stack = vec![md];
                            s.redo_stack.clear();
                            s.editor = Some(ed);
                        }
                    },
                    window,
                    cx,
                );
            })
    });

    let key_req = key.to_owned();
    let req_btn = actions.on_request_access.clone().map(|req| {
        let already = ui.access_requested;
        Button::new(SharedString::from(format!("canvas-request-access-{key}")))
            .outline()
            .with_size(Size::XSmall)
            .icon(IconName::Lock)
            .label(if already {
                "Access requested"
            } else {
                "Request edit"
            })
            .disabled(already)
            .on_click(move |_, window, cx| {
                mutate_ui(&key_req, |s| s.access_requested = true, window, cx);
                req(window, cx);
            })
    });

    let popout_btn = actions.on_pop_out.clone().map(|pop| {
        let doc = canvas.clone();
        Button::new(SharedString::from(format!("canvas-popout-{key}")))
            .outline()
            .with_size(Size::XSmall)
            .icon(IconName::ExternalLink)
            .label("Pop out")
            .tooltip("Open canvas in a new window")
            .on_click(move |_, window, cx| pop(doc.clone(), window, cx))
    });

    let key_reader = key.to_owned();
    let reader_btn = Button::new(SharedString::from(format!("canvas-reader-{key}")))
        .when(ui.reader_mode, |b| b.primary())
        .when(!ui.reader_mode, |b| b.outline())
        .with_size(Size::XSmall)
        .icon(IconName::Eye)
        .label(if ui.reader_mode {
            "Exit reader"
        } else {
            "Reader"
        })
        .tooltip("Toggle distraction-free Reader View")
        .on_click(move |_, window, cx| {
            mutate_ui(&key_reader, |s| s.reader_mode = !s.reader_mode, window, cx);
        });

    let key_comments = key.to_owned();
    let comment_count = ui.comments.len();
    let comments_label = if comment_count == 0 {
        "Comments".to_owned()
    } else {
        format!("Comments ({comment_count})")
    };
    let comments_btn = Button::new(SharedString::from(format!("canvas-comments-{key}")))
        .when(ui.comments_open, |b| b.primary())
        .when(!ui.comments_open, |b| b.outline())
        .with_size(Size::XSmall)
        .icon(IconName::MessageSquare)
        .label(comments_label)
        .tooltip("Toggle canvas comments drawer")
        .on_click(move |_, window, cx| {
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Add a comment on this canvas…")
                    .auto_grow(1, 4)
            });
            mutate_ui(
                &key_comments,
                |s| {
                    s.comments_open = !s.comments_open;
                    if s.comments_open && s.comment_input.is_none() {
                        s.comment_input = Some(input);
                    }
                },
                window,
                cx,
            );
        });

    let key_pin = key.to_owned();
    let pin_tab_btn = actions.on_pin_tab.clone().map(|pin_cb| {
        let already = ui.pinned_tab;
        Button::new(SharedString::from(format!("canvas-pin-tab-{key}")))
            .outline()
            .with_size(Size::XSmall)
            .icon(IconName::Pin)
            .label(if already { "Pinned tab" } else { "Pin tab" })
            .disabled(already)
            .tooltip("Pin this canvas as a channel tab")
            .on_click(move |_, window, cx| {
                mutate_ui(&key_pin, |s| s.pinned_tab = true, window, cx);
                pin_cb(window, cx);
            })
    });

    let header = h_flex()
        .items_center()
        .flex_wrap()
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
        .when(!can_edit, |row| row.child(read_only_badge(cx)))
        .child(reader_btn)
        .child(comments_btn)
        .children(pin_tab_btn)
        .children(edit_toggle)
        .children(req_btn)
        .children(popout_btn)
        .when_some(actions.open_browser.clone(), |row, open| {
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

    let document = if ui.editing {
        if let Some(editor) = ui.editor.clone() {
            render_canvas_editor(
                key,
                canvas,
                &ui,
                editor,
                actions.on_edit.clone(),
                is_fixture,
                cx,
            )
        } else {
            v_flex()
        }
    } else if filtered.is_empty() {
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
        filtered
            .into_iter()
            .fold(v_flex().gap_3(), |doc_col, (block_ix, block)| {
                let on_toggle = can_edit.then(|| {
                    let key_t = key.to_owned();
                    let canvas_t = canvas.clone();
                    let on_edit_t = actions.on_edit.clone();
                    Rc::new(move |item_ix: usize, window: &mut Window, cx: &mut App| {
                        let mut updated = canvas_t.clone();
                        let section_id = updated.section_ids.get(block_ix).and_then(|s| s.clone());
                        if let Some(CanvasBlock::Checklist(items)) =
                            updated.blocks.get_mut(block_ix)
                        {
                            if let Some((done, _)) = items.get_mut(item_ix) {
                                *done = !*done;
                            }
                            let section_md = CanvasBlock::Checklist(items.clone()).to_markdown();
                            let full_md = updated.to_markdown();
                            if is_fixture {
                                mutate_ui(&key_t, |s| s.local_doc = Some(updated), window, cx);
                            }
                            if let Some(cb) = &on_edit_t {
                                let op = if let Some(sid) = section_id {
                                    CanvasEditOp::Replace {
                                        section_id: Some(sid),
                                        markdown: section_md,
                                    }
                                } else {
                                    CanvasEditOp::Replace {
                                        section_id: None,
                                        markdown: full_md,
                                    }
                                };
                                cb(op, window, cx);
                            }
                        }
                    }) as Rc<dyn Fn(usize, &mut Window, &mut App)>
                });
                let section_id = canvas.section_ids.get(block_ix).and_then(|s| s.as_deref());
                doc_col.child(render_block(
                    block_ix,
                    block,
                    section_id,
                    actions.canvas_url.as_deref(),
                    on_toggle,
                    cx,
                ))
            })
    };

    let main_scroll = div()
        .id("canvas-scroll")
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_y_scroll()
        .child(
            v_flex()
                .w_full()
                .max_w(px(if ui.reader_mode { 680. } else { 760. }))
                .mx_auto()
                .px_8()
                .py_6()
                .gap_4()
                .child(header)
                .when(!ui.reader_mode, |col| {
                    col.children(find_bar).children(outline_bar)
                })
                .child(document),
        );

    if ui.comments_open {
        let drawer = render_comments_drawer(key, &ui, cx);
        h_flex()
            .id("canvas-root-with-comments")
            .size_full()
            .child(main_scroll)
            .child(drawer)
    } else {
        main_scroll
    }
}

#[inline(never)]
fn render_canvas_editor(
    key: &str,
    canvas: &CanvasDoc,
    ui: &CanvasUiState,
    editor: Entity<TextareaState>,
    on_edit: Option<CanvasEditFn>,
    is_fixture: bool,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let insert_snippet = |label: &'static str, snippet: &'static str| {
        let ed = editor.clone();
        let k = key.to_owned();
        Button::new(SharedString::from(format!("canvas-fmt-{k}-{label}")))
            .outline()
            .with_size(Size::XSmall)
            .label(label)
            .on_click(move |_, window, cx| {
                let current = ed.read(cx).value().to_string();
                let next = if current.trim().is_empty() {
                    snippet.to_owned()
                } else {
                    format!("{current}\n\n{snippet}")
                };
                push_undo_and_set(&k, &ed, current, next, window, cx);
            })
    };

    let can_undo = ui.undo_stack.len() > 1;
    let can_redo = !ui.redo_stack.is_empty();
    let ed_undo = editor.clone();
    let k_undo = key.to_owned();
    let ed_redo = editor.clone();
    let k_redo = key.to_owned();
    let ed_save = editor.clone();
    let k_save = key.to_owned();
    let k_cancel = key.to_owned();
    let title_save = canvas.title.clone();
    let meta_save = canvas.meta.clone();
    let ed_cycle = editor.clone();
    let k_cycle = key.to_owned();
    let ed_up = editor.clone();
    let k_up = key.to_owned();
    let ed_down = editor.clone();
    let k_down = key.to_owned();

    let snippets = [
        ("H1", "# Heading"),
        ("H2", "## Section"),
        ("H3", "### Subheading"),
        ("Bullet", "- List item"),
        ("Checklist", "- [ ] Task item"),
        ("Quote", "> Quoted note"),
        ("Code", "```\ncode\n```"),
        ("Divider", "---"),
    ];
    let mut toolbar = h_flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary.opacity(0.35));
    for (label, snippet) in snippets {
        toolbar = toolbar.child(insert_snippet(label, snippet));
    }
    let toolbar = toolbar
        .child(
            Button::new(SharedString::from(format!("canvas-cycle-h-{key}")))
                .outline()
                .with_size(Size::XSmall)
                .label("Cycle H")
                .tooltip("Cycle active line heading level (plain / H1 / H2 / H3)")
                .on_click(move |_, window, cx| {
                    let current = ed_cycle.read(cx).value().to_string();
                    let next = cycle_last_line_heading(&current);
                    push_undo_and_set(&k_cycle, &ed_cycle, current, next, window, cx);
                }),
        )
        .child(
            Button::new(SharedString::from(format!("canvas-line-up-{key}")))
                .ghost()
                .with_size(Size::XSmall)
                .icon(IconName::ArrowUp)
                .tooltip("Move active line up")
                .on_click(move |_, window, cx| {
                    let current = ed_up.read(cx).value().to_string();
                    let next = move_last_line(&current, -1);
                    push_undo_and_set(&k_up, &ed_up, current, next, window, cx);
                }),
        )
        .child(
            Button::new(SharedString::from(format!("canvas-line-down-{key}")))
                .ghost()
                .with_size(Size::XSmall)
                .icon(IconName::ArrowDown)
                .tooltip("Move active line down")
                .on_click(move |_, window, cx| {
                    let current = ed_down.read(cx).value().to_string();
                    let next = move_last_line(&current, 1);
                    push_undo_and_set(&k_down, &ed_down, current, next, window, cx);
                }),
        )
        .child(div().flex_1())
        .child(
            Button::new(SharedString::from(format!("canvas-undo-{key}")))
                .ghost()
                .with_size(Size::XSmall)
                .icon(IconName::Undo)
                .tooltip("Undo canvas edit")
                .disabled(!can_undo)
                .on_click(move |_, window, cx| {
                    let current = ed_undo.read(cx).value().to_string();
                    let mut target = None;
                    mutate_ui(
                        &k_undo,
                        |s| {
                            if s.undo_stack.len() > 1 {
                                s.redo_stack.push(current);
                                let _ = s.undo_stack.pop();
                                target = s.undo_stack.last().cloned();
                            }
                        },
                        window,
                        cx,
                    );
                    if let Some(prev) = target {
                        ed_undo.update(cx, |st, cx| st.set_value(prev, window, cx));
                    }
                }),
        )
        .child(
            Button::new(SharedString::from(format!("canvas-redo-{key}")))
                .ghost()
                .with_size(Size::XSmall)
                .icon(IconName::Redo)
                .tooltip("Redo canvas edit")
                .disabled(!can_redo)
                .on_click(move |_, window, cx| {
                    let mut target = None;
                    mutate_ui(
                        &k_redo,
                        |s| {
                            if let Some(next) = s.redo_stack.pop() {
                                s.undo_stack.push(next.clone());
                                target = Some(next);
                            }
                        },
                        window,
                        cx,
                    );
                    if let Some(next) = target {
                        ed_redo.update(cx, |st, cx| st.set_value(next, window, cx));
                    }
                }),
        );

    let footer = h_flex()
        .justify_end()
        .gap_2()
        .child(
            Button::new(SharedString::from(format!("canvas-cancel-{key}")))
                .outline()
                .with_size(Size::Small)
                .label("Cancel")
                .on_click(move |_, window, cx| {
                    mutate_ui(&k_cancel, |s| s.editing = false, window, cx);
                }),
        )
        .child(
            Button::new(SharedString::from(format!("canvas-save-{key}")))
                .primary()
                .with_size(Size::Small)
                .icon(IconName::Check)
                .label("Save changes")
                .on_click(move |_, window, cx| {
                    let markdown = ed_save.read(cx).value().to_string();
                    let updated =
                        CanvasDoc::from_markdown(title_save.clone(), meta_save.clone(), &markdown);
                    mutate_ui(
                        &k_save,
                        |s| {
                            s.editing = false;
                            if is_fixture {
                                s.local_doc = Some(updated);
                            }
                        },
                        window,
                        cx,
                    );
                    if let Some(cb) = &on_edit {
                        cb(
                            CanvasEditOp::Replace {
                                section_id: None,
                                markdown,
                            },
                            window,
                            cx,
                        );
                    }
                }),
        );

    v_flex()
        .gap_3()
        .child(toolbar)
        .child(
            div()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .p_2()
                .child(Textarea::new(&editor).appearance(false).bordered(false)),
        )
        .child(footer)
}

fn push_undo_and_set(
    key: &str,
    editor: &Entity<TextareaState>,
    current: String,
    next: String,
    window: &mut Window,
    cx: &mut App,
) {
    if current == next {
        return;
    }
    mutate_ui(
        key,
        |s| {
            if s.undo_stack.last().map(String::as_str) != Some(current.as_str()) {
                s.undo_stack.push(current);
            }
            s.redo_stack.clear();
        },
        window,
        cx,
    );
    editor.update(cx, |state, cx| state.set_value(next, window, cx));
}

/// Cycle the last non-empty line through plain -> `# ` -> `## ` -> `### ` -> plain (`F-SHELL-262`).
pub(crate) fn cycle_last_line_heading(markdown: &str) -> String {
    let mut lines: Vec<String> = markdown.lines().map(str::to_owned).collect();
    let Some(ix) = lines.iter().rposition(|l| !l.trim().is_empty()) else {
        return "# Heading".to_owned();
    };
    let line = lines[ix].trim_start().to_owned();
    lines[ix] = if let Some(rest) = line.strip_prefix("### ") {
        rest.to_owned()
    } else if let Some(rest) = line.strip_prefix("## ") {
        format!("### {rest}")
    } else if let Some(rest) = line.strip_prefix("# ") {
        format!("## {rest}")
    } else {
        format!("# {line}")
    };
    lines.join("\n")
}

/// Swap the last non-empty line with its neighbor (`delta = -1` moves up, `1` moves down, `F-SHELL-269`).
pub(crate) fn move_last_line(markdown: &str, delta: isize) -> String {
    let mut lines: Vec<String> = markdown.lines().map(str::to_owned).collect();
    let Some(ix) = lines.iter().rposition(|l| !l.trim().is_empty()) else {
        return markdown.to_owned();
    };
    let target = ix as isize + delta;
    if target < 0 || target as usize >= lines.len() {
        return markdown.to_owned();
    }
    lines.swap(ix, target as usize);
    lines.join("\n")
}

fn render_comments_drawer(key: &str, ui: &CanvasUiState, cx: &App) -> Div {
    let theme = cx.theme();
    let key_close = key.to_owned();
    let key_submit = key.to_owned();
    let input_opt = ui.comment_input.clone();

    let mut body = v_flex().flex_1().min_h_0().gap_2();
    if ui.comments.is_empty() {
        body = body.child(
            div()
                .py_4()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("No comments on this canvas yet. Add a note below to discuss sections with teammates."),
        );
    } else {
        for (author, text) in &ui.comments {
            body = body.child(
                v_flex()
                    .gap_0p5()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.secondary.opacity(0.3))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(author.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(text.clone()),
                    ),
            );
        }
    }

    let composer = input_opt.map(|input| {
        let input_send = input.clone();
        v_flex()
            .gap_1p5()
            .pt_2()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .px_2()
                    .py_1()
                    .child(Textarea::new(&input).appearance(false).bordered(false)),
            )
            .child(
                h_flex().justify_end().child(
                    Button::new(SharedString::from(format!("canvas-comment-post-{key}")))
                        .primary()
                        .with_size(Size::XSmall)
                        .label("Comment")
                        .on_click(move |_, window, cx| {
                            let body_text = input_send.read(cx).value().trim().to_owned();
                            if body_text.is_empty() {
                                return;
                            }
                            input_send.update(cx, |st, cx| st.set_value(String::new(), window, cx));
                            mutate_ui(
                                &key_submit,
                                |s| s.comments.push(("You".to_owned(), body_text)),
                                window,
                                cx,
                            );
                        }),
                ),
            )
    });

    v_flex()
        .w(px(300.))
        .flex_none()
        .h_full()
        .border_l_1()
        .border_color(theme.border)
        .bg(theme.background)
        .p_3()
        .gap_3()
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .pb_2()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            Icon::new(IconName::MessageSquare)
                                .size(px(14.))
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Comments"),
                        ),
                )
                .child(
                    Button::new(SharedString::from(format!("canvas-comments-close-{key}")))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::X)
                        .on_click(move |_, window, cx| {
                            mutate_ui(&key_close, |s| s.comments_open = false, window, cx);
                        }),
                ),
        )
        .child(body)
        .children(composer)
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

type ToggleCheckFn = Rc<dyn Fn(usize, &mut Window, &mut App)>;

fn render_block(
    block_ix: usize,
    block: &CanvasBlock,
    section_id: Option<&str>,
    canvas_url: Option<&str>,
    on_toggle_check: Option<ToggleCheckFn>,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    match block {
        CanvasBlock::Heading { level, text } => {
            let slug = section_id.map(str::to_owned).unwrap_or_else(|| {
                text.to_ascii_lowercase()
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .collect::<String>()
            });
            let anchor_url = match canvas_url {
                Some(base) if !base.is_empty() => format!("{base}#{slug}"),
                _ => format!("#{slug}"),
            };
            let heading = div()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.foreground);
            let heading = match level {
                1 => heading.text_2xl(),
                2 => heading.text_lg(),
                _ => heading.text_base(),
            };
            h_flex()
                .items_center()
                .gap_1p5()
                .when(*level == 2, |row| row.pt_2())
                .child(heading.child(text.clone()))
                .child(
                    Button::new(SharedString::from(format!("canvas-anchor-{block_ix}")))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::Link)
                        .tooltip("Copy heading anchor link")
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(
                                anchor_url.clone(),
                            ));
                            crate::actions::notify(
                                "Copied section link",
                                anchor_url.clone(),
                                window,
                                cx,
                            );
                        }),
                )
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
            let hover_bg = theme.secondary.opacity(0.4);
            v_flex()
                .gap_1p5()
                .pl_1()
                .children(items.iter().enumerate().map(|(item_ix, (done, item))| {
                    let (icon, color) = if *done {
                        (IconName::SquareCheck, theme.success)
                    } else {
                        (IconName::Square, theme.muted_foreground)
                    };
                    let toggle = on_toggle_check.clone();
                    h_flex()
                        .id(SharedString::from(format!(
                            "canvas-chk-{block_ix}-{item_ix}"
                        )))
                        .gap_2()
                        .px_1()
                        .py_0p5()
                        .rounded_sm()
                        .items_center()
                        .text_sm()
                        .when(toggle.is_some(), |row| {
                            row.cursor_pointer().hover(move |el| el.bg(hover_bg))
                        })
                        .when_some(toggle, |row, cb| {
                            row.on_click(move |_, window, cx| cb(item_ix, window, cx))
                        })
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
        CanvasBlock::Image {
            url,
            alt,
            width,
            height,
        } => {
            let (box_w, box_h) = match (*width, *height) {
                (Some(w), Some(h)) if w > 0 && h > 0 => {
                    let scale = (640. / w as f32).min(360. / h as f32).min(1.);
                    ((w as f32 * scale).max(64.), (h as f32 * scale).max(48.))
                }
                _ => (480., 270.),
            };
            v_flex()
                .gap_1()
                .child(
                    div()
                        .w(px(box_w))
                        .h(px(box_h))
                        .rounded_md()
                        .bg(theme.muted.opacity(0.3))
                        .border_1()
                        .border_color(theme.border)
                        .overflow_hidden()
                        .child(img(url.clone()).size_full().object_fit(ObjectFit::Contain)),
                )
                .when(!alt.is_empty(), |col| {
                    col.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(alt.clone()),
                    )
                })
        }
        CanvasBlock::Divider => div().h(px(1.)).w_full().bg(theme.border),
    }
}

/// Lightweight parser converting Slack's Quip/Canvas HTML export into structured [`CanvasBlock`]s.
#[cfg(test)]
pub fn parse_canvas_html(html: &str) -> Vec<CanvasBlock> {
    parse_canvas_html_with_ids(html).0
}

/// Like [`parse_canvas_html`], also returning each block's Quip `id` attribute (`temp:C:...`)
/// when present so `canvases.edit` can target individual sections.
pub fn parse_canvas_html_with_ids(html: &str) -> (Vec<CanvasBlock>, Vec<Option<String>>) {
    let body = extract_tag_inner(html, "body").unwrap_or(html);
    let mut blocks = Vec::new();
    let mut ids = Vec::new();
    let mut rest = body;
    while let Some(open_pos) = rest.find('<') {
        let before = clean_text(&rest[..open_pos]);
        if !before.is_empty() {
            blocks.push(CanvasBlock::Paragraph(before));
            ids.push(None);
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
        let section_id = extract_attr(trimmed, "id").filter(|s| !s.is_empty());
        if tag_name == "hr" {
            blocks.push(CanvasBlock::Divider);
            ids.push(section_id);
            continue;
        }
        if tag_name == "img" {
            if let Some(src) = extract_attr(trimmed, "src") {
                if src.starts_with("https://") || src.starts_with("http://") {
                    let alt = extract_attr(trimmed, "alt").unwrap_or_default();
                    let width = extract_attr(trimmed, "width")
                        .and_then(|v| v.trim_end_matches("px").parse::<u32>().ok());
                    let height = extract_attr(trimmed, "height")
                        .and_then(|v| v.trim_end_matches("px").parse::<u32>().ok());
                    blocks.push(CanvasBlock::Image {
                        url: decode_entities(&src),
                        alt: clean_text(&alt),
                        width,
                        height,
                    });
                    ids.push(section_id);
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
            let prev_len = blocks.len();
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
            if blocks.len() > prev_len {
                ids.push(section_id);
            }
        }
    }
    let tail = clean_text(&strip_tags(rest));
    if !tail.is_empty() {
        blocks.push(CanvasBlock::Paragraph(tail));
        ids.push(None);
    }
    (blocks, ids)
}

/// Lightweight Slack canvas markdown parser (headings, bullets, checklists, code fences,
/// blockquotes, horizontal rules, pipe tables, paragraphs).
pub fn parse_canvas_markdown(markdown: &str) -> Vec<CanvasBlock> {
    let mut blocks = Vec::new();
    let lines: Vec<&str> = markdown.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }
        if trimmed.starts_with("```") {
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim().starts_with("```") {
                code.push(lines[i]);
                i += 1;
            }
            if i < lines.len() {
                i += 1;
            }
            blocks.push(CanvasBlock::CodeBlock(code.join("\n")));
            continue;
        }
        if trimmed == "---" || trimmed == "***" {
            blocks.push(CanvasBlock::Divider);
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("### ") {
            blocks.push(CanvasBlock::Heading {
                level: 3,
                text: rest.trim().to_owned(),
            });
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("## ") {
            blocks.push(CanvasBlock::Heading {
                level: 2,
                text: rest.trim().to_owned(),
            });
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            blocks.push(CanvasBlock::Heading {
                level: 1,
                text: rest.trim().to_owned(),
            });
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("> ") {
            blocks.push(CanvasBlock::Quote(rest.trim().to_owned()));
            i += 1;
            continue;
        }
        if trimmed.starts_with("- [ ] ")
            || trimmed.starts_with("- [x] ")
            || trimmed.starts_with("- [X] ")
        {
            let mut items = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim();
                if let Some(rest) = t.strip_prefix("- [ ] ") {
                    items.push((false, rest.trim().to_owned()));
                    i += 1;
                } else if let Some(rest) = t
                    .strip_prefix("- [x] ")
                    .or_else(|| t.strip_prefix("- [X] "))
                {
                    items.push((true, rest.trim().to_owned()));
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(CanvasBlock::Checklist(items));
            continue;
        }
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            let mut items = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim();
                if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
                    items.push(rest.trim().to_owned());
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(CanvasBlock::Bullets(items));
            continue;
        }
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let split_row = |r: &str| -> Vec<String> {
                r.trim_matches('|')
                    .split('|')
                    .map(|c| c.trim().to_owned())
                    .collect()
            };
            let header = split_row(trimmed);
            i += 1;
            if i < lines.len() && lines[i].trim().starts_with('|') && lines[i].contains("---") {
                i += 1;
            }
            let mut rows = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim();
                if t.starts_with('|') && t.ends_with('|') {
                    rows.push(split_row(t));
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(CanvasBlock::Table { header, rows });
            continue;
        }
        blocks.push(CanvasBlock::Paragraph(trimmed.to_owned()));
        i += 1;
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
            <img src="https://files.slack.com/files-pri/T1-F1/arch.png" alt="Architecture diagram" width="800" height="400" />
            <h2>Milestones</h2>
            <h3>Risks</h3>
        </body></html>"#;
        let doc = CanvasDoc::from_html("Launch Plan".into(), html);
        assert!(doc.blocks.iter().any(|b| matches!(
            b,
            CanvasBlock::Image {
                url,
                alt,
                width: Some(800),
                height: Some(400),
            }
                if url == "https://files.slack.com/files-pri/T1-F1/arch.png"
                && alt == "Architecture diagram"
        )));
        assert_eq!(
            doc.outline_headings(),
            vec![(1, "Launch Plan"), (2, "Milestones"), (3, "Risks")]
        );
    }

    #[test]
    fn preserves_section_ids_and_roundtrips_canvas_markdown() {
        let html = r#"<html><body>
            <h1 id="temp:C:h1">Runbook</h1>
            <ul id="temp:C:chk1" class="checklist">
                <li data-checked="true">Verify backup</li>
                <li data-checked="false">Cut release</li>
            </ul>
            <p id="temp:C:p1">All clear.</p>
        </body></html>"#;
        let doc = CanvasDoc::from_html("Runbook".into(), html);
        assert_eq!(
            doc.section_ids,
            vec![
                Some("temp:C:h1".to_owned()),
                Some("temp:C:chk1".to_owned()),
                Some("temp:C:p1".to_owned()),
            ]
        );
        let md = doc.to_markdown();
        assert!(md.contains("# Runbook"));
        assert!(md.contains("- [x] Verify backup"));
        assert!(md.contains("- [ ] Cut release"));
        let reparsed = CanvasDoc::from_markdown("Runbook".into(), "Live".into(), &md);
        assert_eq!(reparsed.blocks, doc.blocks);
    }

    #[test]
    fn cycles_heading_levels_and_moves_lines() {
        assert_eq!(
            cycle_last_line_heading("Notes\nShip it"),
            "Notes\n# Ship it"
        );
        assert_eq!(
            cycle_last_line_heading("Notes\n# Ship it"),
            "Notes\n## Ship it"
        );
        assert_eq!(
            cycle_last_line_heading("Notes\n## Ship it"),
            "Notes\n### Ship it"
        );
        assert_eq!(
            cycle_last_line_heading("Notes\n### Ship it"),
            "Notes\nShip it"
        );
        assert_eq!(
            move_last_line("First\nSecond\nThird", -1),
            "First\nThird\nSecond"
        );
        assert_eq!(move_last_line("Only", -1), "Only");
    }
}
