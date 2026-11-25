//! Canvas reader views, document chrome (header, find bar, outline, embedded files strip,
//! updated banner, delete confirmation, keyboard focus regions & shortcuts), and layout
//! orchestration (`F-COLLAB-125..126`, `F-COLLAB-128`, `F-COLLAB-132`, `F-COLLAB-140`,
//! `F-COLLAB-145`, `F-COLLAB-152`, `F-COLLAB-155..156`, `F-COLLAB-158`, `F-COLLAB-160..174`,
//! `F-COLLAB-182..185`, `F-COLLAB-194..196`).

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, Size, h_flex, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, App, AppContext as _, ClipboardItem, Div, Entity, FontWeight, InteractiveElement as _,
    IntoElement, KeyDownEvent, ParentElement as _, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::api::{CanvasEditOp, format_canvas_error};
use slackless_core::types::UserId;

use crate::canvas::blocks::{
    CanvasBlockDrag, CanvasBlockRenderCtx, SectionEditCtx, append_section_edit_menu_items,
    build_block_callbacks, render_block, render_drag_handle, render_inline_section_actions_bar,
    render_section_actions_button,
};
use crate::canvas::comments::{render_comments_drawer, sample_comment_threads};
use crate::canvas::editor::{CanvasEditorCtx, render_canvas_editor};
use crate::canvas::export::{canvas_print_allowed, trigger_canvas_print};
use crate::canvas::format::{canvas_file_markdown, is_image_filename};
use crate::canvas::keyboard::{
    CanvasActivation, CanvasFocusRegion, activate_focused_block, cycle_region, step_block,
    toggle_section_actions_shortcut,
};
use crate::canvas::model::{CanvasBlock, CanvasDoc, fixture};
use crate::canvas::parse::{classify_media_url, resolve_canvas_emoji};
use crate::canvas::share::render_share_panel;
use crate::canvas::{
    CanvasActions, CanvasUiState, CanvasUploadCompleteFn, OpenBrowserFn, UploadedCanvasFile,
    commit_block_edit, mutate_ui, section_replace_op, ui_state,
};

/// Shared document-level render context passed to header, banner, body, drawer, and key helpers.
struct CanvasDocCtx<'a> {
    key: &'a str,
    canvas: &'a CanvasDoc,
    ui: &'a CanvasUiState,
    actions: &'a CanvasActions,
    can_edit: bool,
    is_fixture: bool,
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
    let open_browser = actions.open_browser.clone();
    match loaded {
        None => render_loading_state(open_browser, cx),
        Some(Err(err)) => render_error_state(err, open_browser, cx),
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

fn render_loading_state(open_browser: Option<OpenBrowserFn>, cx: &App) -> Stateful<Div> {
    v_flex()
        .id("canvas-loading")
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
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
        })
}

fn render_error_state(err: &str, open_browser: Option<OpenBrowserFn>, cx: &App) -> Stateful<Div> {
    v_flex()
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
                .text_color(cx.theme().muted_foreground)
                .child(format_canvas_error(err)),
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
        })
}

