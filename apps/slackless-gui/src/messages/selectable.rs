//! Selectable message paragraphs participating in the window text-selection layer.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::base::{
    GlobalState, TextSelectionHandle, TextSelectionRegistration, TextSelectionRun,
};
use gpui_kit::gpui::{
    App, BorderStyle, Bounds, Corners, CursorStyle, DispatchPhase, Edges, Element, ElementId,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ScrollEvent,
    SharedString, StyledText, Subscription, TextLayout, TextRun, TransformationMatrix, Window, px,
    size, transparent_black,
};

use super::render::RunTheme;
use super::{APP_ICON, APP_ICON_SLOT, LinkHandler, LinkTarget, route_link};
use crate::hover_card::{self, Trigger};

/// One paragraph of message-body text that participates in the window text selection: a
/// [`StyledText`] with styled runs whose link ranges stay clickable (see
/// gpui-base's `SelectableText` for the plain-run original, and its `PlainSelectableText`
/// test element for this registration pattern).
pub(super) struct SelectableParagraph {
    pub(super) id: ElementId,
    pub(super) text: SharedString,
    pub(super) styled: StyledText,
    pub(super) links: Vec<(Range<usize>, LinkTarget)>,
    pub(super) handler: Option<LinkHandler>,
    pub(super) document_order: u64,
    pub(super) selection: Hsla,
    pub(super) link: Hsla,
    /// Inline code spans and their fill color, rendered as a single rounded bordered pill
    /// matching Slack's `.c-mrkdwn__code` (kept out of `washes` so fill and border share exact
    /// geometry with no top/bottom overflow or side gap).
    pub(super) code: Vec<(Range<usize>, Hsla)>,
    /// True when this paragraph is the body of a fenced code block (```` ```...``` ````), so
    /// triple-clicking selects the entire code block across all lines.
    pub(super) is_code_block: bool,
    /// Non-code run backgrounds (mentions, app chips), painted under the selection: as
    /// run backgrounds GPUI paints them with the text, over the highlight.
    pub(super) washes: Vec<(Range<usize>, Hsla)>,
    pub(super) code_border: Hsla,
    /// App chips' icon slots ([`APP_ICON_SLOT`]): text offsets the icons are painted at.
    pub(super) icons: Vec<usize>,
}

/// What one paragraph keeps between frames: its selection handle (and the subscription
/// that redraws the window when a drag moves its selection), any bounded local selection
/// (multi-click inside inline code or a fenced code block), the cached projected text for
/// window drags, the link it hovers, the link a press started in, and its key for
/// member-hover-card reports.
pub(super) struct ParagraphState {
    pub(super) handle: TextSelectionHandle,
    pub(super) local_range: Rc<RefCell<Option<Range<usize>>>>,
    pub(super) projected: Rc<RefCell<String>>,
    pub(super) hovered_link: Rc<Cell<Option<usize>>>,
    pub(super) mouse_down: Rc<Cell<Option<usize>>>,
    /// This paragraph's key in hover-card trigger reports (stable across frames).
    pub(super) hover_key: SharedString,
    pub(super) _refresh: Subscription,
}

impl SelectableParagraph {
    pub(super) fn new(
        id: ElementId,
        text: String,
        mut runs: Vec<TextRun>,
        links: Vec<(Range<usize>, LinkTarget)>,
        document_order: u64,
        theme: &RunTheme,
    ) -> Self {
        let text: SharedString = text.into();
        // Inline code is the inline-code run with a wash; code blocks carry no run
        // background. An app chip's icon slot is its own run on the link wash.
        let mut code: Vec<(Range<usize>, Hsla)> = Vec::new();
        let mut washes = Vec::new();
        let mut icons = Vec::new();
        let mut at = 0;
        for run in &mut runs {
            let span = at..at + run.len;
            at = span.end;
            let Some(wash) = run.background_color.take() else {
                continue;
            };
            if wash == theme.link_wash && text.get(span.clone()) == Some(APP_ICON_SLOT) {
                icons.push(span.start);
            }
            if run.font.family == theme.code_font.family {
                match code.last_mut() {
                    Some((last, last_wash)) if last.end == span.start && *last_wash == wash => {
                        last.end = span.end;
                    }
                    _ => code.push((span, wash)),
                }
                continue;
            }
            washes.push((span, wash));
        }
        Self {
            code,
            is_code_block: false,
            washes,
            icons,
            code_border: theme.foreground.opacity(0.13),
            styled: StyledText::new(text.clone()).with_runs(runs),
            id,
            text,
            links,
            handler: theme.links.clone(),
            document_order,
            selection: theme.selection,
            link: theme.link,
        }
    }

