//! Window chrome, top bar, banners, conversation header, menus, join card, and tabs.

mod app_home;
mod banners;
mod connect;
mod header;
mod top_bar;

use super::*;

pub(super) use self::connect::joinable;
#[cfg(test)]
pub(super) use self::connect::{channel_people, includes_line};

/// A black box shadow (Slack's `box-shadow: <offset> <offset> <blur> rgba(0,0,0,<alpha>)`).
pub(super) fn shadow(offset: f32, alpha: f32, blur: f32) -> BoxShadow {
    BoxShadow {
        color: hsla(0., 0., 0., alpha),
        offset: point(px(offset), px(offset)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    }
}

/// One right corner (`top` or bottom) of the workspace panel over a side panel that paints a
/// square background: GPUI clips rectangularly, so a band whose inner edge is the corner's arc
/// repaints the window chrome outside it, and dark mode's `outline` is redrawn along it.
pub(super) fn corner_cap(top: bool, chrome: Hsla, outline: Option<Hsla>) -> Vec<Div> {
    let band = PANEL_RADIUS;
    // Absolute insets start inside the panel's outline.
    let edge = if outline.is_some() { 1. } else { 0. };
    let cap = div()
        .absolute()
        .right(px(-band - edge))
        .size(px(2. * (PANEL_RADIUS + band)))
        .border_r(px(band))
        .border_color(chrome);
    let mut caps = vec![if top {
        cap.top(px(-band - edge))
            .border_t(px(band))
            .rounded_tr(px(PANEL_RADIUS + band))
    } else {
        cap.bottom(px(-band - edge))
            .border_b(px(band))
            .rounded_br(px(PANEL_RADIUS + band))
    }];
    if let Some(outline) = outline {
        let arc = div()
            .absolute()
            .right(px(-edge))
            .size(px(2. * PANEL_RADIUS))
            .border_r_1()
            .border_color(outline);
        caps.push(if top {
            arc.top(px(-edge)).border_t_1().rounded_tr(px(PANEL_RADIUS))
        } else {
            arc.bottom(px(-edge))
                .border_b_1()
                .rounded_br(px(PANEL_RADIUS))
        });
    }
    caps
}

pub(super) fn chrome_icon(
    id: &'static str,
    icon: IconName,
    color: Hsla,
    hover_bg: Hsla,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(28.))
        .rounded_sm()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(color)
        .hover(|el| el.bg(hover_bg))
        .child(Icon::new(icon).text_color(color).size(px(16.)))
}
