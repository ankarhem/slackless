//! The composer's frame: the formatting toolbar over the draft over Slack's action row
//! (both in `toolbar.rs`), with the completion menu or the link prompt floating above.

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::{ActiveTheme as _, Icon, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, BorderStyle, Bounds, ContentMask, Context, Corners, Div, Edges, ExternalPaths,
    Focusable as _, FontWeight, HighlightStyle, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, PaintQuad, ParentElement as _, Pixels, Point, Render, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, StyledText, Window, canvas, div, point, px,
    relative, rems,
};

use super::tokens::{self, Palette};
use super::toolbar::TOOLBAR;
use super::{BAR_TUCK, Composer, Placeholder};

impl Composer {
    /// The link prompt floating above the frame: a URL field for the selected text.
    fn link_panel(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        v_flex()
            .id("composer-link")
            .occlude()
            .absolute()
            .left(px(16.))
            .right(px(16.))
            .bottom(relative(1.))
            .p_2()
            .gap_1()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_lg()
            .text_sm()
            .child(Input::new(&self.link_input))
            .child(
                div()
                    .px_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("enter to link \u{b7} esc to cancel"),
            )
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for stale in self.stale_gif_urls.drain(..) {
            crate::emoji::evict_gif_asset(&stale, window, cx);
        }
        if let Some(placeholder) = self.pending_placeholder.take() {
            let placeholder = match placeholder {
                Placeholder::Text(text) => {
                    self.private_placeholder = None;
                    text
                }
                Placeholder::Private(name) => {
                    self.private_placeholder = Some(name);
                    SharedString::default()
                }
            };
            let input = self.input.clone();
            window.defer(cx, move |window, cx| {
                input.update(cx, |state, cx| {
                    state.set_placeholder(placeholder, window, cx);
                });
            });
        }
        // Both need the window or an update of the textarea, which a render cannot make.
        let editor_focused = self.input.focus_handle(cx).is_focused(window);
        let focused = editor_focused && window.is_window_active();
        let this = cx.entity();
        if self.is_stale {
            self.was_focused = editor_focused;
            window.defer(cx, move |window, cx| {
                this.update(cx, |composer, cx| composer.reset(window, cx));
            });
        } else if editor_focused != self.was_focused
            || self.palette.is_some_and(|drawn| drawn != Palette::of(cx))
        {
            window.defer(cx, move |_, cx| {
                this.update(cx, |composer, cx| composer.sync_focus(editor_focused, cx));
            });
        }

        // Slack's formatting bar across the top of the box: a shaded band on light themes, the
        // box's own raised surface on dark ones.
        let (surface, band) = crate::theme::composer_surfaces(cx);
        // A context bar takes the frame's top edge, and the formatting bar squares off under it.
        let context_bar = self.context_bar.clone().map(|bar| bar(window, cx));
        let has_bar = context_bar.is_some();
        let toolbar = self.show_toolbar.then(|| {
            h_flex()
                .items_center()
                .gap_0p5()
                .px_1p5()
                // Slack's band: 4px around 28px buttons.
                .py(px(5.))
                .rounded_t_lg()
                .bg(band)
                .children(
                    TOOLBAR
                        .iter()
                        .map(|slot| self.tool_button(slot, focused, cx)),
                )
        });
        let attachments = self.attachments_strip(cx);
        let upload_warning = self.upload_warning.clone().map(|msg| {
            let danger = cx.theme().danger;
            h_flex()
                .px_3()
                .py_1()
                .items_center()
                .justify_between()
                .gap_2()
                .text_xs()
                .text_color(danger)
                .child(msg)
                .child(
                    div()
                        .id("composer-upload-warn-close")
                        .cursor_pointer()
                        .child("\u{2715}")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.upload_warning = None;
                            cx.notify();
                        })),
                )
        });
        // Render spelling suggestions as a floating popover above the composer so triggering
        // spellcheck never changes the composer's layout height or jumps the transcript above.
        let spell_overlay =
            self.spell_guesses
                .clone()
                .filter(|_| focused)
                .map(|(range, word, guesses)| {
                    let theme = cx.theme();
                    let (muted, border, hover, fg, popover) = (
                        theme.muted_foreground,
                        theme.border,
                        theme.accent,
                        theme.foreground,
                        theme.popover,
                    );
                    h_flex()
                        .id("composer-spell")
                        .occlude()
                        .absolute()
                        .left(if self.edit_mode { px(0.) } else { px(16.) })
                        .right(if self.edit_mode { px(0.) } else { px(16.) })
                        .bottom(relative(1.))
                        .mb_1()
                        .px_3()
                        .py_1p5()
                        .items_center()
                        .flex_wrap()
                        .gap_1p5()
                        .rounded_lg()
                        .border_1()
                        .border_color(border)
                        .bg(popover)
                        .shadow_lg()
                        .text_xs()
                        .child(div().text_color(muted).child("Spelling:"))
                        .children(guesses.into_iter().enumerate().map(|(ix, guess)| {
                            let range_c = range.clone();
                            let guess_c = guess.clone();
                            div()
                                .id(("spell-fix", ix))
                                .px_1p5()
                                .py(px(1.))
                                .rounded_sm()
                                .border_1()
                                .border_color(border)
                                .text_color(fg)
                                .cursor_pointer()
                                .hover(|el| el.bg(hover))
                                .child(guess)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.apply_spelling_fix(range_c.clone(), &guess_c, window, cx);
                                }))
                        }))
                        .child(
                            div()
                                .id("spell-learn")
                                .px_1p5()
                                .py(px(1.))
                                .rounded_sm()
                                .text_color(muted)
                                .cursor_pointer()
                                .hover(|el| el.bg(hover).text_color(fg))
                                .child(format!("Learn \u{201C}{word}\u{201D}"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.learn_spelling_word(&word, cx);
                                })),
                        )
                        .into_any_element()
                });
        let also_send = self.also_send_row(cx);
        let actions = self.action_row(cx);
        let overlay: Option<AnyElement> = if self.link_open {
            Some(self.link_panel(cx).into_any_element())
        } else if let Some(menu) = self.menu.as_ref() {
            Some(self.menu_panel(menu, cx).into_any_element())
        } else {
            spell_overlay
        };

        let fixtures = self.fixtures.clone();
        let weak = cx.entity().downgrade();
        let draft = Textarea::new(&self.input)
            .appearance(false)
            .bordered(false)
            .token(move |context, _, cx| tokens::render(context, &fixtures, cx))
            .on_paste(move |item, window, cx| {
                weak.update(cx, |composer, cx| composer.on_paste(item, window, cx))
                    .unwrap_or(false)
            });
        // Slack's private-channel prompt, "Message 🔒 name", over the empty draft exactly where
        // the textarea's own (plain-text) placeholder would sit: past the frame's padding and
        // the editor's own (the kit's medium input padding), in the editor's text size and
        // line height.
        let muted = cx.theme().muted_foreground;
        let private_prompt = self
            .private_placeholder
            .clone()
            .filter(|_| self.editor.doc().is_empty())
            .map(|name| {
                h_flex()
                    .absolute()
                    .top(px(4.) + Size::Medium.input_py())
                    .left(px(8.) + Size::Medium.input_px())
                    .items_center()
                    .gap_1()
                    .text_sm()
                    .line_height(rems(1.25))
                    .text_color(muted)
                    .child("Message")
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(3.))
                            .child(Icon::new(IconName::Lock).size(px(12.)).text_color(muted))
                            .child(name),
                    )
            });
        let code_backdrop = self.code_block_backdrop(editor_focused, cx);
        let theme = cx.theme();
        div()
            .key_context("Composer")
            .relative()
            .w_full()
            .when(!self.edit_mode, |el| el.px_4())
            .capture_action(cx.listener(Self::on_enter))
            .capture_action(cx.listener(Self::on_up))
            .capture_action(cx.listener(Self::on_down))
            .capture_action(cx.listener(Self::on_tab))
            .capture_action(cx.listener(Self::on_escape))
            .capture_action(cx.listener(Self::on_undo))
            .capture_action(cx.listener(Self::on_redo))
            .capture_key_down(cx.listener(Self::on_key))
            .children(overlay)
            // The context bar sits on the box, outside its outline.
            .children(context_bar)
            .child(
                v_flex()
                    .w_full()
                    .rounded_lg()
                    // Slack's box sits over the context bar's lower edge: the bar runs on
                    // under the box, showing behind its rounded top corners.
                    .when(has_bar, |frame| frame.mt(px(-BAR_TUCK)))
                    .border_1()
                    // Slack's outline: a faint 10% foreground (#eaeaea on white) that darkens
                    // while the box has focus (#afafb0).
                    .border_color(theme.foreground.opacity(if focused { 0.35 } else { 0.1 }))
                    .bg(surface)
                    .when(!self.edit_mode, |frame| {
                        frame
                            .on_drop::<ExternalPaths>(cx.listener(
                                |this, paths: &ExternalPaths, _, cx| {
                                    this.attach_paths(paths.paths().to_vec(), cx);
                                },
                            ))
                            .drag_over::<ExternalPaths>(|style, _, _, cx| {
                                style.border_color(cx.theme().link)
                            })
                    })
                    .children(toolbar)
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .min_w_0()
                            .px_2()
                            .py_1()
                            .capture_any_mouse_down(cx.listener(Self::on_draft_mouse_down_capture))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(Self::on_draft_mouse_down_bubble),
                            )
                            .children(code_backdrop)
                            .child(draft)
                            .children(private_prompt),
                    )
                    .children(attachments)
                    .children(upload_warning)
                    .children(also_send)
                    .child(actions),
            )
            .when(!self.edit_mode, |el| el.child(self.notification_bar(cx)))
    }
}