    pub(super) fn with_code_block(mut self) -> Self {
        self.is_code_block = true;
        self
    }

    /// The line-box quads a range covers: one per wrapped line between its ends (gpui-base's
    /// own selection painting, replicated because it is private there).
    fn span_quads(
        start: Point<Pixels>,
        end: Point<Pixels>,
        bounds: Bounds<Pixels>,
        line_height: Pixels,
    ) -> Vec<Bounds<Pixels>> {
        if start.y == end.y {
            return vec![Bounds::from_corners(
                start,
                Point::new(end.x, end.y + line_height),
            )];
        }
        let mut quads = vec![Bounds::from_corners(
            start,
            Point::new(bounds.right(), start.y + line_height),
        )];
        if end.y > start.y + line_height {
            quads.push(Bounds::from_corners(
                Point::new(bounds.left(), start.y + line_height),
                Point::new(bounds.right(), end.y),
            ));
        }
        quads.push(Bounds::from_corners(
            Point::new(bounds.left(), end.y),
            Point::new(end.x, end.y + line_height),
        ));
        quads
    }

    /// Slack's inline-code pill: a single rounded quad carrying both the fill and the 1px 13%
    /// foreground ring with 3px corners, slightly wider than the glyphs and inset 1px from the
    /// line box so the fill never pokes outside the border.
    fn paint_code_box(&self, pills: &[Bounds<Pixels>], wash: Hsla, window: &mut Window) {
        for &pill in pills {
            window.paint_quad(PaintQuad {
                bounds: pill,
                background: wash.into(),
                corner_radii: Corners::all(px(3.)),
                border_widths: Edges::all(px(1.)),
                border_color: self.code_border,
                border_style: BorderStyle::default(),
            });
        }
    }

    /// An app chip's icon in its slot at `ix`, centered on the line in the link color. Slack
    /// draws the app's own glyph (Jira's mark); the bundled link glyph stands in for it.
    fn paint_app_icon(&self, layout: &TextLayout, ix: usize, window: &mut Window, cx: &App) {
        let Some(at) = layout.position_for_index(ix) else {
            return;
        };
        let side = px(APP_ICON);
        let bounds = Bounds::new(
            Point::new(at.x + px(1.), at.y + (layout.line_height() - side) / 2.),
            gpui_kit::gpui::Size::new(side, side),
        );
        let icon = IconName::Link.path();
        if let Err(e) = window.paint_svg(
            bounds,
            icon.clone(),
            None,
            TransformationMatrix::unit(),
            self.link,
            cx,
        ) {
            eprintln!("slackless: could not paint the {icon} icon ({e})");
        }
    }

    /// A run wash under the text, laid out as GPUI lays out a run background.
    fn paint_wash(
        &self,
        layout: &TextLayout,
        range: Range<usize>,
        wash: Hsla,
        window: &mut Window,
    ) {
        for bounds in self.range_quads(layout, range) {
            window.paint_quad(PaintQuad {
                bounds,
                background: wash.into(),
                corner_radii: Corners::default(),
                border_widths: Edges::default(),
                border_color: transparent_black(),
                border_style: BorderStyle::default(),
            });
        }
    }

