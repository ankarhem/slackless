//! Shared form controls (buttons, single/multi-line text boxes, monospace inputs, search boxes)
//! sized consistently to Clean (`32px`) or Compact (`28px`) density.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex};
use gpui_kit::gpui::{App, Div, Entity, ParentElement as _, Pixels, Styled as _, div, px};

use crate::preferences::display_prefs;

/// Shared height and size tokens for form controls (buttons, selects, single-line inputs),
/// switching between Clean (`32px`) and Compact (`28px`) based on `DisplayPrefs::compact`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlMetrics {
    pub compact: bool,
    pub height: Pixels,
    pub button_size: Size,
    pub input_size: Size,
}

impl ControlMetrics {
    pub fn for_compact(compact: bool) -> Self {
        if compact {
            Self {
                compact: true,
                height: px(28.),
                button_size: Size::Small,
                input_size: Size::Small,
            }
        } else {
            Self {
                compact: false,
                height: px(32.),
                button_size: Size::Medium,
                input_size: Size::Small,
            }
        }
    }
}

pub fn control_metrics(cx: &App) -> ControlMetrics {
    ControlMetrics::for_compact(display_prefs(cx).compact)
}

/// Apply standard form/dialog/toolbar button sizing (`32px` Clean, `28px` Compact).
///
/// Use only for labeled buttons — never icon-only buttons, which should keep square dimensions.
pub fn form_button(btn: Button, cx: &App) -> Button {
    form_button_for(btn, control_metrics(cx))
}

pub(crate) fn form_button_for(btn: Button, m: ControlMetrics) -> Button {
    if m.compact {
        btn.with_size(m.button_size).h(m.height)
    } else {
        btn.with_size(m.button_size)
    }
}

/// Bordered text input wrapper sized to `ControlMetrics` (`32px` Clean, `28px` Compact).
///
/// Set `multi_line = false` for single-line (`auto_grow(1, 1)`) fields and `multi_line = true`
/// for multi-line (`auto_grow(1, N)`) fields.
pub fn text_input_box(state: &Entity<TextareaState>, multi_line: bool, cx: &App) -> Div {
    text_input_box_for(state, multi_line, false, control_metrics(cx), cx)
}

/// Monospace bordered text input wrapper sized to `ControlMetrics` (`32px` Clean, `28px` Compact).
pub fn text_input_mono(state: &Entity<TextareaState>, multi_line: bool, cx: &App) -> Div {
    text_input_box_for(state, multi_line, true, control_metrics(cx), cx)
}

pub(crate) fn text_input_box_for(
    state: &Entity<TextareaState>,
    multi_line: bool,
    mono: bool,
    m: ControlMetrics,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let border = theme.input;
    let mut textarea = Textarea::new(state)
        .with_size(m.input_size)
        .appearance(false)
        .bordered(false);
    if mono {
        textarea = textarea
            .font_family(theme.mono_font_family.clone())
            .text_xs();
    }
    let wrapper = h_flex()
        .items_center()
        .rounded_md()
        .border_1()
        .border_color(border)
        .px_1()
        .child(div().flex_1().min_w_0().child(textarea));
    if multi_line {
        wrapper
            .min_h(m.height)
            .py(px(if m.compact { 1. } else { 2. }))
    } else {
        wrapper.h(m.height)
    }
}

/// Bordered search/filter input with leading search icon sized to `ControlMetrics`.
pub fn search_input_box(state: &Entity<TextareaState>, cx: &App) -> Div {
    let m = control_metrics(cx);
    let theme = cx.theme();
    let border = theme.input;
    let muted = theme.muted_foreground;
    h_flex()
        .items_center()
        .gap_0p5()
        .pl_2p5()
        .pr_1()
        .h(m.height)
        .rounded_md()
        .border_1()
        .border_color(border)
        .child(
            Icon::new(IconName::Search)
                .size(px(14.))
                .flex_none()
                .text_color(muted),
        )
        .child(
            div().flex_1().min_w_0().child(
                Textarea::new(state)
                    .with_size(m.input_size)
                    .appearance(false)
                    .bordered(false),
            ),
        )
}