fn render_deleted_state(key: &str, is_fixture: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let k_restore = key.to_owned();
    v_flex()
        .id(SharedString::from(format!("canvas-deleted-{key}")))
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .px_8()
        .child(
            Icon::new(IconName::Trash)
                .size(px(28.))
                .text_color(theme.muted_foreground),
        )
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child("Canvas deleted"),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("This canvas has been permanently deleted."),
        )
        .when(is_fixture, |col| {
            col.child(
                Button::new(SharedString::from(format!("canvas-restore-{key}")))
                    .outline()
                    .with_size(Size::Small)
                    .icon(IconName::RotateCcw)
                    .label("Restore sample canvas")
                    .on_click(move |_, window, cx| {
                        mutate_ui(
                            &k_restore,
                            |s| {
                                s.deleted = false;
                                s.confirm_delete = false;
                            },
                            window,
                            cx,
                        );
                    }),
            )
        })
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
    let ui = ui_state(key, cx);
    if ui.deleted {
        return render_deleted_state(key, is_fixture, cx);
    }

    let doc_ctx = CanvasDocCtx {
        key,
        canvas,
        ui: &ui,
        actions: &actions,
        can_edit: is_fixture || actions.on_edit.is_some(),
        is_fixture,
    };

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

    let headings = canvas.outline_headings();
    let has_outline = headings.len() >= 2;
    let outline_bar = has_outline.then(|| {
        render_outline_bar(
            headings,
            ui.keyboard.region == CanvasFocusRegion::Outline,
            search,
            cx,
        )
    });

    let header = render_canvas_header(&doc_ctx, cx);
    let delete_confirm_bar = render_delete_confirm_bar(&doc_ctx, cx);
    let updated_banner = render_updated_banner(&doc_ctx, cx);
    let main_status_banner = render_status_banner(&doc_ctx, cx);
    let find_bar = render_find_bar(&needle, match_note, search, cx);
    let files_strip = render_files_strip(&doc_ctx, cx);
    let document = render_document_body(&doc_ctx, filtered, &needle, cx);
    let drawer = render_side_drawer(&doc_ctx, cx);
    let handle_key_down = build_canvas_key_handler(&doc_ctx, has_outline);

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
                .children(delete_confirm_bar)
                .children(updated_banner)
                .children(main_status_banner)
                .when(!ui.reader_mode, |col| {
                    col.children(find_bar)
                        .children(outline_bar)
                        .children(files_strip)
                })
                .child(document),
        );

    let root = h_flex()
        .id(SharedString::from(format!("canvas-root-{key}")))
        .size_full()
        .when_some(ui.focus_handle.clone(), |el, fh| el.track_focus(&fh))
        .on_key_down(handle_key_down)
        .child(main_scroll);

    if let Some(side) = drawer {
        root.child(side)
    } else {
        root
    }
}

fn render_canvas_header(doc_ctx: &CanvasDocCtx<'_>, cx: &App) -> Div {
    let theme = cx.theme();
    let key = doc_ctx.key;
    let ui = doc_ctx.ui;
    let actions = doc_ctx.actions;

    let effective_permalink = actions
        .meta
        .as_ref()
        .and_then(|m| m.permalink.as_deref())
        .filter(|s| !s.is_empty())
        .or(actions.canvas_url.as_deref())
        .unwrap_or("https://slackless.slack.com/docs/T01SAMPLE/F01CANVAS")
        .to_owned();

    let key_copy_link = key.to_owned();
    let copy_link_btn = Button::new(SharedString::from(format!("canvas-copy-link-{key}")))
        .outline()
        .with_size(Size::XSmall)
        .icon(if ui.link_copied {
            IconName::Check
        } else {
            IconName::Link
        })
        .label(if ui.link_copied {
            "Copied"
        } else {
            "Copy link"
        })
        .tooltip("Copy canvas permalink")
        .on_click(move |_, window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(effective_permalink.clone()));
            mutate_ui(&key_copy_link, |s| s.link_copied = true, window, cx);
            crate::actions::notify(
                "Link copied",
                "Canvas link copied to clipboard.",
                window,
                cx,
            );
        });

    let key_share = key.to_owned();
    let share_btn = Button::new(SharedString::from(format!("canvas-share-btn-{key}")))
        .when(ui.share_open, |b| b.primary())
        .when(!ui.share_open, |b| b.outline())
        .with_size(Size::XSmall)
        .icon(IconName::UserPlus)
        .label("Share")
        .tooltip("Share canvas & manage access permissions")
        .on_click(move |_, window, cx| {
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Channel or user IDs (e.g. C01ENG, U01ALICE)")
                    .auto_grow(1, 2)
            });
            mutate_ui(
                &key_share,
                |s| {
                    s.share_open = !s.share_open;
                    if s.share_open {
                        s.comments_open = false;
                        if s.share_target_input.is_none() {
                            s.share_target_input = Some(input);
                        }
                    }
                },
                window,
                cx,
            );
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
        let doc = doc_ctx.canvas.clone();
        Button::new(SharedString::from(format!("canvas-popout-{key}")))
            .outline()
            .with_size(Size::XSmall)
            .icon(IconName::ExternalLink)
            .label("Pop out")
            .tooltip("Open canvas in a new window")
            .on_click(move |_, window, cx| pop(doc.clone(), window, cx))
    });

    let header_focused = ui.keyboard.region == CanvasFocusRegion::Header;
    h_flex()
        .items_center()
        .flex_wrap()
        .gap_2()
        .when(header_focused, |row| {
            row.p_1p5().rounded_md().border_1().border_color(theme.ring)
        })
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
                        .child(resolve_canvas_emoji(&doc_ctx.canvas.title)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(doc_ctx.canvas.meta.clone()),
                ),
        )
        .when(!doc_ctx.can_edit, |row| row.child(read_only_badge(cx)))
        .child(copy_link_btn)
        .child(share_btn)
        .child(reader_btn)
        .child(render_comments_toggle_btn(doc_ctx))
        .children(pin_tab_btn)
        .children(render_header_upload_btn(doc_ctx))
        .children(render_edit_toggle_btn(doc_ctx))
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
        })
        .child(render_overflow_menu_btn(doc_ctx))
}