    /// The boxes a text range covers, one per wrapped line, each ending where that line's
    /// text ends (a selection instead runs on to the paragraph's edge).
    fn range_quads(&self, layout: &TextLayout, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        let (Some(start), Some(end)) = (
            layout.position_for_index(range.start),
            layout.position_for_index(range.end),
        ) else {
            return Vec::new();
        };
        let line_height = layout.line_height();
        let quad = |y: Pixels, left: Pixels, right: Pixels| {
            Bounds::from_corners(Point::new(left, y), Point::new(right, y + line_height))
        };
        if start.y == end.y {
            return vec![quad(start.y, start.x, end.x)];
        }
        // A wrapped span. The layout places a wrap boundary at the end of the line it closes,
        // so the farthest position found on a line is where that line's text ends; a line the
        // span only touches at such a boundary gets no box.
        let left = layout.bounds().left();
        let mut quads = Vec::new();
        let (mut y, mut from, mut to) = (start.y, start.x, start.x);
        for ix in (range.start..=range.end).filter(|ix| self.text.is_char_boundary(*ix)) {
            let Some(at) = layout.position_for_index(ix) else {
                continue;
            };
            if at.y == y {
                to = to.max(at.x);
                continue;
            }
            if to - from > px(1.) {
                quads.push(quad(y, from, to));
            }
            (y, from, to) = (at.y, left, at.x);
        }
        if to - from > px(1.) {
            quads.push(quad(y, from, to));
        }
        quads
    }

    /// The selection highlight behind the text.
    fn paint_selection(&self, layout: &TextLayout, range: Range<usize>, window: &mut Window) {
        let (Some(start), Some(end)) = (
            layout.position_for_index(range.start),
            layout.position_for_index(range.end),
        ) else {
            return;
        };
        for bounds in Self::span_quads(start, end, layout.bounds(), layout.line_height()) {
            window.paint_quad(PaintQuad {
                bounds,
                background: self.selection.into(),
                corner_radii: Corners::default(),
                border_widths: Edges::default(),
                border_color: transparent_black(),
                border_style: BorderStyle::default(),
            });
        }
    }

    /// Slack's hover affordance on URL links: the hovered link underlines (mentions keep
    /// their highlight). Painted over the text, so it needs no re-layout.
    fn paint_hover_underline(&self, layout: &TextLayout, range: Range<usize>, window: &mut Window) {
        let (Some(start), Some(end)) = (
            layout.position_for_index(range.start),
            layout.position_for_index(range.end),
        ) else {
            return;
        };
        let line_height = layout.line_height();
        for bounds in Self::span_quads(start, end, layout.bounds(), line_height) {
            let underline = Bounds::new(
                Point::new(bounds.origin.x, bounds.bottom() - px(4.)),
                gpui_kit::gpui::Size::new(bounds.size.width, px(1.)),
            );
            window.paint_quad(PaintQuad {
                bounds: underline,
                background: self.link.into(),
                corner_radii: Corners::default(),
                border_widths: Edges::default(),
                border_color: transparent_black(),
                border_style: BorderStyle::default(),
            });
        }
    }

