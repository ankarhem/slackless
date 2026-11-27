//! The transcript's shell elements, one implementation shared by every column that renders
//! messages (the transcript, the thread panel, the Threads view's cards, Unreads): the day
//! divider, the "New" unread marker and the collapsed deleted-message row. A caller sets the
//! element's horizontal inset to line up with its own rows; the element owns the rest.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonRounded};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex};
use gpui_kit::gpui::{
    Anchor, App, Div, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, div,
    px,
};

use super::MessageList;

/// The date divider's "Jump to…" targets.
#[derive(Clone, Copy)]
pub enum Jump {
    Recent,
    Yesterday,
    LastWeek,
    LastMonth,
    Beginning,
}

/// Slack's day divider: a full-width rule with the day's pill (label, then a chevron) on it.
/// With `list`, the pill's menu lists Slack's "Jump to…" targets; Unreads, which has nowhere
/// to jump, shows the label alone.
pub(crate) fn date_divider(
    label: String,
    key: String,
    list: Option<WeakEntity<MessageList>>,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let (border, background, fg) = (theme.border, theme.background, theme.foreground);
    // Slack's pill: 28px high, 13px bold, 16px before the label and 8px after its small
    // chevron, a 13% foreground ring and a faint drop shadow.
    let pill = Button::new(SharedString::from(key))
        .outline()
        .with_size(Size::XSmall)
        .rounded(ButtonRounded::Size(px(999.)))
        .h(px(28.))
        .pl(px(16.))
        .bg(background)
        .border_color(fg.opacity(0.13))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::BOLD)
                .text_color(fg)
                .child(label),
        );
    // `Button::shadow` toggles the kit's own shadow; this is the element's.
    let pill = gpui_kit::gpui::Styled::shadow(
        pill,
        vec![gpui_kit::gpui::BoxShadow {
            color: Hsla::black().opacity(0.08),
            offset: gpui_kit::gpui::point(px(0.), px(1.)),
            blur_radius: px(3.),
            spread_radius: px(0.),
            inset: false,
        }],
    );
    let pill = match list {
        Some(list) => pill
            .pr(px(8.))
            .child(
                Icon::new(IconName::ChevronDown)
                    .size(px(10.))
                    .text_color(fg),
            )
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                menu = menu.label("Jump to…");
                for (jump, text) in [
                    (Jump::Recent, "Most recent"),
                    (Jump::Yesterday, "Yesterday"),
                    (Jump::LastWeek, "Last week"),
                    (Jump::LastMonth, "Last month"),
                    (Jump::Beginning, "The very beginning"),
                ] {
                    let list = list.clone();
                    menu = menu.item(PopupMenuItem::new(text).on_click(move |_, _, cx| {
                        let _ = list.update(cx, |list, cx| list.jump(jump, cx));
                    }));
                }
                menu
            })
            .into_any_element(),
        None => pill.pr(px(16.)).into_any_element(),
    };
    let rule = || div().flex_1().h(px(1.)).bg(border);
    h_flex()
        .items_center()
        .py_1()
        .child(rule())
        .child(pill)
        .child(rule())
}

/// Slack's unread marker: a danger-colored line with the "New" label at its right end.
pub(crate) fn new_messages_divider(cx: &App) -> Div {
    let danger = cx.theme().danger;
    h_flex()
        .items_center()
        .gap_2()
        .py_1()
        .child(div().flex_1().h(px(1.)).bg(danger))
        .child(
            div()
                .flex_none()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(danger)
                .child("New"),
        )
}

/// The hover group a deleted message's "Show" / "Hide" links reveal themselves through.
const DELETED_GROUP: &str = "deleted-row";

/// Flips whether a deleted message stands expanded or collapsed.
pub(crate) type DeletedToggle = Rc<dyn Fn(&mut Window, &mut App)>;

/// A collapsed deleted message: the trash icon in the avatar gutter (`gutter` wide), the
/// muted "Deleted message · author · time" line and a hover-underlined "Show"; the row
/// expands on click. A deleted message starts collapsed when "Expand deleted messages by
/// default" is off, and collapses again when its "Hide" is clicked.
pub(crate) struct DeletedRow {
    /// The row's element id.
    pub key: String,
    pub author: String,
    pub stamp: String,
    /// Width of the avatar gutter (px), the trash icon's column.
    pub gutter: f32,
    pub on_toggle: DeletedToggle,
}

pub(crate) fn deleted_row(row: DeletedRow, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let (muted, fg, hover_bg) = (
        theme.muted_foreground,
        theme.foreground,
        theme.secondary.opacity(0.38),
    );
    let compact = crate::preferences::display_prefs(cx).compact;
    let DeletedRow {
        key,
        author,
        stamp,
        gutter,
        on_toggle,
    } = row;
    h_flex()
        .id(SharedString::from(key))
        .group(DELETED_GROUP)
        .min_w_0()
        .items_center()
        .gap(px(if compact { 8. } else { 12. }))
        .py(px(if compact { 2. } else { 4. }))
        .cursor_pointer()
        .hover(move |el| el.bg(hover_bg))
        .child(
            div()
                .w(px(gutter))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(IconName::Trash)
                        .size(px(12.))
                        .text_color(muted.opacity(0.55)),
                ),
        )
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap_2()
                .text_xs()
                .child(
                    div()
                        .italic()
                        .text_color(muted.opacity(0.8))
                        .child(format!("Deleted message \u{b7} {author} \u{b7} {stamp}")),
                )
                .child(
                    div()
                        .text_color(muted)
                        .group_hover(DELETED_GROUP, move |el| el.text_color(fg).underline())
                        .child("Show"),
                ),
        )
        .on_click(move |_, window, cx| on_toggle(window, cx))
}

/// The "Hide" link after an expanded deleted message's "deleted" marker; it collapses the
/// message again.
pub(crate) fn deleted_hide(key: String, on_toggle: DeletedToggle, cx: &App) -> Stateful<Div> {
    let (muted, fg) = (cx.theme().muted_foreground, cx.theme().foreground);
    div()
        .id(SharedString::from(key))
        .text_xs()
        .text_color(muted)
        .cursor_pointer()
        .hover(move |el| el.text_color(fg).underline())
        .child("Hide")
        .on_click(move |_, window, cx| on_toggle(window, cx))
}
