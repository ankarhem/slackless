//! Canvas markdown section editor with selection inline formatting bar (`F-COLLAB-133`),
//! block & table helpers (`F-COLLAB-134..136`), quick emoji popover (`F-COLLAB-139`), file upload
//! insertion (`F-COLLAB-140`), and `/` slash-command insert palette (`F-COLLAB-143`).

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, Div, Entity, FontWeight, InteractiveElement as _, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::api::CanvasEditOp;

use crate::canvas::format::{
    BlockFormat, CANVAS_QUICK_EMOJIS, InlineFormat, SlashCommand, add_table_column_at,
    add_table_row_at, apply_block_format, apply_inline_format, apply_slash_command,
    canvas_file_markdown, detect_slash_query, filter_slash_commands, insert_text_at,
    move_last_line,
};
use crate::canvas::model::CanvasDoc;
use crate::canvas::{
    CanvasEditFn, CanvasUiState, CanvasUploadCompleteFn, CanvasUploadFileFn, UploadedCanvasFile,
    mutate_ui,
};

#[inline(never)]
#[allow(clippy::too_many_arguments)]
pub(super) fn render_canvas_editor(
    key: &str,
    canvas: &CanvasDoc,
    ui: &CanvasUiState,
    editor: Entity<TextareaState>,
    on_edit: Option<CanvasEditFn>,
    on_upload_file: Option<CanvasUploadFileFn>,
    is_fixture: bool,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let current_text = editor.read(cx).value().to_string();
    let current_sel = editor.read(cx).selected_range();
    let slash_match = detect_slash_query(&current_text, current_sel.end);
    let show_slash = ui.slash_menu_open || slash_match.is_some();

    let inline_bar =
        render_inline_toolbar(key, ui, &editor, on_upload_file.clone(), show_slash, cx);
    let block_bar = render_block_toolbar(key, &editor, cx);
    let emoji_popover = ui
        .emoji_popover_open
        .then(|| render_emoji_popover(key, &editor, cx));
    let slash_palette = show_slash
        .then(|| render_slash_palette(key, &editor, slash_match, current_sel, on_upload_file, cx));
    let footer = render_editor_footer(key, canvas, &editor, on_edit, is_fixture);

    v_flex()
        .gap_2()
        .child(inline_bar)
        .child(block_bar)
        .children(emoji_popover)
        .children(slash_palette)
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

fn make_upload_callback(
    key: &str,
    editor: &Entity<TextareaState>,
    replace_range: Option<Range<usize>>,
) -> CanvasUploadCompleteFn {
    let ed = editor.clone();
    let k = key.to_owned();
    Rc::new(
        move |uploaded: UploadedCanvasFile, window: &mut Window, cx: &mut App| {
            let (current, sel) = {
                let st = ed.read(cx);
                (st.value().to_string(), st.selected_range())
            };
            let target = replace_range.clone().unwrap_or(sel);
            let md = canvas_file_markdown(&uploaded.filename, &uploaded.url);
            let snippet = if current.trim().is_empty() || !target.is_empty() {
                md
            } else {
                format!("\n\n{md}")
            };
            let (next, new_sel) = insert_text_at(&current, target, &snippet);
            push_undo_and_set(&k, &ed, current, next, Some(new_sel), window, cx);
            mutate_ui(
                &k,
                |s| {
                    s.slash_menu_open = false;
                    s.emoji_popover_open = false;
                },
                window,
                cx,
            );
        },
    )
}

#[inline(never)]
fn render_inline_toolbar(
    key: &str,
    ui: &CanvasUiState,
    editor: &Entity<TextareaState>,
    on_upload_file: Option<CanvasUploadFileFn>,
    show_slash: bool,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let inline_btn =
        |id_suffix: &'static str, icon: IconName, tooltip: &'static str, fmt: InlineFormat| {
            let ed = editor.clone();
            let k = key.to_owned();
            Button::new(SharedString::from(format!("canvas-inl-{k}-{id_suffix}")))
                .ghost()
                .with_size(Size::XSmall)
                .icon(icon)
                .tooltip(tooltip)
                .on_click(move |_, window, cx| {
                    let (current, sel) = {
                        let st = ed.read(cx);
                        (st.value().to_string(), st.selected_range())
                    };
                    let (next, new_sel) = apply_inline_format(&current, sel, fmt);
                    push_undo_and_set(&k, &ed, current, next, Some(new_sel), window, cx);
                })
        };

    let can_undo = ui.undo_stack.len() > 1;
    let can_redo = !ui.redo_stack.is_empty();
    let ed_undo = editor.clone();
    let k_undo = key.to_owned();
    let ed_redo = editor.clone();
    let k_redo = key.to_owned();
    let k_emoji = key.to_owned();
    let k_slash = key.to_owned();

    h_flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .px_2()
        .py_1p5()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary.opacity(0.42))
        .child(inline_btn(
            "bold",
            IconName::Bold,
            "Bold (**text**)",
            InlineFormat::Bold,
        ))
        .child(inline_btn(
            "italic",
            IconName::Italic,
            "Italic (*text*)",
            InlineFormat::Italic,
        ))
        .child(inline_btn(
            "strike",
            IconName::Strikethrough,
            "Strikethrough (~~text~~)",
            InlineFormat::Strike,
        ))
        .child(inline_btn(
            "code",
            IconName::CodeXml,
            "Inline code (`code`)",
            InlineFormat::Code,
        ))
        .child(inline_btn(
            "link",
            IconName::Link,
            "Link ([label](url))",
            InlineFormat::Link,
        ))
        .child(div().w(px(1.)).h(px(16.)).bg(theme.border).mx_1())
        .child(
            Button::new(SharedString::from(format!("canvas-emoji-btn-{key}")))
                .when(ui.emoji_popover_open, |b| b.primary())
                .when(!ui.emoji_popover_open, |b| b.ghost())
                .with_size(Size::XSmall)
                .icon(IconName::FaceGrinning)
                .tooltip("Insert emoji")
                .on_click(move |_, window, cx| {
                    mutate_ui(
                        &k_emoji,
                        |s| {
                            s.emoji_popover_open = !s.emoji_popover_open;
                            if s.emoji_popover_open {
                                s.slash_menu_open = false;
                            }
                        },
                        window,
                        cx,
                    );
                }),
        )
        .when_some(on_upload_file, |bar, upload_cb| {
            let on_done = make_upload_callback(key, editor, None);
            bar.child(
                Button::new(SharedString::from(format!("canvas-upload-btn-{key}")))
                    .ghost()
                    .with_size(Size::XSmall)
                    .icon(IconName::Plus)
                    .label("File")
                    .tooltip("Upload a file or image into this canvas")
                    .on_click(move |_, window, cx| {
                        upload_cb(on_done.clone(), window, cx);
                    }),
            )
        })
        .child(
            Button::new(SharedString::from(format!("canvas-slash-btn-{key}")))
                .when(show_slash, |b| b.primary())
                .when(!show_slash, |b| b.outline())
                .with_size(Size::XSmall)
                .icon(IconName::SquareSlash)
                .label("Insert")
                .tooltip("Open slash insert menu (or type / at start of line)")
                .on_click(move |_, window, cx| {
                    mutate_ui(
                        &k_slash,
                        |s| {
                            s.slash_menu_open = !s.slash_menu_open;
                            if s.slash_menu_open {
                                s.emoji_popover_open = false;
                            }
                        },
                        window,
                        cx,
                    );
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
        )
}

#[inline(never)]
fn render_block_toolbar(key: &str, editor: &Entity<TextareaState>, cx: &App) -> Div {
    let theme = cx.theme();
    let block_btn =
        |id_suffix: &'static str, label: &'static str, tooltip: &'static str, fmt: BlockFormat| {
            let ed = editor.clone();
            let k = key.to_owned();
            Button::new(SharedString::from(format!("canvas-blk-{k}-{id_suffix}")))
                .outline()
                .with_size(Size::XSmall)
                .label(label)
                .tooltip(tooltip)
                .on_click(move |_, window, cx| {
                    let (current, sel) = {
                        let st = ed.read(cx);
                        (st.value().to_string(), st.selected_range())
                    };
                    let (next, new_sel) = apply_block_format(&current, sel, fmt);
                    push_undo_and_set(&k, &ed, current, next, Some(new_sel), window, cx);
                })
        };

    let ed_add_row = editor.clone();
    let k_add_row = key.to_owned();
    let ed_add_col = editor.clone();
    let k_add_col = key.to_owned();
    let ed_up = editor.clone();
    let k_up = key.to_owned();
    let ed_down = editor.clone();
    let k_down = key.to_owned();

    h_flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .px_2()
        .py_1p5()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary.opacity(0.22))
        .child(block_btn(
            "heading",
            "H",
            "Cycle heading level (plain / H1 / H2 / H3)",
            BlockFormat::CycleHeading,
        ))
        .child(block_btn(
            "quote",
            "Quote",
            "Toggle blockquote (> )",
            BlockFormat::Quote,
        ))
        .child(block_btn(
            "bullet",
            "Bullets",
            "Toggle bulleted list (- )",
            BlockFormat::Bullet,
        ))
        .child(block_btn(
            "numbered",
            "1. List",
            "Toggle numbered list (1. )",
            BlockFormat::Numbered,
        ))
        .child(block_btn(
            "checklist",
            "Checklist",
            "Toggle checklist (- [ ] )",
            BlockFormat::Checklist,
        ))
        .child(block_btn(
            "codeblock",
            "Code block",
            "Wrap selection or insert fenced code block",
            BlockFormat::CodeBlock,
        ))
        .child(block_btn(
            "table",
            "Table",
            "Insert 3-column markdown table",
            BlockFormat::Table,
        ))
        .child(
            Button::new(SharedString::from(format!("canvas-tbl-row-{key}")))
                .outline()
                .with_size(Size::XSmall)
                .label("+ Row")
                .tooltip("Add row to active markdown table")
                .on_click(move |_, window, cx| {
                    let (current, sel) = {
                        let st = ed_add_row.read(cx);
                        (st.value().to_string(), st.selected_range())
                    };
                    let (next, new_sel) = add_table_row_at(&current, sel.end);
                    push_undo_and_set(
                        &k_add_row,
                        &ed_add_row,
                        current,
                        next,
                        Some(new_sel),
                        window,
                        cx,
                    );
                }),
        )
        .child(
            Button::new(SharedString::from(format!("canvas-tbl-col-{key}")))
                .outline()
                .with_size(Size::XSmall)
                .label("+ Col")
                .tooltip("Add column to active markdown table")
                .on_click(move |_, window, cx| {
                    let (current, sel) = {
                        let st = ed_add_col.read(cx);
                        (st.value().to_string(), st.selected_range())
                    };
                    let (next, new_sel) = add_table_column_at(&current, sel.end);
                    push_undo_and_set(
                        &k_add_col,
                        &ed_add_col,
                        current,
                        next,
                        Some(new_sel),
                        window,
                        cx,
                    );
                }),
        )
        .child(block_btn(
            "divider",
            "Divider",
            "Insert horizontal divider (---)",
            BlockFormat::Divider,
        ))
        .child(div().flex_1())
        .child(
            Button::new(SharedString::from(format!("canvas-line-up-{key}")))
                .ghost()
                .with_size(Size::XSmall)
                .icon(IconName::ArrowUp)
                .tooltip("Move last line up")
                .on_click(move |_, window, cx| {
                    let current = ed_up.read(cx).value().to_string();
                    let next = move_last_line(&current, -1);
                    push_undo_and_set(&k_up, &ed_up, current, next, None, window, cx);
                }),
        )
        .child(
            Button::new(SharedString::from(format!("canvas-line-down-{key}")))
                .ghost()
                .with_size(Size::XSmall)
                .icon(IconName::ArrowDown)
                .tooltip("Move last line down")
                .on_click(move |_, window, cx| {
                    let current = ed_down.read(cx).value().to_string();
                    let next = move_last_line(&current, 1);
                    push_undo_and_set(&k_down, &ed_down, current, next, None, window, cx);
                }),
        )
}

