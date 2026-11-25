//! Per-block rendering (headings, multi-line quotes, bullets, numbered lists, interactive
//! checklists, rich tables, images, inline audio/video embeds, user profile cards, embedded
//! canvases & lists, emoji), drag-to-reorder handles, and per-section actions menu
//! (`F-COLLAB-134..136`, `F-COLLAB-139`, `F-COLLAB-141`, `F-COLLAB-144..145`,
//! `F-COLLAB-147..148`).

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, App, AppContext as _, ClipboardItem, Context, Div, FontWeight, InteractiveElement as _,
    IntoElement, ObjectFit, ParentElement as _, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, img, px,
};
use slackless_core::types::UserId;

use crate::canvas::mentions::render_inline_canvas_text;
use crate::canvas::model::{
    CanvasBlock, CanvasDoc, CanvasEmbedKind, MediaKind, resolve_profile_card,
};
use crate::canvas::parse::{media_format_pill, resolve_canvas_emoji};
use crate::canvas::{
    CanvasActions, CanvasChannelLookupFn, CanvasEditFn, CanvasOpenChannelFn, CanvasOpenEmbedFn,
    CanvasOpenMediaFn, CanvasOpenProfileFn, CanvasUiState, CanvasUserLookupFn, commit_block_edit,
    mutate_ui, section_delete_op, section_insert_after_op, section_replace_op,
};

/// Drag payload and floating preview chip for drag-to-reorder canvas blocks (`F-COLLAB-144`).
#[derive(Clone, Debug)]
pub(super) struct CanvasBlockDrag {
    pub(super) key: String,
    pub(super) from_ix: usize,
    pub(super) label: String,
    pub(super) position: Point<Pixels>,
}

impl Render for CanvasBlockDrag {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .pl(self.position.x + px(10.))
            .pt(self.position.y - px(14.))
            .child(
                h_flex()
                    .h(px(28.))
                    .max_w(px(280.))
                    .px_2p5()
                    .gap_1p5()
                    .items_center()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.ring)
                    .bg(theme.background.opacity(0.94))
                    .shadow_md()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .child(
                        Icon::new(IconName::ArrowUpDown)
                            .size(px(12.))
                            .text_color(theme.link),
                    )
                    .child(div().truncate().child(self.label.clone())),
            )
    }
}

/// Compact left-gutter drag handle for reordering a canvas block (`F-COLLAB-144`).
pub(super) fn render_drag_handle(
    key: &str,
    block_ix: usize,
    block: &CanvasBlock,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let hover_bg = theme.secondary.opacity(0.55);
    let fg = theme.foreground;
    let drag = CanvasBlockDrag {
        key: key.to_owned(),
        from_ix: block_ix,
        label: block.preview_label(),
        position: Point::default(),
    };
    div()
        .id(SharedString::from(format!(
            "canvas-drag-handle-{key}-{block_ix}"
        )))
        .flex_none()
        .mt_0p5()
        .px_0p5()
        .py_1()
        .rounded_sm()
        .cursor_grab()
        .text_color(theme.muted_foreground.opacity(0.65))
        .hover(move |s| s.bg(hover_bg).text_color(fg))
        .on_drag(drag, |drag: &CanvasBlockDrag, position, _, cx| {
            cx.new(|_| CanvasBlockDrag {
                position,
                ..drag.clone()
            })
        })
        .child(Icon::new(IconName::ArrowUpDown).size(px(12.)))
}

type ChecklistItemFn = Rc<dyn Fn(usize, &mut Window, &mut App)>;
type ChecklistAddFn = Rc<dyn Fn(String, &mut Window, &mut App)>;
type TableMutateFn = Rc<dyn Fn(&mut Window, &mut App)>;
type ImageAltSetFn = Rc<dyn Fn(String, &mut Window, &mut App)>;