fn render_edit_toggle_btn(doc_ctx: &CanvasDocCtx<'_>) -> Option<impl IntoElement> {
    if !doc_ctx.can_edit {
        return None;
    }
    let key = doc_ctx.key;
    let key_edit = key.to_owned();
    let canvas_for_edit = doc_ctx.canvas.clone();
    let editing = doc_ctx.ui.editing;
    Some(
        Button::new(SharedString::from(format!("canvas-edit-toggle-{key}")))
            .outline()
            .with_size(Size::XSmall)
            .icon(if editing {
                IconName::Eye
            } else {
                IconName::Pencil
            })
            .label(if editing { "Preview" } else { "Edit" })
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
            }),
    )
}

/// Header "Upload file" button in read mode (`F-COLLAB-140`).
fn render_header_upload_btn(doc_ctx: &CanvasDocCtx<'_>) -> Option<impl IntoElement> {
    if !doc_ctx.can_edit || doc_ctx.ui.editing {
        return None;
    }
    let upload_cb = doc_ctx.actions.on_upload_file.clone()?;
    let key = doc_ctx.key;
    let k_up = key.to_owned();
    let doc_up = doc_ctx.canvas.clone();
    let edit_cb = doc_ctx.actions.on_edit.clone();
    let is_fixture = doc_ctx.is_fixture;

    Some(
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
                        } else if let Some((kind, mime)) = classify_media_url(&uploaded.filename) {
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
            }),
    )
}

fn render_comments_toggle_btn(doc_ctx: &CanvasDocCtx<'_>) -> impl IntoElement {
    let key = doc_ctx.key;
    let ui = doc_ctx.ui;
    let is_fixture = doc_ctx.is_fixture;
    let key_comments = key.to_owned();
    let on_load_comments = doc_ctx.actions.on_load_comments.clone();
    let comment_count = doc_ctx
        .actions
        .comment_threads
        .as_ref()
        .map(Vec::len)
        .unwrap_or_else(|| {
            if is_fixture && !ui.comments_seeded {
                sample_comment_threads().len()
            } else {
                ui.comments.len()
            }
        });
    let comments_label = if comment_count == 0 {
        "Comments".to_owned()
    } else {
        format!("Comments ({comment_count})")
    };

    Button::new(SharedString::from(format!("canvas-comments-{key}")))
        .when(ui.comments_open, |b| b.primary())
        .when(!ui.comments_open, |b| b.outline())
        .with_size(Size::XSmall)
        .icon(IconName::MessageSquare)
        .label(comments_label)
        .tooltip("Toggle threaded canvas comments drawer")
        .on_click(move |_, window, cx| {
            let opening = !ui_state(&key_comments, cx).comments_open;
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Start a comment thread…")
                    .auto_grow(1, 4)
            });
            mutate_ui(
                &key_comments,
                |s| {
                    s.comments_open = opening;
                    if s.comments_open {
                        s.share_open = false;
                        if s.comment_input.is_none() {
                            s.comment_input = Some(input);
                        }
                        if is_fixture && !s.comments_seeded {
                            s.comments = sample_comment_threads();
                            s.comments_seeded = true;
                        }
                    }
                },
                window,
                cx,
            );
            if opening && let Some(load_cb) = &on_load_comments {
                load_cb(window, cx);
            }
        })
}

