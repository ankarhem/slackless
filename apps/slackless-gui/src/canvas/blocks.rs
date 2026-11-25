//! Per-block rendering (headings, multi-line quotes, bullets, numbered lists, interactive
//! checklists, rich tables, images, emoji) and per-section actions menu (`F-COLLAB-134..136`,
//! `F-COLLAB-139`, `F-COLLAB-145`).

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, App, AppContext as _, ClipboardItem, Div, FontWeight, InteractiveElement as _,
    IntoElement, ObjectFit, ParentElement as _, SharedString, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, Window, div, img, px,
};
use slackless_core::api::CanvasEditOp;

use crate::canvas::model::{CanvasBlock, CanvasDoc};
use crate::canvas::parse::resolve_canvas_emoji;
use crate::canvas::{
    CanvasEditFn, CanvasUiState, commit_block_edit, mutate_ui, section_delete_op,
    section_insert_after_op, section_replace_op,
};

type ChecklistItemFn = Rc<dyn Fn(usize, &mut Window, &mut App)>;
type ChecklistAddFn = Rc<dyn Fn(String, &mut Window, &mut App)>;
type TableMutateFn = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone, Default)]
pub(super) struct BlockCallbacks {
    pub(super) on_toggle_check: Option<ChecklistItemFn>,
    pub(super) on_delete_check: Option<ChecklistItemFn>,
    pub(super) on_add_check_submit: Option<ChecklistAddFn>,
    pub(super) on_table_add_row: Option<TableMutateFn>,
    pub(super) on_table_add_col: Option<TableMutateFn>,
}

pub(super) fn build_block_callbacks(
    key: &str,
    canvas: &CanvasDoc,
    block_ix: usize,
    section_id: Option<String>,
    is_fixture: bool,
    on_edit: Option<CanvasEditFn>,
) -> BlockCallbacks {
    let on_toggle_check = {
        let k = key.to_owned();
        let doc = canvas.clone();
        let sid = section_id.clone();
        let cb = on_edit.clone();
        Some(
            Rc::new(move |item_ix: usize, window: &mut Window, cx: &mut App| {
                let mut updated = doc.clone();
                if let Some(section_md) = updated.toggle_checklist_item(block_ix, item_ix) {
                    let full_md = updated.to_markdown();
                    let op = section_replace_op(sid.clone(), section_md, full_md);
                    commit_block_edit(&k, updated, op, is_fixture, cb.as_ref(), window, cx);
                }
            }) as Rc<dyn Fn(usize, &mut Window, &mut App)>,
        )
    };

    let on_delete_check = {
        let k = key.to_owned();
        let doc = canvas.clone();
        let sid = section_id.clone();
        let cb = on_edit.clone();
        Some(
            Rc::new(move |item_ix: usize, window: &mut Window, cx: &mut App| {
                let mut updated = doc.clone();
                if let Some(res) = updated.delete_checklist_item(block_ix, item_ix) {
                    let full_md = updated.to_markdown();
                    let op = match res {
                        Some(section_md) => section_replace_op(sid.clone(), section_md, full_md),
                        None => section_delete_op(sid.clone(), full_md),
                    };
                    commit_block_edit(&k, updated, op, is_fixture, cb.as_ref(), window, cx);
                }
            }) as Rc<dyn Fn(usize, &mut Window, &mut App)>,
        )
    };

    let on_add_check_submit = {
        let k = key.to_owned();
        let doc = canvas.clone();
        let sid = section_id.clone();
        let cb = on_edit.clone();
        Some(
            Rc::new(move |label: String, window: &mut Window, cx: &mut App| {
                let mut updated = doc.clone();
                if let Some(section_md) = updated.add_checklist_item(block_ix, label) {
                    let full_md = updated.to_markdown();
                    let op = section_replace_op(sid.clone(), section_md, full_md);
                    commit_block_edit(&k, updated, op, is_fixture, cb.as_ref(), window, cx);
                }
            }) as Rc<dyn Fn(String, &mut Window, &mut App)>,
        )
    };

    let on_table_add_row = {
        let k = key.to_owned();
        let doc = canvas.clone();
        let sid = section_id.clone();
        let cb = on_edit.clone();
        Some(Rc::new(move |window: &mut Window, cx: &mut App| {
            let mut updated = doc.clone();
            if let Some(section_md) = updated.table_add_row(block_ix) {
                let full_md = updated.to_markdown();
                let op = section_replace_op(sid.clone(), section_md, full_md);
                commit_block_edit(&k, updated, op, is_fixture, cb.as_ref(), window, cx);
            }
        }) as Rc<dyn Fn(&mut Window, &mut App)>)
    };

    let on_table_add_col = {
        let k = key.to_owned();
        let doc = canvas.clone();
        let sid = section_id;
        let cb = on_edit;
        Some(Rc::new(move |window: &mut Window, cx: &mut App| {
            let mut updated = doc.clone();
            if let Some(section_md) = updated.table_add_column(block_ix) {
                let full_md = updated.to_markdown();
                let op = section_replace_op(sid.clone(), section_md, full_md);
                commit_block_edit(&k, updated, op, is_fixture, cb.as_ref(), window, cx);
            }
        }) as Rc<dyn Fn(&mut Window, &mut App)>)
    };

    BlockCallbacks {
        on_toggle_check,
        on_delete_check,
        on_add_check_submit,
        on_table_add_row,
        on_table_add_col,
    }
}