#[inline(never)]
fn render_emoji_popover(key: &str, editor: &Entity<TextareaState>, cx: &App) -> Div {
    let theme = cx.theme();
    let mut grid = h_flex().flex_wrap().gap_1();
    for &code in CANVAS_QUICK_EMOJIS {
        let glyph = crate::emoji::display_glyph(code).unwrap_or_else(|| format!(":{code}:"));
        let ed = editor.clone();
        let k = key.to_owned();
        let insert_glyph = glyph.clone();
        grid = grid.child(
            Button::new(SharedString::from(format!(
                "canvas-pick-emoji-{key}-{code}"
            )))
            .ghost()
            .with_size(Size::XSmall)
            .label(glyph)
            .tooltip(format!(":{code}:"))
            .on_click(move |_, window, cx| {
                let (current, sel) = {
                    let st = ed.read(cx);
                    (st.value().to_string(), st.selected_range())
                };
                let (next, new_sel) = insert_text_at(&current, sel, &insert_glyph);
                push_undo_and_set(&k, &ed, current, next, Some(new_sel), window, cx);
                mutate_ui(&k, |s| s.emoji_popover_open = false, window, cx);
            }),
        );
    }
    let k_close = key.to_owned();
    v_flex()
        .gap_1p5()
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.muted_foreground)
                        .child("Quick emoji"),
                )
                .child(
                    Button::new(SharedString::from(format!("canvas-emoji-close-{key}")))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::X)
                        .on_click(move |_, window, cx| {
                            mutate_ui(&k_close, |s| s.emoji_popover_open = false, window, cx);
                        }),
                ),
        )
        .child(grid)
}