/// Header overflow menu (`...`): Print…, Save as PDF…, Export Markdown, Delete canvas
/// (`F-COLLAB-158`, `F-COLLAB-182..184`).
fn render_overflow_menu_btn(doc_ctx: &CanvasDocCtx<'_>) -> impl IntoElement {
    let key = doc_ctx.key;
    let print_allowed = canvas_print_allowed(doc_ctx.actions.meta.as_ref());
    let doc_print = doc_ctx.canvas.clone();
    let doc_pdf = doc_ctx.canvas.clone();
    let full_md = doc_ctx.canvas.to_markdown();
    let on_print = doc_ctx.actions.on_print_canvas.clone();
    let on_pdf = doc_ctx.actions.on_save_pdf.clone();
    let k_del = key.to_owned();
    let can_delete = doc_ctx.is_fixture || doc_ctx.actions.on_delete_canvas.is_some();

    Button::new(SharedString::from(format!("canvas-more-menu-{key}")))
        .outline()
        .with_size(Size::XSmall)
        .icon(IconName::Ellipsis)
        .tooltip(if print_allowed {
            "Canvas actions (Print, Save as PDF, Delete)"
        } else {
            "Canvas actions (Printing disabled by workspace admin)"
        })
        .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
            let d_print = doc_print.clone();
            let print_cb = on_print.clone();
            let print_label = if print_allowed {
                "Print…"
            } else {
                "Print… (disabled by workspace admin)"
            };
            menu = menu.item(
                PopupMenuItem::new(print_label)
                    .icon(IconName::FileText)
                    .disabled(!print_allowed)
                    .on_click(move |_, window, cx| {
                        if !print_allowed {
                            return;
                        }
                        if let Some(cb) = &print_cb {
                            cb(d_print.clone(), window, cx);
                        } else {
                            match trigger_canvas_print(&d_print) {
                                Ok(_) => crate::actions::notify(
                                    "Print canvas",
                                    "Opened printable canvas preview.",
                                    window,
                                    cx,
                                ),
                                Err(err) => {
                                    crate::actions::notify("Print unavailable", err, window, cx)
                                }
                            }
                        }
                    }),
            );

            let d_pdf = doc_pdf.clone();
            let pdf_cb = on_pdf.clone();
            let pdf_label = if print_allowed {
                "Save as PDF…"
            } else {
                "Save as PDF… (disabled by workspace admin)"
            };
            menu = menu.item(
                PopupMenuItem::new(pdf_label)
                    .icon(IconName::Download)
                    .disabled(!print_allowed)
                    .on_click(move |_, window, cx| {
                        if !print_allowed {
                            return;
                        }
                        if let Some(cb) = &pdf_cb {
                            cb(d_pdf.clone(), window, cx);
                        } else {
                            crate::actions::notify(
                                "Save as PDF",
                                "PDF export handler not configured.",
                                window,
                                cx,
                            );
                        }
                    }),
            );

            let md_copy = full_md.clone();
            menu = menu.item(
                PopupMenuItem::new("Export Markdown")
                    .icon(IconName::Copy)
                    .on_click(move |_, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(md_copy.clone()));
                        crate::actions::notify(
                            "Exported Markdown",
                            "Copied full canvas markdown to clipboard.",
                            window,
                            cx,
                        );
                    }),
            );

            if can_delete {
                let k_d = k_del.clone();
                menu = menu.separator().item(
                    PopupMenuItem::new("Delete canvas")
                        .icon(IconName::Trash)
                        .on_click(move |_, window, cx| {
                            mutate_ui(&k_d, |s| s.confirm_delete = true, window, cx);
                        }),
                );
            }
            menu
        })
}

/// Inline two-step Delete confirmation bar (`F-COLLAB-158`).
fn render_delete_confirm_bar(doc_ctx: &CanvasDocCtx<'_>, cx: &App) -> Option<Div> {
    if !doc_ctx.ui.confirm_delete {
        return None;
    }
    let theme = cx.theme();
    let key = doc_ctx.key;
    let k_yes = key.to_owned();
    let k_no = key.to_owned();
    let on_del = doc_ctx.actions.on_delete_canvas.clone();
    let is_fixture = doc_ctx.is_fixture;

    Some(
        h_flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(theme.danger.opacity(0.5))
            .bg(theme.danger.opacity(0.1))
            .child(
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size(px(14.))
                            .text_color(theme.danger),
                    )
                    .child("Delete this canvas permanently? This cannot be undone."),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .child(
                        Button::new(SharedString::from(format!("canvas-del-confirm-{key}")))
                            .primary()
                            .with_size(Size::XSmall)
                            .icon(IconName::Trash)
                            .label("Delete permanently")
                            .on_click(move |_, window, cx| {
                                if let Some(cb) = &on_del {
                                    cb(window, cx);
                                }
                                mutate_ui(
                                    &k_yes,
                                    |s| {
                                        s.confirm_delete = false;
                                        if is_fixture {
                                            s.deleted = true;
                                        }
                                    },
                                    window,
                                    cx,
                                );
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("canvas-del-cancel-{key}")))
                            .outline()
                            .with_size(Size::XSmall)
                            .label("Cancel")
                            .on_click(move |_, window, cx| {
                                mutate_ui(&k_no, |s| s.confirm_delete = false, window, cx);
                            }),
                    ),
            ),
    )
}