#[derive(Clone, Default)]
pub(super) struct BlockCallbacks {
    pub(super) on_toggle_check: Option<ChecklistItemFn>,
    pub(super) on_delete_check: Option<ChecklistItemFn>,
    pub(super) on_add_check_submit: Option<ChecklistAddFn>,
    pub(super) on_table_add_row: Option<TableMutateFn>,
    pub(super) on_table_add_col: Option<TableMutateFn>,
    pub(super) on_set_image_alt: Option<ImageAltSetFn>,
    pub(super) on_open_media: Option<CanvasOpenMediaFn>,
    pub(super) user_lookup: Option<CanvasUserLookupFn>,
    pub(super) channel_lookup: Option<CanvasChannelLookupFn>,
    pub(super) on_open_profile: Option<CanvasOpenProfileFn>,
    pub(super) on_open_channel: Option<CanvasOpenChannelFn>,
    pub(super) on_open_embed: Option<CanvasOpenEmbedFn>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_block_callbacks(
    key: &str,
    canvas: &CanvasDoc,
    block_ix: usize,
    section_id: Option<String>,
    can_edit: bool,
    is_fixture: bool,
    actions: &CanvasActions,
) -> BlockCallbacks {
    let mut cbs = BlockCallbacks {
        on_open_media: actions.on_open_media.clone(),
        user_lookup: actions.user_lookup.clone(),
        channel_lookup: actions.channel_lookup.clone(),
        on_open_profile: actions.on_open_profile.clone(),
        on_open_channel: actions.on_open_channel.clone(),
        on_open_embed: actions.on_open_embed.clone(),
        ..Default::default()
    };
    if !can_edit {
        return cbs;
    }

    let on_edit = actions.on_edit.clone();

    cbs.on_toggle_check = {
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

    cbs.on_delete_check = {
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

    cbs.on_add_check_submit = {
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

    cbs.on_table_add_row = {
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

    cbs.on_table_add_col = {
        let k = key.to_owned();
        let doc = canvas.clone();
        let sid = section_id.clone();
        let cb = on_edit.clone();
        Some(Rc::new(move |window: &mut Window, cx: &mut App| {
            let mut updated = doc.clone();
            if let Some(section_md) = updated.table_add_column(block_ix) {
                let full_md = updated.to_markdown();
                let op = section_replace_op(sid.clone(), section_md, full_md);
                commit_block_edit(&k, updated, op, is_fixture, cb.as_ref(), window, cx);
            }
        }) as Rc<dyn Fn(&mut Window, &mut App)>)
    };

    cbs.on_set_image_alt = {
        let k = key.to_owned();
        let doc = canvas.clone();
        let sid = section_id;
        let cb = on_edit;
        Some(
            Rc::new(move |alt: String, window: &mut Window, cx: &mut App| {
                let mut updated = doc.clone();
                if let Some(section_md) = updated.set_image_alt(block_ix, alt) {
                    let full_md = updated.to_markdown();
                    let op = section_replace_op(sid.clone(), section_md, full_md);
                    commit_block_edit(&k, updated, op, is_fixture, cb.as_ref(), window, cx);
                }
            }) as Rc<dyn Fn(String, &mut Window, &mut App)>,
        )
    };

    cbs
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
                if block_ix > 0 {
                    let mut updated = c_up.clone();
                    if let Some(op) = updated.reorder_block_op(block_ix, block_ix - 1) {
                        commit_block_edit(
                            &k_up,
                            updated,
                            op,
                            is_fixture,
                            e_up.as_ref(),
                            window,
                            cx,
                        );
                    }
                }
            }),
    );

    let (c_dn, k_dn, e_dn) = (canvas.clone(), key.to_owned(), on_edit.cloned());
    menu = menu.item(
        PopupMenuItem::new("Move down")
            .disabled(block_ix + 1 >= block_count)
            .on_click(move |_, window, cx| {
                let mut updated = c_dn.clone();
                if let Some(op) = updated.reorder_block_op(block_ix, block_ix + 1) {
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

/// Inline keyboard-accessible Section Actions bar opened by `Cmd+Shift+.` or `.` (`F-COLLAB-196`).
#[allow(clippy::too_many_arguments)]
pub(super) fn render_inline_section_actions_bar(
    key: &str,
    block_ix: usize,
    block_count: usize,
    can_edit: bool,
    is_fixture: bool,
    canvas: &CanvasDoc,
    section_id: Option<String>,
    block_md: String,
    on_edit: Option<CanvasEditFn>,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let k_close = key.to_owned();
    let copy_md = block_md;

    let mut bar = h_flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .mt_1()
        .rounded_md()
        .border_1()
        .border_color(theme.ring)
        .bg(theme.secondary.opacity(0.45))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child("Section actions:"),
        )
        .child(
            Button::new(SharedString::from(format!(
                "canvas-kb-sec-copy-{key}-{block_ix}"
            )))
            .ghost()
            .with_size(Size::XSmall)
            .icon(IconName::Copy)
            .label("Copy")
            .on_click(move |_, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_md.clone()));
                crate::actions::notify(
                    "Copied section",
                    "Copied canvas section markdown to clipboard.",
                    window,
                    cx,
                );
            }),
        );

    if can_edit {
        let (c_up, k_up, e_up) = (canvas.clone(), key.to_owned(), on_edit.clone());
        let (c_dn, k_dn, e_dn) = (canvas.clone(), key.to_owned(), on_edit.clone());
        let (c_dup, k_dup, e_dup, sid_dup) = (
            canvas.clone(),
            key.to_owned(),
            on_edit.clone(),
            section_id.clone(),
        );
        let (c_del, k_del, e_del, sid_del) = (canvas.clone(), key.to_owned(), on_edit, section_id);

        bar = bar
            .when(block_ix > 0, |row| {
                row.child(
                    Button::new(SharedString::from(format!(
                        "canvas-kb-sec-up-{key}-{block_ix}"
                    )))
                    .ghost()
                    .with_size(Size::XSmall)
                    .icon(IconName::ArrowUp)
                    .label("Move up")
                    .on_click(move |_, window, cx| {
                        let mut updated = c_up.clone();
                        if let Some(op) = updated.reorder_block_op(block_ix, block_ix - 1) {
                            commit_block_edit(
                                &k_up,
                                updated,
                                op,
                                is_fixture,
                                e_up.as_ref(),
                                window,
                                cx,
                            );
                        }
                    }),
                )
            })
            .when(block_ix + 1 < block_count, |row| {
                row.child(
                    Button::new(SharedString::from(format!(
                        "canvas-kb-sec-dn-{key}-{block_ix}"
                    )))
                    .ghost()
                    .with_size(Size::XSmall)
                    .icon(IconName::ArrowDown)
                    .label("Move down")
                    .on_click(move |_, window, cx| {
                        let mut updated = c_dn.clone();
                        if let Some(op) = updated.reorder_block_op(block_ix, block_ix + 1) {
                            commit_block_edit(
                                &k_dn,
                                updated,
                                op,
                                is_fixture,
                                e_dn.as_ref(),
                                window,
                                cx,
                            );
                        }
                    }),
                )
            })
            .child(
                Button::new(SharedString::from(format!(
                    "canvas-kb-sec-dup-{key}-{block_ix}"
                )))
                .ghost()
                .with_size(Size::XSmall)
                .label("Duplicate")
                .on_click(move |_, window, cx| {
                    let mut updated = c_dup.clone();
                    let duplicated_md = updated.block_plain_text(block_ix).unwrap_or_default();
                    if updated.duplicate_block(block_ix) {
                        let full_md = updated.to_markdown();
                        let op = section_insert_after_op(sid_dup.clone(), duplicated_md, full_md);
                        commit_block_edit(
                            &k_dup,
                            updated,
                            op,
                            is_fixture,
                            e_dup.as_ref(),
                            window,
                            cx,
                        );
                    }
                }),
            )
            .child(
                Button::new(SharedString::from(format!(
                    "canvas-kb-sec-del-{key}-{block_ix}"
                )))
                .ghost()
                .with_size(Size::XSmall)
                .icon(IconName::Trash)
                .label("Delete")
                .on_click(move |_, window, cx| {
                    let mut updated = c_del.clone();
                    if updated.delete_block(block_ix) {
                        let full_md = updated.to_markdown();
                        let op = section_delete_op(sid_del.clone(), full_md);
                        commit_block_edit(
                            &k_del,
                            updated,
                            op,
                            is_fixture,
                            e_del.as_ref(),
                            window,
                            cx,
                        );
                        mutate_ui(&k_del, |s| s.keyboard.section_menu_block = None, window, cx);
                    }
                }),
            );
    }

    bar.child(
        Button::new(SharedString::from(format!(
            "canvas-kb-sec-close-{key}-{block_ix}"
        )))
        .ghost()
        .with_size(Size::XSmall)
        .icon(IconName::X)
        .on_click(move |_, window, cx| {
            mutate_ui(
                &k_close,
                |s| s.keyboard.section_menu_block = None,
                window,
                cx,
            );
        }),
    )
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
                .child(heading.child(render_inline_canvas_text(
                    &format!("canvas-h-{key}-{block_ix}"),
                    text,
                    cbs,
                    cx,
                )))
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
        CanvasBlock::Paragraph(text) => {
            div()
                .text_sm()
                .line_height(px(22.))
                .child(render_inline_canvas_text(
                    &format!("canvas-p-{key}-{block_ix}"),
                    text,
                    cbs,
                    cx,
                ))
        }
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
        CanvasBlock::Quote(text) => v_flex()
            .gap_1()
            .pl_3()
            .py_1()
            .border_l_2()
            .border_color(theme.link)
            .bg(theme.secondary.opacity(0.18))
            .rounded_r_sm()
            .text_sm()
            .text_color(theme.muted_foreground)
            .children(text.lines().enumerate().map(|(line_ix, line)| {
                render_inline_canvas_text(
                    &format!("canvas-q-{key}-{block_ix}-{line_ix}"),
                    line,
                    cbs,
                    cx,
                )
            })),
        CanvasBlock::Bullets(items) => {
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
                                .text_color(theme.muted_foreground)
                                .child("•"),
                        )
                        .child(div().flex_1().child(render_inline_canvas_text(
                            &format!("canvas-b-{key}-{block_ix}-{ix}"),
                            item,
                            cbs,
                            cx,
                        )))
                }))
        }
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
                        .child(div().flex_1().child(render_inline_canvas_text(
                            &format!("canvas-n-{key}-{block_ix}-{ix}"),
                            item,
                            cbs,
                            cx,
                        )))
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
            let is_editing_alt = ui.editing_image_block == Some(block_ix);
            let mut col = v_flex().gap_1().child(
                div()
                    .w(px(box_w))
                    .h(px(box_h))
                    .rounded_md()
                    .bg(theme.muted.opacity(0.3))
                    .border_1()
                    .border_color(theme.border)
                    .overflow_hidden()
                    .child(img(url.clone()).size_full().object_fit(ObjectFit::Contain)),
            );

            if is_editing_alt
                && let Some(input) = ui.image_alt_input.clone()
                && let Some(on_set_alt) = cbs.on_set_image_alt.clone()
            {
                let input_save = input.clone();
                let k_save = key.to_owned();
                let k_cancel = key.to_owned();
                col = col.child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .pt_0p5()
                        .child(
                            crate::preferences::text_input_box(&input, false, cx)
                                .flex_1()
                                .min_w_0()
                                .bg(theme.background),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "canvas-img-alt-save-{key}-{block_ix}"
                            )))
                            .primary()
                            .with_size(Size::XSmall)
                            .label("Save")
                            .on_click(move |_, window, cx| {
                                let val = input_save.read(cx).value().trim().to_owned();
                                on_set_alt(val, window, cx);
                                mutate_ui(
                                    &k_save,
                                    |s| {
                                        s.editing_image_block = None;
                                        s.image_alt_input = None;
                                    },
                                    window,
                                    cx,
                                );
                            }),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "canvas-img-alt-cancel-{key}-{block_ix}"
                            )))
                            .ghost()
                            .with_size(Size::XSmall)
                            .icon(IconName::X)
                            .on_click(move |_, window, cx| {
                                mutate_ui(
                                    &k_cancel,
                                    |s| {
                                        s.editing_image_block = None;
                                        s.image_alt_input = None;
                                    },
                                    window,
                                    cx,
                                );
                            }),
                        ),
                );
            } else {
                let edit_alt_btn = cbs.on_set_image_alt.as_ref().map(|_| {
                    let k_open = key.to_owned();
                    let cur_alt = alt.clone();
                    Button::new(SharedString::from(format!(
                        "canvas-img-alt-edit-{key}-{block_ix}"
                    )))
                    .ghost()
                    .with_size(Size::XSmall)
                    .icon(IconName::Pencil)
                    .label(if alt.is_empty() {
                        "Add description"
                    } else {
                        "Edit description"
                    })
                    .tooltip("Edit accessible image description (alt text)")
                    .on_click(move |_, window, cx| {
                        let initial = cur_alt.clone();
                        let input = cx.new(|cx| {
                            TextareaState::new(window, cx)
                                .default_value(initial)
                                .placeholder("Describe this image for screen readers…")
                                .auto_grow(1, 3)
                        });
                        mutate_ui(
                            &k_open,
                            |s| {
                                s.editing_image_block = Some(block_ix);
                                s.image_alt_input = Some(input);
                            },
                            window,
                            cx,
                        );
                    })
                });
                if !alt.is_empty() || edit_alt_btn.is_some() {
                    col = col.child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .when(!alt.is_empty(), |row| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(resolve_canvas_emoji(alt)),
                                )
                            })
                            .children(edit_alt_btn),
                    );
                }
            }
            col
        }
        CanvasBlock::MediaEmbed {
            kind,
            title,
            url,
            file_id,
            duration,
            mime,
        } => render_media_embed_block(
            key,
            block_ix,
            *kind,
            title,
            url,
            file_id.as_deref(),
            duration.as_deref(),
            mime.as_deref(),
            cbs,
            cx,
        ),
        CanvasBlock::ProfileCard {
            user_id,
            display_name,
            title,
            handle,
        } => render_profile_card_block(
            key,
            block_ix,
            user_id,
            display_name.as_deref(),
            title.as_deref(),
            handle.as_deref(),
            cbs,
            cx,
        ),
        CanvasBlock::DocEmbed {
            kind,
            file_id,
            title,
            url,
        } => render_doc_embed_block(key, block_ix, *kind, file_id, title, url, cbs, cx),
        CanvasBlock::Divider => div().h(px(1.)).w_full().bg(theme.border),
    }
}