/// Compact per-section actions menu trigger button (`F-COLLAB-145`).
#[allow(clippy::too_many_arguments)]
pub(super) fn render_section_actions_button(
    key: &str,
    block_ix: usize,
    block_count: usize,
    can_edit: bool,
    is_fixture: bool,
    canvas: &CanvasDoc,
    section_id: Option<String>,
    block_md: String,
    on_edit: Option<CanvasEditFn>,
) -> impl IntoElement {
    let key_owned = key.to_owned();
    let canvas_owned = canvas.clone();
    Button::new(SharedString::from(format!(
        "canvas-sec-actions-{key}-{block_ix}"
    )))
    .ghost()
    .with_size(Size::XSmall)
    .icon(IconName::EllipsisVertical)
    .tooltip("Section actions")
    .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
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
            menu = append_section_edit_menu_items(
                menu,
                &key_owned,
                block_ix,
                block_count,
                is_fixture,
                &canvas_owned,
                section_id.as_deref(),
                on_edit.as_ref(),
            );
        }
        menu
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn append_section_edit_menu_items(
    mut menu: PopupMenu,
    key: &str,
    block_ix: usize,
    block_count: usize,
    is_fixture: bool,
    canvas: &CanvasDoc,
    section_id: Option<&str>,
    on_edit: Option<&CanvasEditFn>,
) -> PopupMenu {
    let sid = section_id.map(str::to_owned);
    let (c_up, k_up, e_up) = (canvas.clone(), key.to_owned(), on_edit.cloned());
    menu = menu.separator().item(
        PopupMenuItem::new("Move up")
            .disabled(block_ix == 0)
            .on_click(move |_, window, cx| {
                let mut updated = c_up.clone();
                if updated.move_block(block_ix, -1) {
                    let op = CanvasEditOp::Replace {
                        section_id: None,
                        markdown: updated.to_markdown(),
                    };
                    commit_block_edit(&k_up, updated, op, is_fixture, e_up.as_ref(), window, cx);
                }
            }),
    );

    let (c_dn, k_dn, e_dn) = (canvas.clone(), key.to_owned(), on_edit.cloned());
    menu = menu.item(
        PopupMenuItem::new("Move down")
            .disabled(block_ix + 1 >= block_count)
            .on_click(move |_, window, cx| {
                let mut updated = c_dn.clone();
                if updated.move_block(block_ix, 1) {
                    let op = CanvasEditOp::Replace {
                        section_id: None,
                        markdown: updated.to_markdown(),
                    };
                    commit_block_edit(&k_dn, updated, op, is_fixture, e_dn.as_ref(), window, cx);
                }
            }),
    );

    let (c_dup, k_dup, e_dup, sid_dup) = (
        canvas.clone(),
        key.to_owned(),
        on_edit.cloned(),
        sid.clone(),
    );
    menu = menu.item(
        PopupMenuItem::new("Duplicate section").on_click(move |_, window, cx| {
            let mut updated = c_dup.clone();
            let duplicated_md = updated.block_plain_text(block_ix).unwrap_or_default();
            if updated.duplicate_block(block_ix) {
                let full_md = updated.to_markdown();
                let op = section_insert_after_op(sid_dup.clone(), duplicated_md, full_md);
                commit_block_edit(&k_dup, updated, op, is_fixture, e_dup.as_ref(), window, cx);
            }
        }),
    );

    let (c_ins, k_ins, e_ins, sid_ins) = (
        canvas.clone(),
        key.to_owned(),
        on_edit.cloned(),
        sid.clone(),
    );
    menu = menu.item(
        PopupMenuItem::new("Insert section below").on_click(move |_, window, cx| {
            let mut updated = c_ins.clone();
            let new_text = "New section".to_owned();
            if updated.insert_block_after(block_ix, CanvasBlock::Paragraph(new_text.clone())) {
                let full_md = updated.to_markdown();
                let op = section_insert_after_op(sid_ins.clone(), new_text, full_md);
                commit_block_edit(&k_ins, updated, op, is_fixture, e_ins.as_ref(), window, cx);
            }
        }),
    );

    let (c_del, k_del, e_del, sid_del) = (canvas.clone(), key.to_owned(), on_edit.cloned(), sid);
    menu = menu
        .separator()
        .item(
            PopupMenuItem::new("Delete section").on_click(move |_, window, cx| {
                let mut updated = c_del.clone();
                if updated.delete_block(block_ix) {
                    let full_md = updated.to_markdown();
                    let op = section_delete_op(sid_del.clone(), full_md);
                    commit_block_edit(&k_del, updated, op, is_fixture, e_del.as_ref(), window, cx);
                }
            }),
        );

    menu
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_block(
    key: &str,
    block_ix: usize,
    block: &CanvasBlock,
    section_id: Option<&str>,
    canvas_url: Option<&str>,
    ui: &CanvasUiState,
    cbs: &BlockCallbacks,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    match block {
        CanvasBlock::Heading { level, text } => {
            let display_text = resolve_canvas_emoji(text);
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
                .child(heading.child(display_text))
                .child(
                    Button::new(SharedString::from(format!("canvas-anchor-{block_ix}")))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::Link)
                        .tooltip("Copy heading anchor link")
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(anchor_url.clone()));
                            crate::actions::notify(
                                "Copied section link",
                                anchor_url.clone(),
                                window,
                                cx,
                            );
                        }),
                )
        }
        CanvasBlock::Paragraph(text) => div()
            .text_sm()
            .line_height(px(22.))
            .child(resolve_canvas_emoji(text)),
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
        CanvasBlock::Quote(text) => {
            let lines: Vec<String> = text.lines().map(resolve_canvas_emoji).collect();
            v_flex()
                .gap_1()
                .pl_3()
                .py_1()
                .border_l_2()
                .border_color(theme.link)
                .bg(theme.secondary.opacity(0.18))
                .rounded_r_sm()
                .text_sm()
                .text_color(theme.muted_foreground)
                .children(lines.into_iter().map(|line| div().child(line)))
        }
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
                .child(div().flex_1().child(resolve_canvas_emoji(item)))
        })),
        CanvasBlock::NumberedList(items) => {
            v_flex()
                .gap_1()
                .pl_2()
                .children(items.iter().enumerate().map(|(ix, item)| {
                    h_flex()
                        .gap_2()
                        .items_start()
                        .text_sm()
                        .child(
                            div()
                                .flex_none()
                                .min_w(px(20.))
                                .text_color(theme.muted_foreground)
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("{}.", ix + 1)),
                        )
                        .child(div().flex_1().child(resolve_canvas_emoji(item)))
                }))
        }
        CanvasBlock::Checklist(items) => render_checklist_block(key, block_ix, items, ui, cbs, cx),
        CanvasBlock::Table { header, rows } => {
            render_table_block(key, block_ix, block, header, rows, cbs, cx)
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
                            .child(resolve_canvas_emoji(alt)),
                    )
                })
        }
        CanvasBlock::Divider => div().h(px(1.)).w_full().bg(theme.border),
    }
}