/// Unobtrusive "Canvas updated" live banner (`F-COLLAB-185`).
fn render_updated_banner(doc_ctx: &CanvasDocCtx<'_>, cx: &App) -> Option<Div> {
    let note = doc_ctx.actions.updated_note.clone()?;
    let theme = cx.theme();
    let key = doc_ctx.key;
    let on_refresh = doc_ctx.actions.on_refresh_canvas.clone();
    let on_dismiss = doc_ctx.actions.on_dismiss_updated.clone();
    Some(
        h_flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_1p5()
            .rounded_md()
            .border_1()
            .border_color(theme.link.opacity(0.4))
            .bg(theme.link.opacity(0.1))
            .text_xs()
            .child(
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        Icon::new(IconName::Info)
                            .size(px(14.))
                            .text_color(theme.link),
                    )
                    .child(note),
            )
            .child(
                h_flex()
                    .gap_1()
                    .when_some(on_refresh, |row, refresh_cb| {
                        row.child(
                            Button::new(SharedString::from(format!("canvas-upd-refresh-{key}")))
                                .outline()
                                .with_size(Size::XSmall)
                                .icon(IconName::RefreshCw)
                                .label("Refresh")
                                .on_click(move |_, window, cx| refresh_cb(window, cx)),
                        )
                    })
                    .when_some(on_dismiss, |row, dismiss_cb| {
                        row.child(
                            Button::new(SharedString::from(format!("canvas-upd-dismiss-{key}")))
                                .ghost()
                                .with_size(Size::XSmall)
                                .icon(IconName::X)
                                .on_click(move |_, window, cx| dismiss_cb(window, cx)),
                        )
                    }),
            ),
    )
}

/// Surface server-enforced canvas errors inline when neither drawer is open
/// (`F-COLLAB-120`, `F-COLLAB-171..174`, `F-COLLAB-204`).
fn render_status_banner(doc_ctx: &CanvasDocCtx<'_>, cx: &App) -> Option<Div> {
    if doc_ctx.ui.share_open || doc_ctx.ui.comments_open {
        return None;
    }
    let (is_err, msg) = doc_ctx.actions.status_banner.as_ref()?;
    let theme = cx.theme();
    let fg = if *is_err { theme.danger } else { theme.success };
    let icon = if *is_err {
        IconName::TriangleAlert
    } else {
        IconName::Check
    };
    Some(
        h_flex()
            .items_center()
            .gap_1p5()
            .px_3()
            .py_1p5()
            .rounded_md()
            .border_1()
            .border_color(fg.opacity(0.45))
            .bg(fg.opacity(0.1))
            .text_xs()
            .child(Icon::new(icon).size(px(14.)).text_color(fg))
            .child(format_canvas_error(msg)),
    )
}

fn render_find_bar(
    needle: &str,
    match_note: Option<String>,
    search: Option<&Entity<TextareaState>>,
    cx: &App,
) -> Option<Div> {
    let input = search?;
    let theme = cx.theme();
    let m = crate::preferences::control_metrics(cx);
    Some(
        h_flex()
            .h(m.height)
            .pl_2p5()
            .pr_1()
            .gap_0p5()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(theme.input)
            .child(
                Icon::new(IconName::Search)
                    .flex_none()
                    .size(px(14.))
                    .text_color(theme.muted_foreground),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Textarea::new(input)
                        .with_size(m.input_size)
                        .appearance(false)
                        .bordered(false),
                ),
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
            }),
    )
}

fn render_outline_bar(
    headings: Vec<(u8, &str)>,
    outline_focused: bool,
    search: Option<&Entity<TextareaState>>,
    cx: &App,
) -> Div {
    let theme = cx.theme();
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
        .border_color(if outline_focused {
            theme.ring
        } else {
            theme.border
        })
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
}