#[allow(clippy::too_many_arguments)]
fn render_media_embed_block(
    key: &str,
    block_ix: usize,
    kind: MediaKind,
    title: &str,
    url: &str,
    file_id: Option<&str>,
    duration: Option<&str>,
    mime: Option<&str>,
    cbs: &BlockCallbacks,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let icon = match kind {
        MediaKind::Audio => IconName::Headphones,
        MediaKind::Video => IconName::Video,
    };
    let format_pill = media_format_pill(mime, url);
    let url_open = url.to_owned();
    let on_open_media = cbs.on_open_media.clone();

    let mut pills = h_flex()
        .flex_wrap()
        .items_center()
        .gap_1p5()
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(
            div()
                .px_1p5()
                .py_0p5()
                .rounded_sm()
                .bg(theme.secondary.opacity(0.7))
                .text_color(theme.foreground)
                .font_weight(FontWeight::MEDIUM)
                .child(kind.label()),
        );
    if let Some(fmt) = format_pill {
        pills = pills.child(
            div()
                .px_1p5()
                .py_0p5()
                .rounded_sm()
                .border_1()
                .border_color(theme.border)
                .child(fmt),
        );
    }
    if let Some(dur) = duration {
        pills = pills.child(div().child(dur.to_owned()));
    }
    if let Some(fid) = file_id {
        pills = pills.child(div().child(fid.to_owned()));
    }

    h_flex()
        .w_full()
        .items_center()
        .gap_3()
        .p_2p5()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary.opacity(0.28))
        .child(
            h_flex()
                .flex_none()
                .w(px(36.))
                .h(px(36.))
                .items_center()
                .justify_center()
                .rounded_md()
                .bg(theme.link.opacity(0.14))
                .child(Icon::new(icon).size(px(18.)).text_color(theme.link)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .truncate()
                        .child(resolve_canvas_emoji(title)),
                )
                .child(pills),
        )
        .child(
            Button::new(SharedString::from(format!(
                "canvas-media-open-{key}-{block_ix}"
            )))
            .outline()
            .with_size(Size::XSmall)
            .icon(IconName::Play)
            .label("Play")
            .tooltip(format!("Open {} media", kind.label().to_ascii_lowercase()))
            .on_click(move |_, window, cx| {
                if let Some(cb) = &on_open_media {
                    cb(url_open.clone(), kind, window, cx);
                } else if url_open.starts_with("https://") || url_open.starts_with("http://") {
                    cx.open_url(&url_open);
                }
            }),
        )
}