impl Composer {
    /// Capture phase before `Textarea::on_mouse_down`: remember if a triple-click landed
    /// inside an inline-code pill before `Textarea::select_line` replaces the selection with
    /// the whole line.
    fn on_draft_mouse_down_capture(
        &mut self,
        event: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_code_click = None;
        if event.button != MouseButton::Left || event.click_count < 3 {
            return;
        }
        let doc = self.editor.doc();
        let state = self.input.read(cx);
        if *state.text() != doc.text() {
            return;
        }
        let hit = inline_code_inner_at_position(doc, state, event.position).or_else(|| {
            // Fallback when the previous click's word selection already sat inside an inline
            // code span on the clicked visual line.
            let prev = state.selected_range();
            if prev.is_empty() {
                return None;
            }
            tokens::inline_code_constructs(doc)
                .into_iter()
                .find(|(outer, _)| prev.start >= outer.start && prev.end <= outer.end)
                .filter(|(_, inner)| {
                    state.range_to_bounds(inner).is_some_and(|b| {
                        event.position.y >= b.top() - px(2.)
                            && event.position.y <= b.bottom() + px(2.)
                    })
                })
                .map(|(_, inner)| inner)
        });
        self.pending_code_click = hit;
    }

    /// Bubble phase immediately after `Textarea::on_mouse_down`: apply bounded selection for
    /// inline code pills and fenced code blocks (triple-click selects the entire inline code
    /// span or entire fenced code block body; double-click stays clamped inside inline code).
    fn on_draft_mouse_down_bubble(
        &mut self,
        event: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pending = self.pending_code_click.take();
        if event.click_count < 2 {
            return;
        }
        let doc = self.editor.doc();
        let (matches_doc, sel, inline_hit) = {
            let state = self.input.read(cx);
            let matches_doc = *state.text() == doc.text();
            let sel = state.selected_range();
            let inline_hit = if matches_doc && event.click_count == 2 {
                inline_code_inner_at_position(doc, state, event.position)
            } else {
                None
            };
            (matches_doc, sel, inline_hit)
        };
        if !matches_doc {
            return;
        }
        let target = if event.click_count >= 3 {
            pending.or_else(|| tokens::fenced_code_block_body_at(doc, sel.start))
        } else {
            inline_hit.and_then(|inner| tokens::clamp_selection_to_inner(sel, &inner))
        };
        if let Some(range) = target {
            self.input.update(cx, |state, cx| {
                state.set_selected_range(range.clone(), cx);
            });
            self.editor.select(range);
            self.sync_presentation(cx);
            self.changed(cx);
        }
    }