/// Collapsible "Files in this canvas" strip (`F-COLLAB-126`).
fn render_files_strip(doc_ctx: &CanvasDocCtx<'_>, cx: &App) -> Option<Div> {
    let embedded_files = doc_ctx.canvas.embedded_files();
    if embedded_files.is_empty() {
        return None;
    }
    let theme = cx.theme();
    let key = doc_ctx.key;
    let k_toggle = key.to_owned();
    let file_count = embedded_files.len();
    let mut strip = v_flex()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary.opacity(0.22))
        .px_2p5()
        .py_1p5()
        .gap_1p5()
        .child(
            h_flex()
                .id(SharedString::from(format!("canvas-files-toggle-{key}")))
                .items_center()
                .justify_between()
                .cursor_pointer()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .on_click(move |_, window, cx| {
                    mutate_ui(
                        &k_toggle,
                        |s| s.files_strip_open = !s.files_strip_open,
                        window,
                        cx,
                    );
                })
                .child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            Icon::new(if doc_ctx.ui.files_strip_open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(px(12.)),
                        )
                        .child(Icon::new(IconName::Files).size(px(12.)))
                        .child(format!("Files in this canvas ({file_count})")),
                ),
        );

    if doc_ctx.ui.files_strip_open {
        let mut items_row = h_flex().flex_wrap().gap_1p5();
        for (ix, item) in embedded_files.into_iter().enumerate() {
            let k_jump = key.to_owned();
            let on_media = doc_ctx.actions.on_open_media.clone();
            let on_embed = doc_ctx.actions.on_open_embed.clone();
            let block_ix = item.block_ix;
            let url = item.url.clone();
            let file_id = item.file_id.clone();
            let embed_kind = item.embed_kind;
            let media_kind = item.media_kind;
            let hover_bg = theme.accent.opacity(0.45);
            items_row = items_row.child(
                h_flex()
                    .id(SharedString::from(format!("canvas-file-chip-{key}-{ix}")))
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .text_xs()
                    .on_click(move |_, window, cx| {
                        mutate_ui(
                            &k_jump,
                            |s| {
                                s.keyboard.region = CanvasFocusRegion::Document;
                                s.keyboard.focused_block = Some(block_ix);
                            },
                            window,
                            cx,
                        );
                        if let (Some(kind), Some(cb)) = (media_kind, &on_media) {
                            cb(url.clone(), kind, window, cx);
                        } else if let (Some(kind), Some(cb)) = (embed_kind, &on_embed) {
                            cb(
                                kind,
                                file_id.clone().unwrap_or_default(),
                                url.clone(),
                                window,
                                cx,
                            );
                        }
                    })
                    .child(
                        div()
                            .px_1()
                            .rounded_sm()
                            .bg(theme.secondary.opacity(0.7))
                            .text_color(theme.muted_foreground)
                            .child(item.kind_label),
                    )
                    .child(div().text_color(theme.foreground).child(item.title)),
            );
        }
        strip = strip.child(items_row);
    }

    Some(strip)
}

fn render_document_body(
    doc_ctx: &CanvasDocCtx<'_>,
    filtered: Vec<(usize, &CanvasBlock)>,
    needle: &str,
    cx: &App,
) -> Div {
    if doc_ctx.ui.editing {
        return if let Some(editor) = doc_ctx.ui.editor.clone() {
            render_canvas_editor(
                CanvasEditorCtx {
                    key: doc_ctx.key,
                    canvas: doc_ctx.canvas,
                    ui: doc_ctx.ui,
                    editor,
                    on_edit: doc_ctx.actions.on_edit.clone(),
                    on_upload_file: doc_ctx.actions.on_upload_file.clone(),
                    is_fixture: doc_ctx.is_fixture,
                },
                cx,
            )
        } else {
            v_flex()
        };
    }

    if filtered.is_empty() {
        return v_flex().py_8().items_center().child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(if needle.is_empty() {
                    "This canvas is empty."
                } else {
                    "No matching sections in this canvas."
                }),
        );
    }

    filtered
        .into_iter()
        .fold(v_flex().gap_3(), |doc_col, (block_ix, block)| {
            let section_id = doc_ctx
                .canvas
                .section_ids
                .get(block_ix)
                .and_then(|s| s.clone());
            let block_ctx = CanvasBlockRenderCtx {
                key: doc_ctx.key,
                canvas: doc_ctx.canvas,
                ui: doc_ctx.ui,
                actions: doc_ctx.actions,
                block_ix,
                section_id,
                can_edit: doc_ctx.can_edit,
                is_fixture: doc_ctx.is_fixture,
            };
            doc_col.child(render_block_row(&block_ctx, block, cx))
        })
}