#[allow(clippy::too_many_arguments)]
fn render_profile_card_block(
    key: &str,
    block_ix: usize,
    user_id: &str,
    display_name: Option<&str>,
    title: Option<&str>,
    handle: Option<&str>,
    cbs: &BlockCallbacks,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let profile = resolve_profile_card(
        user_id,
        display_name,
        title,
        handle,
        cbs.user_lookup.as_ref(),
    );
    let initials = profile.initials();
    let uid_click = UserId::from(profile.user_id.as_str());
    let on_open_profile = cbs.on_open_profile.clone();

    let name_row = h_flex()
        .items_center()
        .gap_1p5()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .truncate()
                .child(resolve_canvas_emoji(&profile.display_name)),
        )
        .when_some(profile.handle.clone(), |row, h| {
            row.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("@{h}")),
            )
        });

    let subtitle = profile
        .title
        .clone()
        .unwrap_or_else(|| profile.user_id.clone());

    h_flex()
        .w_full()
        .items_center()
        .gap_3()
        .p_2p5()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary.opacity(0.24))
        .child(
            h_flex()
                .flex_none()
                .w(px(36.))
                .h(px(36.))
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(theme.link.opacity(0.18))
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.link)
                .child(initials),
        )
        .child(
            v_flex().flex_1().min_w_0().gap_0p5().child(name_row).child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .truncate()
                    .child(resolve_canvas_emoji(&subtitle)),
            ),
        )
        .when_some(on_open_profile, |row, cb| {
            let uid = uid_click.clone();
            row.child(
                Button::new(SharedString::from(format!(
                    "canvas-profile-open-{key}-{block_ix}"
                )))
                .outline()
                .with_size(Size::XSmall)
                .icon(IconName::UserRound)
                .label("View profile")
                .tooltip("Open user profile")
                .on_click(move |_, window, cx| cb(uid.clone(), window, cx)),
            )
        })
}

