//! Canvas reader views, document chrome (header, find bar, outline), and layout orchestration
//! (`F-COLLAB-125`, `F-COLLAB-128`, `F-COLLAB-132`, `F-COLLAB-140`, `F-COLLAB-145`).

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, Size, h_flex, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, ClipboardItem, Div, Entity, FontWeight, InteractiveElement as _,
    ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    Window, div, px,
};
use slackless_core::api::CanvasEditOp;

use crate::canvas::blocks::{
    CanvasBlockDrag, append_section_edit_menu_items, build_block_callbacks, render_block,
    render_drag_handle, render_section_actions_button,
};
use crate::canvas::comments::render_comments_drawer;
use crate::canvas::editor::render_canvas_editor;
use crate::canvas::format::{canvas_file_markdown, is_image_filename};
use crate::canvas::model::{CanvasBlock, CanvasDoc, fixture};
use crate::canvas::parse::{classify_media_url, resolve_canvas_emoji};
use crate::canvas::{
    CanvasActions, CanvasUploadCompleteFn, UploadedCanvasFile, commit_block_edit, mutate_ui,
    ui_state,
};

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

/// Native live Canvas reader & editor (`F-COLLAB-125`, `F-COLLAB-132..136`, `F-SHELL-264`,
/// `F-SHELL-266`): shows loading, error state with browser fallback, or the parsed live canvas.
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
                            .placeholder("Write canvas markdown… (type / for insert commands)")
                    }))
                } else {
                    None
                };
                mutate_ui(
                    &key_edit,
                    |s| {
                        s.editing = should_open;
                        s.emoji_popover_open = false;
                        s.slash_menu_open = false;
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

    // Header "Upload file" button in read mode (`F-COLLAB-140`).
    let upload_header_btn = (can_edit && !ui.editing)
        .then(|| actions.on_upload_file.clone())
        .flatten()
        .map(|upload_cb| {
            let k_up = key.to_owned();
            let doc_up = canvas.clone();
            let edit_cb = actions.on_edit.clone();
            Button::new(SharedString::from(format!("canvas-header-upload-{key}")))
                .outline()
                .with_size(Size::XSmall)
                .icon(IconName::Plus)
                .label("Upload file")
                .tooltip("Upload a file or image into this canvas")
                .on_click(move |_, window, cx| {
                    let k_done = k_up.clone();
                    let doc_done = doc_up.clone();
                    let edit_done = edit_cb.clone();
                    let on_done: CanvasUploadCompleteFn = Rc::new(
                        move |uploaded: UploadedCanvasFile, window: &mut Window, cx: &mut App| {
                            let md = canvas_file_markdown(&uploaded.filename, &uploaded.url);
                            let mut updated = doc_done.clone();
                            let block = if is_image_filename(&uploaded.filename) {
                                CanvasBlock::Image {
                                    url: uploaded.url.clone(),
                                    alt: uploaded.filename.clone(),
                                    width: None,
                                    height: None,
                                }
                            } else if let Some((kind, mime)) =
                                classify_media_url(&uploaded.filename)
                            {
                                CanvasBlock::MediaEmbed {
                                    kind,
                                    title: uploaded.filename.clone(),
                                    url: uploaded.url.clone(),
                                    file_id: None,
                                    duration: None,
                                    mime: Some(mime.to_owned()),
                                }
                            } else {
                                CanvasBlock::Paragraph(md.clone())
                            };
                            updated.push_block(block);
                            let op = CanvasEditOp::InsertAtEnd { markdown: md };
                            commit_block_edit(
                                &k_done,
                                updated,
                                op,
                                is_fixture,
                                edit_done.as_ref(),
                                window,
                                cx,
                            );
                        },
                    );
                    upload_cb(on_done, window, cx);
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
            .label(if already {
                "Pinned to tabs"
            } else {
                "Pin to channel tabs"
            })
            .disabled(already)
            .tooltip("Pin this canvas to the channel header tabs")
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
                        .child(resolve_canvas_emoji(&canvas.title)),
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
        .children(upload_header_btn)
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
                let label = resolve_canvas_emoji(text);
                let jump_text = text.to_owned();
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
                        d.cursor_pointer()
                            .hover(move |el| el.bg(hover).text_color(fg))
                            .on_click(move |_, window, cx| {
                                input.update(cx, |state, cx| {
                                    state.set_value(jump_text.clone(), window, cx);
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
                actions.on_upload_file.clone(),
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
                let section_id_owned = canvas.section_ids.get(block_ix).and_then(|s| s.clone());
                let cbs = build_block_callbacks(
                    key,
                    canvas,
                    block_ix,
                    section_id_owned.clone(),
                    can_edit,
                    is_fixture,
                    &actions,
                );

                let block_count = canvas.blocks.len();
                let can_drag = can_edit && block_count > 1;
                let block_md = canvas.block_plain_text(block_ix).unwrap_or_default();
                let full_md_export = canvas.to_markdown();
                let section_menu_btn = render_section_actions_button(
                    key,
                    block_ix,
                    block_count,
                    can_edit,
                    is_fixture,
                    canvas,
                    section_id_owned.clone(),
                    block_md.clone(),
                    actions.on_edit.clone(),
                );

                let canvas_ctx = canvas.clone();
                let key_ctx = key.to_owned();
                let on_edit_ctx = actions.on_edit.clone();
                let on_pin_ctx = actions.on_pin_tab.clone();
                let already_pinned = ui.pinned_tab;
                let sid_ctx = section_id_owned.clone();

                let drag_key_over = key.to_owned();
                let drag_key_drop = key.to_owned();
                let canvas_drop = canvas.clone();
                let on_edit_drop = actions.on_edit.clone();

                let row = h_flex()
                    .id(SharedString::from(format!("canvas-blk-{key}-{block_ix}")))
                    .items_start()
                    .gap_1()
                    .rounded_sm()
                    .when(can_drag, |row| {
                        row.child(render_drag_handle(key, block_ix, block, cx))
                            .drag_over::<CanvasBlockDrag>(move |style, drag, _, cx| {
                                if drag.key == drag_key_over && drag.from_ix != block_ix {
                                    let themed = style
                                        .bg(cx.theme().accent.opacity(0.24))
                                        .border_color(cx.theme().link);
                                    if drag.from_ix > block_ix {
                                        themed.border_t_2()
                                    } else {
                                        themed.border_b_2()
                                    }
                                } else {
                                    style
                                }
                            })
                            .on_drop(move |drag: &CanvasBlockDrag, window, cx| {
                                if drag.key != drag_key_drop || drag.from_ix == block_ix {
                                    return;
                                }
                                let mut updated = canvas_drop.clone();
                                if let Some(op) = updated.reorder_block_op(drag.from_ix, block_ix) {
                                    commit_block_edit(
                                        &drag_key_drop,
                                        updated,
                                        op,
                                        is_fixture,
                                        on_edit_drop.as_ref(),
                                        window,
                                        cx,
                                    );
                                }
                            })
                    })
                    .child(div().flex_1().min_w_0().child(render_block(
                        key,
                        block_ix,
                        block,
                        section_id_owned.as_deref(),
                        actions.canvas_url.as_deref(),
                        &ui,
                        &cbs,
                        cx,
                    )))
                    .child(section_menu_btn);

                doc_col.child(row.context_menu(move |mut menu, _, _| {
                    let copy_text = block_md.clone();
                    menu = menu.item(PopupMenuItem::new("Copy markdown").on_click(
                        move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                            crate::actions::notify(
                                "Copied section",
                                "Copied canvas section markdown to clipboard.",
                                window,
                                cx,
                            );
                        },
                    ));
                    if can_edit {
                        menu = append_section_edit_menu_items(
                            menu,
                            &key_ctx,
                            block_ix,
                            block_count,
                            is_fixture,
                            &canvas_ctx,
                            sid_ctx.as_deref(),
                            on_edit_ctx.as_ref(),
                        );
                    }
                    let export_md = full_md_export.clone();
                    menu = menu
                        .separator()
                        .item(PopupMenuItem::new("Export Markdown").on_click(
                            move |_, window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(export_md.clone()));
                                crate::actions::notify(
                                    "Exported Markdown",
                                    "Copied full canvas markdown to clipboard.",
                                    window,
                                    cx,
                                );
                            },
                        ));
                    if let Some(pin_cb) = on_pin_ctx.clone() {
                        let k_pin = key_ctx.clone();
                        menu = menu.item(
                            PopupMenuItem::new("Pin to channel tabs")
                                .disabled(already_pinned)
                                .on_click(move |_, window, cx| {
                                    mutate_ui(&k_pin, |s| s.pinned_tab = true, window, cx);
                                    pin_cb(window, cx);
                                }),
                        );
                    }
                    menu
                }))
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