#[inline(never)]
fn render_slash_palette(
    key: &str,
    editor: &Entity<TextareaState>,
    slash_match: Option<(Range<usize>, String)>,
    current_sel: Range<usize>,
    on_upload_file: Option<CanvasUploadFileFn>,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let (replace_range, query) = slash_match.unwrap_or_else(|| (current_sel, String::new()));
    let matches = filter_slash_commands(&query);
    let k_close = key.to_owned();
    let hover_bg = theme.accent;
    let muted = theme.muted_foreground;
    let fg = theme.foreground;

    let mut list = v_flex().gap_0p5();
    if matches.is_empty() {
        list = list.child(
            div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(muted)
                .child("No matching insert commands."),
        );
    } else {
        for spec in matches {
            let ed = editor.clone();
            let k = key.to_owned();
            let range = replace_range.clone();
            let upload = on_upload_file.clone();
            let on_done = make_upload_callback(key, editor, Some(replace_range.clone()));
            let cmd = spec.cmd;
            list = list.child(
                h_flex()
                    .id(SharedString::from(format!(
                        "canvas-slash-item-{key}-{}",
                        spec.trigger
                    )))
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(move |el| el.bg(hover_bg))
                    .on_click(move |_, window, cx| {
                        if cmd == SlashCommand::Emoji {
                            if !range.is_empty() {
                                let current = ed.read(cx).value().to_string();
                                let (next, new_sel) = insert_text_at(&current, range.clone(), "");
                                push_undo_and_set(
                                    &k,
                                    &ed,
                                    current,
                                    next,
                                    Some(new_sel),
                                    window,
                                    cx,
                                );
                            }
                            mutate_ui(
                                &k,
                                |s| {
                                    s.slash_menu_open = false;
                                    s.emoji_popover_open = true;
                                },
                                window,
                                cx,
                            );
                            return;
                        }
                        if cmd == SlashCommand::File {
                            if let Some(cb) = &upload {
                                cb(on_done.clone(), window, cx);
                            } else {
                                mutate_ui(&k, |s| s.slash_menu_open = false, window, cx);
                            }
                            return;
                        }
                        let current = ed.read(cx).value().to_string();
                        let (next, new_sel) = apply_slash_command(&current, range.clone(), cmd);
                        push_undo_and_set(&k, &ed, current, next, Some(new_sel), window, cx);
                        mutate_ui(&k, |s| s.slash_menu_open = false, window, cx);
                    })
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg)
                                    .child(spec.label),
                            )
                            .child(div().text_xs().text_color(muted).child(spec.description)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("/{}", spec.trigger)),
                    ),
            );
        }
    }

    v_flex()
        .gap_1()
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(muted)
                        .child(if query.is_empty() {
                            "Insert block (slash commands)".to_owned()
                        } else {
                            format!("Insert matching /{query}")
                        }),
                )
                .child(
                    Button::new(SharedString::from(format!("canvas-slash-close-{key}")))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::X)
                        .on_click(move |_, window, cx| {
                            mutate_ui(&k_close, |s| s.slash_menu_open = false, window, cx);
                        }),
                ),
        )
        .child(list)
}