#[allow(clippy::too_many_arguments)]
fn render_doc_embed_block(
    key: &str,
    block_ix: usize,
    kind: CanvasEmbedKind,
    file_id: &str,
    title: &str,
    url: &str,
    cbs: &BlockCallbacks,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let icon = match kind {
        CanvasEmbedKind::Canvas => IconName::LayoutTemplate,
        CanvasEmbedKind::List => IconName::ListChecks,
    };
    let on_open_embed = cbs.on_open_embed.clone();
    let fid_open = file_id.to_owned();
    let url_open = url.to_owned();

    h_flex()
        .w_full()
        .items_center()
        .gap_3()
        .p_2p5()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary.opacity(0.28))
        .child(
            h_flex()
                .flex_none()
                .w(px(36.))
                .h(px(36.))
                .items_center()
                .justify_center()
                .rounded_md()
                .bg(theme.link.opacity(0.14))
                .child(Icon::new(icon).size(px(18.)).text_color(theme.link)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .truncate()
                        .child(resolve_canvas_emoji(title)),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(
                            div()
                                .px_1p5()
                                .py_0p5()
                                .rounded_sm()
                                .bg(theme.secondary.opacity(0.7))
                                .text_color(theme.foreground)
                                .font_weight(FontWeight::MEDIUM)
                                .child(kind.label()),
                        )
                        .child(div().truncate().child(file_id.to_owned())),
                ),
        )
        .child(
            Button::new(SharedString::from(format!(
                "canvas-embed-open-{key}-{block_ix}"
            )))
            .outline()
            .with_size(Size::XSmall)
            .icon(IconName::ExternalLink)
            .label("Open")
            .tooltip(format!(
                "Open embedded {}",
                kind.label().to_ascii_lowercase()
            ))
            .on_click(move |_, window, cx| {
                if let Some(cb) = &on_open_embed {
                    cb(kind, fid_open.clone(), url_open.clone(), window, cx);
                } else if url_open.starts_with("https://") || url_open.starts_with("http://") {
                    cx.open_url(&url_open);
                }
            }),
        )
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
                                .child(render_inline_canvas_text(
                                    &format!("canvas-c-{key}-{block_ix}-{item_ix}"),
                                    item,
                                    cbs,
                                    cx,
                                )),
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
                        crate::preferences::text_input_box(&input, false, cx)
                            .flex_1()
                            .min_w_0()
                            .bg(theme.background),
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
            .child(render_inline_canvas_text(
                &format!("canvas-t-{key}-{block_ix}-{row_id}-{col_ix}"),
                text,
                cbs,
                cx,
            ))
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