    /// The paragraph's link interactions: the pointing cursor over a link, the hover
    /// underline (with a redraw when the hovered link changes) and the click — a press and
    /// a release inside one link range, so a drag that selects never activates the link.
    #[allow(clippy::too_many_arguments)]
    fn paint_links(
        &self,
        global_id: Option<&GlobalElementId>,
        layout: &TextLayout,
        hitbox: &Hitbox,
        window: &mut Window,
    ) {
        let hovered_now = layout
            .index_for_position(window.mouse_position())
            .ok()
            .and_then(|ix| self.links.iter().position(|(range, _)| range.contains(&ix)));
        if hovered_now.is_some() {
            window.set_cursor_style(CursorStyle::PointingHand, hitbox);
        }
        if let Some(ix) = hovered_now {
            if let Some((range, LinkTarget::Url(_))) = self.links.get(ix) {
                self.paint_hover_underline(layout, range.clone(), window);
            }
        }
        let (mouse_down, hovered_link, hover_key) = window.with_element_state::<ParagraphState, _>(
            global_id.expect("SelectableParagraph must have a stable element id"),
            |state, _| {
                let state = state.expect("request_layout retained the paragraph state");
                let cells = (
                    state.mouse_down.clone(),
                    state.hovered_link.clone(),
                    state.hover_key.clone(),
                );
                (cells, state)
            },
        );
        let down = {
            let layout = layout.clone();
            let mouse_down = mouse_down.clone();
            let hitbox = hitbox.clone();
            move |event: &MouseDownEvent, phase, window: &mut Window, _: &mut App| {
                if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                    if let Ok(ix) = layout.index_for_position(event.position) {
                        mouse_down.set(Some(ix));
                    }
                }
            }
        };
        window.on_mouse_event(down);
        let up = {
            let (layout, links, handler) =
                (layout.clone(), self.links.clone(), self.handler.clone());
            let hitbox = hitbox.clone();
            move |event: &MouseUpEvent, phase, window: &mut Window, cx: &mut App| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                let Some(down) = mouse_down.take() else {
                    return;
                };
                let Ok(release) = layout.index_for_position(event.position) else {
                    return;
                };
                if let Some((_, target)) = links
                    .iter()
                    .find(|(range, _)| range.contains(&down) && range.contains(&release))
                {
                    route_link(handler.as_ref(), target.clone(), window, cx);
                }
            }
        };
        window.on_mouse_event(up);
        // The wheel handler below takes its own copies: `r#move` moves the cells in.
        let wheel_hover = (hover_key.clone(), hovered_link.clone());
        let r#move = {
            let (layout, links) = (layout.clone(), self.links.clone());
            let (hitbox, hover_key) = (hitbox.clone(), hover_key.clone());
            move |event: &MouseMoveEvent, phase, window: &mut Window, cx: &mut App| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                // Off the paragraph counts as no link, so the cell (and a profile-link hover
                // card trigger) clears when the pointer leaves the hitbox too.
                let now = if hitbox.is_hovered(window) {
                    layout
                        .index_for_position(event.position)
                        .ok()
                        .and_then(|ix| links.iter().position(|(range, _)| range.contains(&ix)))
                } else {
                    None
                };
                let was = hovered_link.replace(now);
                if was == now {
                    return;
                }
                // Profile links (@mention pills) drive the member hover card: the anchor is
                // the pill's first glyph in window coordinates, pill-wide and one line tall.
                let trigger = |ix: usize| {
                    links.get(ix).and_then(|(range, target)| {
                        let LinkTarget::Profile(user) = target else {
                            return None;
                        };
                        let origin = layout.position_for_index(range.start)?;
                        Some((
                            user.clone(),
                            Bounds::new(origin, size(px(40.), layout.line_height())),
                        ))
                    })
                };
                if let Some((user, anchor)) = now.and_then(trigger) {
                    hover_card::entered(
                        Trigger {
                            key: hover_key.clone(),
                            user,
                            anchor,
                        },
                        cx,
                    );
                } else if was.and_then(trigger).is_some() {
                    hover_card::left(&hover_key, cx);
                }
                window.refresh();
            }
        };
        window.on_mouse_event(r#move);
        // A scroll can move a trigger out from under a still pointer: the card closes, as
        // Slack's does. Only the paragraph owning the trigger reports (its cell is set).
        let wheel = {
            let (hover_key, hovered_link) = wheel_hover;
            move |_: &ScrollEvent, phase, _window: &mut Window, cx: &mut App| {
                if phase == DispatchPhase::Bubble && hovered_link.replace(None).is_some() {
                    hover_card::left(&hover_key, cx);
                }
            }
        };
        window.on_mouse_event(wheel);
    }

    /// Bounded multi-click selection inside inline code pills (`.c-mrkdwn__code`) and fenced
    /// code blocks (`.c-mrkdwn__pre`), matching Slack:
    /// - Double-click inside inline code selects the word strictly clamped inside that pill.
    /// - Triple-click (or 4+ clicks) inside inline code selects only that inline code span,
    ///   never the surrounding paragraph.
    /// - Triple-click (or 4+ clicks) inside a fenced code block selects the entire code block
    ///   across all lines.
    fn paint_code_clicks(
        &self,
        handle: &TextSelectionHandle,
        local_range: Rc<RefCell<Option<Range<usize>>>>,
        code_pills: Vec<(Range<usize>, Vec<Bounds<Pixels>>)>,
        layout: &TextLayout,
        hitbox: &Hitbox,
        window: &mut Window,
    ) {
        let is_code_block = self.is_code_block;
        let text = self.text.clone();
        let layout = layout.clone();
        let hitbox = hitbox.clone();
        let handle = handle.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || event.button != MouseButton::Left
                || event.click_count < 2
                || !hitbox.is_hovered(window)
            {
                return;
            }
            let selected = if is_code_block {
                if event.click_count >= 3 && !text.is_empty() {
                    Some(0..text.len())
                } else {
                    None
                }
            } else {
                let hit = match layout.index_for_position(event.position) {
                    Ok(ix) => code_pills
                        .iter()
                        .find(|(span, _)| span.contains(&ix))
                        .map(|(span, _)| (span.clone(), ix))
                        .or_else(|| {
                            code_pills
                                .iter()
                                .find(|(_, quads)| {
                                    quads.iter().any(|q| q.contains(&event.position))
                                })
                                .map(|(span, _)| {
                                    let clamped = ix.clamp(
                                        span.start,
                                        span.end.saturating_sub(1).max(span.start),
                                    );
                                    (span.clone(), clamped)
                                })
                        }),
                    Err(nearest) => code_pills
                        .iter()
                        .find(|(_, quads)| quads.iter().any(|q| q.contains(&event.position)))
                        .map(|(span, _)| {
                            let clamped = nearest
                                .clamp(span.start, span.end.saturating_sub(1).max(span.start));
                            (span.clone(), clamped)
                        }),
                };
                hit.and_then(|(span, ix)| {
                    if event.click_count >= 3 {
                        (!span.is_empty()).then_some(span)
                    } else {
                        word_range_bounded(&text, span, ix)
                    }
                })
            };
            let Some(range) = selected else {
                return;
            };
            GlobalState::suppress_text_selection(cx);
            *local_range.borrow_mut() = Some(range);
            handle.set_local_selection(true, cx);
            window.refresh();
        });
    }
}