    /// Paint rounded Slack code-block containers and inline-code pills behind the transparent
    /// `Textarea`. Because GPUI runs `prepaint` across the whole tree before any `paint`, the
    /// sibling textarea has already stored this frame's `last_layout`/`last_bounds` by the time
    /// this canvas paints underneath the text, selection, and caret.
    fn code_block_backdrop(&self, editor_focused: bool, cx: &App) -> Option<AnyElement> {
        let doc = self.editor.doc();
        let selection = self.editor.selection();
        let blocks = tokens::code_blocks(doc, &selection, editor_focused);
        let inline_spans = tokens::inline_code_spans(doc, &selection, editor_focused);
        if blocks.is_empty() && inline_spans.is_empty() {
            return None;
        }
        let text = doc.text().to_owned();
        let input = self.input.clone();
        let code_bg = crate::theme::code_block_background(cx);
        let code_border = cx.theme().foreground.opacity(0.16);
        Some(
            canvas(
                |_, _, _| {},
                move |canvas_bounds, _, window, cx| {
                    let state = input.read(cx);
                    let Some(text_bounds) = state.text_bounds() else {
                        return;
                    };
                    let input_bounds = state.input_bounds();
                    // Outset 8px into the textarea's 20px side padding so code text sits
                    // cleanly 8px inside the rounded border (matching sent `pre` blocks).
                    let outset_x = px(8.);
                    let box_left =
                        (text_bounds.left() - outset_x).max(canvas_bounds.left() + px(4.));
                    let box_right =
                        (text_bounds.right() + outset_x).min(canvas_bounds.right() - px(4.));
                    if box_right <= box_left {
                        return;
                    }
                    let clip_top = input_bounds.top().max(canvas_bounds.top());
                    let clip_bottom = input_bounds.bottom().min(canvas_bounds.bottom());
                    if clip_bottom <= clip_top {
                        return;
                    }
                    let clip = Bounds::from_corners(
                        point(box_left - px(1.), clip_top),
                        point(box_right + px(1.), clip_bottom),
                    );
                    let pad_y = px(8.);
                    let active_pad_y = px(2.);

                    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                        for block in &blocks {
                            let open_b = state.range_to_bounds(&block.open_line);
                            let close_b = block
                                .close_line
                                .as_ref()
                                .and_then(|r| state.range_to_bounds(r));
                            let first_body_b = block
                                .body_lines
                                .iter()
                                .find_map(|r| state.range_to_bounds(r));
                            let last_body_b = block
                                .body_lines
                                .iter()
                                .rev()
                                .find_map(|r| state.range_to_bounds(r));

                            if open_b.is_none() && close_b.is_none() && first_body_b.is_none() {
                                continue;
                            }

                            let top_y = match open_b {
                                Some(b) if block.open_collapsed_empty => {
                                    (b.bottom() - pad_y).max(b.top())
                                }
                                Some(b) => b.top() - active_pad_y,
                                None => clip.top() - pad_y,
                            };

                            let bottom_y = if block.close_line.is_some() {
                                match close_b {
                                    Some(b) if block.close_collapsed => {
                                        (b.top() + pad_y).min(b.bottom())
                                    }
                                    Some(b) => b.bottom() + active_pad_y,
                                    None => clip.bottom() + pad_y,
                                }
                            } else {
                                match last_body_b.or(open_b) {
                                    Some(b) => b.bottom() + pad_y,
                                    None => clip.bottom() + pad_y,
                                }
                            };

                            let bottom_y = bottom_y.max(top_y + px(16.));
                            window.paint_quad(PaintQuad {
                                bounds: Bounds::from_corners(
                                    point(box_left, top_y),
                                    point(box_right, bottom_y),
                                ),
                                corner_radii: Corners::all(px(4.)),
                                background: code_bg.into(),
                                border_widths: Edges::all(px(1.)),
                                border_color: code_border,
                                border_style: BorderStyle::default(),
                            });
                        }

                        // Inline `` `code` `` pills: rounded 1px bordered boxes matching sent
                        // `.c-mrkdwn__code` spans, split per visual line if a span wraps.
                        for span in &inline_spans {
                            for quad in inline_code_quads(state, &text, span) {
                                let pill = Bounds::from_corners(
                                    point(quad.left() - px(2.), quad.top() + px(1.)),
                                    point(quad.right() + px(2.), quad.bottom() - px(1.)),
                                );
                                window.paint_quad(PaintQuad {
                                    bounds: pill,
                                    corner_radii: Corners::all(px(3.)),
                                    background: code_bg.into(),
                                    border_widths: Edges::all(px(1.)),
                                    border_color: code_border,
                                    border_style: BorderStyle::default(),
                                });
                            }
                        }
                    });
                },
            )
            .absolute()
            .inset_0()
            .into_any_element(),
        )
    }

    /// Slack's notification bar under the box (`.p-notification_bar`): a 24px strip that
    /// says who is typing, in 0.7em muted text with the names in bold.
    fn notification_bar(&self, cx: &App) -> Div {
        let bar = h_flex()
            .h(px(24.))
            .pl(px(12.))
            .pr(px(8.))
            .items_center()
            .overflow_hidden();
        let Some(typing) = &self.typing else {
            return bar;
        };
        let (text, names) = typing.line();
        let bold = HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        bar.child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(10.5))
                .line_height(px(16.))
                .text_color(cx.theme().muted_foreground)
                .child(StyledText::new(text).with_highlights(names.into_iter().map(|r| (r, bold)))),
        )
    }
}