fn render_checklist_block(
    key: &str,
    block_ix: usize,
    items: &[(bool, String)],
    ui: &CanvasUiState,
    cbs: &BlockCallbacks,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let hover_bg = theme.secondary.opacity(0.4);
    let is_adding = ui.adding_checklist_block == Some(block_ix);
    let mut col = v_flex()
        .gap_1p5()
        .pl_1()
        .children(items.iter().enumerate().map(|(item_ix, (done, item))| {
            let (icon, color) = if *done {
                (IconName::SquareCheck, theme.success)
            } else {
                (IconName::Square, theme.muted_foreground)
            };
            let toggle = cbs.on_toggle_check.clone();
            let del = cbs.on_delete_check.clone();
            h_flex()
                .gap_1()
                .items_center()
                .child(
                    h_flex()
                        .id(SharedString::from(format!(
                            "canvas-chk-{key}-{block_ix}-{item_ix}"
                        )))
                        .flex_1()
                        .min_w_0()
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
                                .child(resolve_canvas_emoji(item)),
                        ),
                )
                .when_some(del, |row, del_cb| {
                    row.child(
                        Button::new(SharedString::from(format!(
                            "canvas-chk-del-{key}-{block_ix}-{item_ix}"
                        )))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::X)
                        .tooltip("Delete checklist item")
                        .on_click(move |_, window, cx| del_cb(item_ix, window, cx)),
                    )
                })
        }));

    if let Some(add_cb) = cbs.on_add_check_submit.clone() {
        if is_adding && let Some(input) = ui.checklist_add_input.clone() {
            let input_submit = input.clone();
            let k_submit = key.to_owned();
            let k_cancel = key.to_owned();
            col = col.child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .pt_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.background)
                            .px_2()
                            .py_0p5()
                            .child(Textarea::new(&input).appearance(false).bordered(false)),
                    )
                    .child(
                        Button::new(SharedString::from(format!(
                            "canvas-chk-save-{key}-{block_ix}"
                        )))
                        .primary()
                        .with_size(Size::XSmall)
                        .label("Add")
                        .on_click(move |_, window, cx| {
                            let val = input_submit.read(cx).value().trim().to_owned();
                            if !val.is_empty() {
                                add_cb(val, window, cx);
                            }
                            mutate_ui(
                                &k_submit,
                                |s| {
                                    s.adding_checklist_block = None;
                                    s.checklist_add_input = None;
                                },
                                window,
                                cx,
                            );
                        }),
                    )
                    .child(
                        Button::new(SharedString::from(format!(
                            "canvas-chk-cancel-{key}-{block_ix}"
                        )))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::X)
                        .on_click(move |_, window, cx| {
                            mutate_ui(
                                &k_cancel,
                                |s| {
                                    s.adding_checklist_block = None;
                                    s.checklist_add_input = None;
                                },
                                window,
                                cx,
                            );
                        }),
                    ),
            );
        } else {
            let k_open = key.to_owned();
            col = col.child(
                h_flex().pt_0p5().child(
                    Button::new(SharedString::from(format!(
                        "canvas-chk-add-{key}-{block_ix}"
                    )))
                    .ghost()
                    .with_size(Size::XSmall)
                    .icon(IconName::Plus)
                    .label("Add checklist item")
                    .on_click(move |_, window, cx| {
                        let input = cx.new(|cx| {
                            TextareaState::new(window, cx)
                                .placeholder("New checklist item…")
                                .auto_grow(1, 2)
                        });
                        mutate_ui(
                            &k_open,
                            |s| {
                                s.adding_checklist_block = Some(block_ix);
                                s.checklist_add_input = Some(input);
                            },
                            window,
                            cx,
                        );
                    }),
                ),
            );
        }
    }
    col
}