/// Word range around `offset`, strictly clamped to `bounds` so double-clicking inside an
/// inline code span never bleeds into adjacent prose.
fn word_range_bounded(text: &str, bounds: Range<usize>, offset: usize) -> Option<Range<usize>> {
    if bounds.is_empty() || bounds.end > text.len() {
        return None;
    }
    let slice = text.get(bounds.clone())?;
    let mut rel = offset.saturating_sub(bounds.start).min(slice.len());
    if rel == slice.len() {
        rel = slice.char_indices().next_back().map_or(0, |(ix, _)| ix);
    } else {
        while rel > 0 && !slice.is_char_boundary(rel) {
            rel -= 1;
        }
    }
    let ch = slice[rel..].chars().next()?;
    let classify = |c: char| -> u8 {
        if c == '_'
            || c.is_ascii_alphanumeric()
            || matches!(
                c,
                '\u{00C0}'..='\u{024F}'
                    | '\u{0400}'..='\u{04FF}'
                    | '\u{1E00}'..='\u{1EFF}'
                    | '\u{0300}'..='\u{036F}'
            )
        {
            1 // Word
        } else if matches!(c, '\n' | '\r') {
            2 // Newline
        } else if c.is_whitespace() {
            3 // Whitespace
        } else {
            4 // Other
        }
    };
    let kind = classify(ch);
    let connects = |next: char| matches!((kind, classify(next)), (1, 1) | (3, 3));
    let mut start = rel;
    for prev in slice[..rel].chars().rev().take(128) {
        if connects(prev) {
            start -= prev.len_utf8();
        } else {
            break;
        }
    }
    let mut end = rel + ch.len_utf8();
    for next in slice[end..].chars().take(128) {
        if connects(next) {
            end += next.len_utf8();
        } else {
            break;
        }
    }
    Some((bounds.start + start)..(bounds.start + end))
}