/// Visual-line bounding boxes for an inline code range inside `TextareaState`. Fast-paths
/// single-line spans; splits wrapped spans per visual line so pills never stretch backwards.
fn inline_code_quads(
    state: &gpui_kit::component::input::TextareaState,
    text: &str,
    range: &std::ops::Range<usize>,
) -> Vec<Bounds<Pixels>> {
    let Some(start_b) = state.range_to_bounds(&(range.start..range.start)) else {
        return Vec::new();
    };
    let Some(end_b) = state.range_to_bounds(&(range.end..range.end)) else {
        return Vec::new();
    };
    if start_b.top() == end_b.top() {
        if end_b.left() > start_b.left() {
            return vec![Bounds::from_corners(
                start_b.origin,
                point(end_b.left(), start_b.bottom()),
            )];
        }
        return Vec::new();
    }
    let mut quads: Vec<Bounds<Pixels>> = Vec::new();
    let mut current: Option<Bounds<Pixels>> = None;
    let slice = match text.get(range.clone()) {
        Some(s) => s,
        None => return Vec::new(),
    };
    for (rel, ch) in slice.char_indices() {
        let start = range.start + rel;
        let end = start + ch.len_utf8();
        let Some(b) = state.range_to_bounds(&(start..end)) else {
            continue;
        };
        if b.size.width <= px(0.) {
            continue;
        }
        match &mut current {
            Some(acc) if (acc.top() - b.top()).abs() < px(1.) => {
                let left = acc.left().min(b.left());
                let right = acc.right().max(b.right());
                *acc = Bounds::from_corners(point(left, acc.top()), point(right, acc.bottom()));
            }
            _ => {
                if let Some(prev) = current.replace(b) {
                    quads.push(prev);
                }
            }
        }
    }
    if let Some(last) = current {
        quads.push(last);
    }
    quads
}

/// If `pos` falls inside any painted inline-code pill in `state`, return that construct's
/// `inner` byte range (excluding backticks).
fn inline_code_inner_at_position(
    doc: &crate::editor::Doc,
    state: &gpui_kit::component::input::TextareaState,
    pos: Point<Pixels>,
) -> Option<std::ops::Range<usize>> {
    let text = doc.text();
    for (outer, inner) in tokens::inline_code_constructs(doc) {
        let hit_quads = |range: &std::ops::Range<usize>| {
            inline_code_quads(state, text, range).into_iter().any(|q| {
                let pill = Bounds::from_corners(
                    point(q.left() - px(2.), q.top()),
                    point(q.right() + px(2.), q.bottom()),
                );
                pill.contains(&pos)
            })
        };
        if hit_quads(&outer) || hit_quads(&inner) {
            return Some(inner);
        }
    }
    None
}