fn render_block_row(
    ctx: &CanvasBlockRenderCtx<'_>,
    block: &CanvasBlock,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let key = ctx.key;
    let block_ix = ctx.block_ix;
    let block_count = ctx.canvas.blocks.len();
    let can_drag = ctx.can_edit && block_count > 1;
    let block_md = ctx.canvas.block_plain_text(block_ix).unwrap_or_default();
    let full_md_export = ctx.canvas.to_markdown();

    let cbs = build_block_callbacks(ctx);
    let section_menu_btn = render_section_actions_button(ctx, block_md.clone());
    let block_focused = ctx.ui.keyboard.region == CanvasFocusRegion::Document
        && ctx.ui.keyboard.focused_block == Some(block_ix);
    let inline_actions_bar = (ctx.ui.keyboard.section_menu_block == Some(block_ix))
        .then(|| render_inline_section_actions_bar(ctx, block_md.clone(), cx));

    let canvas_ctx = ctx.canvas.clone();
    let key_ctx = key.to_owned();
    let on_edit_ctx = ctx.actions.on_edit.clone();
    let on_pin_ctx = ctx.actions.on_pin_tab.clone();
    let already_pinned = ctx.ui.pinned_tab;
    let sid_ctx = ctx.section_id.clone();
    let can_edit = ctx.can_edit;
    let is_fixture = ctx.is_fixture;

    let drag_key_over = key.to_owned();
    let drag_key_drop = key.to_owned();
    let canvas_drop = ctx.canvas.clone();
    let on_edit_drop = ctx.actions.on_edit.clone();

    let row = h_flex()
        .id(SharedString::from(format!("canvas-blk-{key}-{block_ix}")))
        .items_start()
        .gap_1()
        .rounded_sm()
        .when(block_focused, |r| {
            r.p_1()
                .border_1()
                .border_color(theme.ring)
                .bg(theme.accent.opacity(0.14))
        })
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
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(render_block(ctx, block, &cbs, cx))
                .children(inline_actions_bar),
        )
        .child(section_menu_btn);

    row.context_menu(move |mut menu, _, _| {
        let copy_text = block_md.clone();
        menu = menu.item(
            PopupMenuItem::new("Copy markdown").on_click(move |_, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                crate::actions::notify(
                    "Copied section",
                    "Copied canvas section markdown to clipboard.",
                    window,
                    cx,
                );
            }),
        );
        if can_edit {
            let edit_ctx = SectionEditCtx {
                key: &key_ctx,
                block_ix,
                block_count,
                is_fixture,
                canvas: &canvas_ctx,
                section_id: sid_ctx.as_deref(),
                on_edit: on_edit_ctx.as_ref(),
            };
            menu = append_section_edit_menu_items(menu, &edit_ctx);
        }
        let export_md = full_md_export.clone();
        menu = menu
            .separator()
            .item(
                PopupMenuItem::new("Export Markdown").on_click(move |_, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(export_md.clone()));
                    crate::actions::notify(
                        "Exported Markdown",
                        "Copied full canvas markdown to clipboard.",
                        window,
                        cx,
                    );
                }),
            );
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
    })
}

fn render_side_drawer(doc_ctx: &CanvasDocCtx<'_>, cx: &App) -> Option<Stateful<Div>> {
    if doc_ctx.ui.share_open {
        Some(render_share_panel(
            doc_ctx.key,
            doc_ctx.actions.meta.as_ref(),
            doc_ctx.actions.canvas_url.as_deref(),
            doc_ctx.ui,
            doc_ctx.actions,
            doc_ctx.is_fixture,
            cx,
        ))
    } else if doc_ctx.ui.comments_open {
        Some(render_comments_drawer(
            doc_ctx.key,
            doc_ctx.ui,
            doc_ctx.actions,
            doc_ctx.is_fixture,
            cx,
        ))
    } else {
        None
    }
}