impl gpui_kit::gpui::IntoElement for SelectableParagraph {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectableParagraph {
    type RequestLayoutState = TextSelectionHandle;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let handle = window.with_element_state(
            global_id.expect("SelectableParagraph must have a stable element id"),
            |retained: Option<ParagraphState>, window| {
                let retained = retained.unwrap_or_else(|| {
                    let handle = TextSelectionHandle::new(self.text.clone(), cx);
                    let local_range: Rc<RefCell<Option<Range<usize>>>> =
                        Rc::new(RefCell::new(None));
                    let projected: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
                    {
                        let local_for_copy = local_range.clone();
                        let projected_for_copy = projected.clone();
                        let text_for_copy = self.text.clone();
                        handle.copy_with(
                            move |_| {
                                if let Some(range) = local_for_copy.borrow().as_ref() {
                                    return text_for_copy
                                        .get(range.clone())
                                        .unwrap_or_default()
                                        .to_owned();
                                }
                                projected_for_copy.borrow().clone()
                            },
                            cx,
                        );
                    }
                    {
                        let local_for_clear = local_range.clone();
                        handle.clear_with(
                            move |_| {
                                *local_for_clear.borrow_mut() = None;
                            },
                            cx,
                        );
                    }
                    let refresh = handle.refresh_window_on_change(window, cx);
                    ParagraphState {
                        handle,
                        local_range,
                        projected,
                        hovered_link: Rc::new(Cell::new(None)),
                        mouse_down: Rc::new(Cell::new(None)),
                        hover_key: format!("{:?}", self.id).into(),
                        _refresh: refresh,
                    }
                });
                (retained.handle.clone(), retained)
            },
        );
        let (layout_id, ()) = self
            .styled
            .request_layout(global_id, inspector_id, window, cx);
        (layout_id, handle)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.styled
            .prepaint(global_id, inspector_id, bounds, &mut (), window, cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
            .with_document_order(self.document_order)
            .with_text_bounds(vec![bounds])
            .with_rendered_element(handle, window, cx);
        handle.register(registration, window, cx);
        hitbox
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.styled.layout().clone();
        #[cfg(test)]
        super::tests::record_painted_paragraph(&self.text, &layout);
        let projection = handle.update_runs(
            &[
                TextSelectionRun::new(self.text.clone(), layout.clone(), bounds)
                    .with_document_order(self.document_order),
            ],
            cx,
        );
        let (local_range, projected) = window.with_element_state::<ParagraphState, _>(
            global_id.expect("SelectableParagraph must have a stable element id"),
            |state, _| {
                let state = state.expect("request_layout retained the paragraph state");
                let cells = (state.local_range.clone(), state.projected.clone());
                (cells, state)
            },
        );
        {
            let mut out = projected.borrow_mut();
            out.clear();
            for range in projection.ranges().iter().flatten() {
                if let Some(slice) = self.text.get(range.clone()) {
                    out.push_str(slice);
                }
            }
        }
        for (range, wash) in &self.washes {
            self.paint_wash(&layout, range.clone(), *wash, window);
        }
        let mut code_pills: Vec<(Range<usize>, Vec<Bounds<Pixels>>)> =
            Vec::with_capacity(self.code.len());
        for (range, wash) in &self.code {
            let pills: Vec<Bounds<Pixels>> = self
                .range_quads(&layout, range.clone())
                .into_iter()
                .map(|b| {
                    Bounds::from_corners(
                        Point::new(b.left() - px(2.), b.top() + px(1.)),
                        Point::new(b.right() + px(2.), b.bottom() - px(1.)),
                    )
                })
                .collect();
            self.paint_code_box(&pills, *wash, window);
            code_pills.push((range.clone(), pills));
        }
        if let Some(range) = local_range.borrow().clone() {
            self.paint_selection(&layout, range, window);
        } else {
            for range in projection.ranges().iter().flatten().cloned() {
                self.paint_selection(&layout, range, window);
            }
        }
        self.styled.paint(
            global_id,
            inspector_id,
            bounds,
            &mut (),
            &mut (),
            window,
            cx,
        );
        for &ix in &self.icons {
            self.paint_app_icon(&layout, ix, window, cx);
        }
        // Selectable text takes the text cursor, as in Slack; links switch it to a hand.
        let over_link = layout
            .index_for_position(window.mouse_position())
            .ok()
            .is_some_and(|ix| self.links.iter().any(|(range, _)| range.contains(&ix)));
        if !over_link {
            window.set_cursor_style(CursorStyle::IBeam, hitbox);
        }
        if self.is_code_block || !code_pills.is_empty() {
            self.paint_code_clicks(handle, local_range, code_pills, &layout, hitbox, window);
        }
        if !self.links.is_empty() {
            self.paint_links(global_id, &layout, hitbox, window);
        }
    }
}