fn render_table_block(
    key: &str,
    block_ix: usize,
    block: &CanvasBlock,
    header: &[String],
    rows: &[Vec<String>],
    cbs: &BlockCallbacks,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let table_md = block.to_markdown();
    let hover_cell = theme.accent.opacity(0.45);
    let border = theme.border;
    let zebra = theme.secondary.opacity(0.18);
    let header_bg = theme.secondary.opacity(0.6);
    let col_count = header.len().max(1);

    let make_cell = |row_id: &str, col_ix: usize, text: &str| {
        let display = resolve_canvas_emoji(text);
        let copy_val = display.clone();
        div()
            .id(SharedString::from(format!(
                "canvas-tbl-cell-{key}-{block_ix}-{row_id}-{col_ix}"
            )))
            .flex_1()
            .min_w_0()
            .px_3()
            .py_2()
            .when(col_ix + 1 < col_count, |c| {
                c.border_r_1().border_color(border)
            })
            .truncate()
            .cursor_pointer()
            .hover(move |el| el.bg(hover_cell))
            .on_click(move |_, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_val.clone()));
                crate::actions::notify(
                    "Copied cell",
                    format!("Copied \"{copy_val}\" to clipboard."),
                    window,
                    cx,
                );
            })
            .child(display)
    };

    let action_strip = h_flex()
        .items_center()
        .justify_end()
        .gap_1()
        .px_2()
        .py_1()
        .bg(theme.secondary.opacity(0.28))
        .border_b_1()
        .border_color(border)
        .child(
            Button::new(SharedString::from(format!(
                "canvas-tbl-copy-{key}-{block_ix}"
            )))
            .ghost()
            .with_size(Size::XSmall)
            .icon(IconName::Copy)
            .label("Copy table")
            .on_click(move |_, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(table_md.clone()));
                crate::actions::notify(
                    "Copied table",
                    "Copied markdown table to clipboard.",
                    window,
                    cx,
                );
            }),
        )
        .when_some(cbs.on_table_add_row.clone(), |bar, add_row| {
            bar.child(
                Button::new(SharedString::from(format!(
                    "canvas-tbl-add-row-{key}-{block_ix}"
                )))
                .ghost()
                .with_size(Size::XSmall)
                .icon(IconName::Plus)
                .label("Row")
                .tooltip("Append a row to this table")
                .on_click(move |_, window, cx| add_row(window, cx)),
            )
        })
        .when_some(cbs.on_table_add_col.clone(), |bar, add_col| {
            bar.child(
                Button::new(SharedString::from(format!(
                    "canvas-tbl-add-col-{key}-{block_ix}"
                )))
                .ghost()
                .with_size(Size::XSmall)
                .icon(IconName::Plus)
                .label("Column")
                .tooltip("Append a column to this table")
                .on_click(move |_, window, cx| add_col(window, cx)),
            )
        });

    let header_row = h_flex()
        .bg(header_bg)
        .font_weight(FontWeight::SEMIBOLD)
        .children(
            header
                .iter()
                .enumerate()
                .map(|(col_ix, text)| make_cell("h", col_ix, text)),
        );

    v_flex()
        .w_full()
        .rounded_md()
        .border_1()
        .border_color(border)
        .overflow_hidden()
        .text_sm()
        .child(action_strip)
        .child(header_row)
        .children(rows.iter().enumerate().map(|(row_ix, row)| {
            let row_tag = format!("r{row_ix}");
            h_flex()
                .border_t_1()
                .border_color(border)
                .when(row_ix % 2 == 1, |r| r.bg(zebra))
                .children((0..col_count).map(|col_ix| {
                    let text = row.get(col_ix).map(String::as_str).unwrap_or("");
                    make_cell(&row_tag, col_ix, text)
                }))
        }))
}