/// Keyboard handler for focus regions (`F6`/`Shift+F6`), block navigation (`Up`/`Down`),
/// embedded item activation (`Enter`/`Space`), and Section Actions shortcut (`Cmd+Shift+.`/`.`)
/// (`F-COLLAB-194`, `F-COLLAB-195`, `F-COLLAB-196`).
fn build_canvas_key_handler(
    doc_ctx: &CanvasDocCtx<'_>,
    has_outline: bool,
) -> impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static {
    let key_kb = doc_ctx.key.to_owned();
    let doc_kb = doc_ctx.canvas.clone();
    let can_edit = doc_ctx.can_edit;
    let is_fixture = doc_ctx.is_fixture;
    let on_edit_kb = doc_ctx.actions.on_edit.clone();
    let on_media_kb = doc_ctx.actions.on_open_media.clone();
    let on_embed_kb = doc_ctx.actions.on_open_embed.clone();
    let on_profile_kb = doc_ctx.actions.on_open_profile.clone();

    move |event: &KeyDownEvent, window: &mut Window, cx: &mut App| {
        let state = ui_state(&key_kb, cx);
        if state.editing {
            return;
        }
        let ks = &event.keystroke;
        let k = ks.key.as_str();
        if k.eq_ignore_ascii_case("f6") {
            let forward = !ks.modifiers.shift;
            mutate_ui(
                &key_kb,
                |s| {
                    s.keyboard.region =
                        cycle_region(s.keyboard.region, forward, has_outline, s.comments_open);
                    if s.keyboard.region == CanvasFocusRegion::Document
                        && s.keyboard.focused_block.is_none()
                        && !doc_kb.blocks.is_empty()
                    {
                        s.keyboard.focused_block = Some(0);
                    }
                },
                window,
                cx,
            );
            return;
        }
        if k == "escape" {
            if state.keyboard.section_menu_block.is_some() || state.confirm_delete {
                mutate_ui(
                    &key_kb,
                    |s| {
                        s.keyboard.section_menu_block = None;
                        s.confirm_delete = false;
                    },
                    window,
                    cx,
                );
            }
            return;
        }
        if (k == "." && !ks.modifiers.control && !ks.modifiers.alt)
            || (k == "." && ks.modifiers.platform && ks.modifiers.shift)
        {
            let count = doc_kb.blocks.len();
            mutate_ui(
                &key_kb,
                |s| {
                    let _ = toggle_section_actions_shortcut(&mut s.keyboard, count);
                },
                window,
                cx,
            );
            return;
        }
        if state.keyboard.region == CanvasFocusRegion::Document {
            if k == "up" || k == "down" {
                let delta = if k == "up" { -1 } else { 1 };
                let count = doc_kb.blocks.len();
                mutate_ui(
                    &key_kb,
                    |s| {
                        s.keyboard.focused_block =
                            step_block(s.keyboard.focused_block, delta, count);
                    },
                    window,
                    cx,
                );
                return;
            }
            if k == "enter" || k == "space" {
                let Some(act) = activate_focused_block(&doc_kb, state.keyboard.focused_block)
                else {
                    return;
                };
                match act {
                    CanvasActivation::ToggleChecklist { block_ix, item_ix } => {
                        if can_edit {
                            let mut updated = doc_kb.clone();
                            let sid = updated.section_ids.get(block_ix).and_then(|s| s.clone());
                            if let Some(section_md) =
                                updated.toggle_checklist_item(block_ix, item_ix)
                            {
                                let full_md = updated.to_markdown();
                                let op = section_replace_op(sid, section_md, full_md);
                                commit_block_edit(
                                    &key_kb,
                                    updated,
                                    op,
                                    is_fixture,
                                    on_edit_kb.as_ref(),
                                    window,
                                    cx,
                                );
                            }
                        }
                    }
                    CanvasActivation::OpenMedia { kind, url, .. } => {
                        if let Some(cb) = &on_media_kb {
                            cb(url, kind, window, cx);
                        }
                    }
                    CanvasActivation::OpenDocEmbed {
                        kind, file_id, url, ..
                    } => {
                        if let Some(cb) = &on_embed_kb {
                            cb(kind, file_id, url, window, cx);
                        }
                    }
                    CanvasActivation::OpenProfile { user_id, .. } => {
                        if let Some(cb) = &on_profile_kb {
                            cb(UserId::from(user_id.as_str()), window, cx);
                        }
                    }
                }
            }
        }
    }
}