#[inline(never)]
fn render_editor_footer(
    key: &str,
    canvas: &CanvasDoc,
    editor: &Entity<TextareaState>,
    on_edit: Option<CanvasEditFn>,
    is_fixture: bool,
) -> Div {
    let k_cancel = key.to_owned();
    let k_save = key.to_owned();
    let ed_save = editor.clone();
    let title_save = canvas.title.clone();
    let meta_save = canvas.meta.clone();

    h_flex()
        .justify_end()
        .gap_2()
        .child(
            Button::new(SharedString::from(format!("canvas-cancel-{key}")))
                .outline()
                .with_size(Size::Small)
                .label("Cancel")
                .on_click(move |_, window, cx| {
                    mutate_ui(
                        &k_cancel,
                        |s| {
                            s.editing = false;
                            s.emoji_popover_open = false;
                            s.slash_menu_open = false;
                        },
                        window,
                        cx,
                    );
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
                            s.emoji_popover_open = false;
                            s.slash_menu_open = false;
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
        )
}

pub(super) fn push_undo_and_set(
    key: &str,
    editor: &Entity<TextareaState>,
    current: String,
    next: String,
    new_selection: Option<Range<usize>>,
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
    editor.update(cx, |state, cx| {
        state.set_value(next, window, cx);
        if let Some(sel) = new_selection {
            state.set_selected_range(sel, cx);
        }
    });
}
