//! The composer's controls: the formatting toolbar (buttons lit for what is in effect at
//! the caret) and Slack's action row with the send button. Icons come from the bundled
//! Lucide set.

use gpui_kit::assets::IconName;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, AnyElement, AnyView, App, ClickEvent, Context, Div, FontWeight, Hsla, ImageSource,
    InteractiveElement as _, IntoElement, MouseButton, ObjectFit, ParentElement as _, Stateful,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, img, px,
};

use super::keys::Tool;
use super::{AlsoSendTarget, Composer, ComposerEvent};
use crate::drafts::SendLater;
use crate::editor::{LineKind, Style};

/// One toolbar position: a tool with its button id and icon, a divider between groups, or
/// the paragraph-style dropdown.
pub(super) enum Slot {
    Tool(&'static str, IconName, Tool),
    Divider,
    Styles,
}

/// Slack's grouping: text style + headings | link & lists | quote, code & divider.
pub(super) const TOOLBAR: [Slot; 14] = [
    Slot::Tool("fmt-bold", IconName::Bold, Tool::Style(Style::BOLD)),
    Slot::Tool("fmt-italic", IconName::Italic, Tool::Style(Style::ITALIC)),
    Slot::Tool(
        "fmt-underline",
        IconName::Underline,
        Tool::Style(Style::UNDERLINE),
    ),
    Slot::Tool(
        "fmt-strike",
        IconName::Strikethrough,
        Tool::Style(Style::STRIKE),
    ),
    Slot::Styles,
    Slot::Divider,
    Slot::Tool("fmt-link", IconName::Link, Tool::Link),
    Slot::Tool(
        "fmt-ol",
        IconName::ListOrdered,
        Tool::Block(LineKind::Ordered),
    ),
    Slot::Tool("fmt-ul", IconName::List, Tool::Block(LineKind::Bullet)),
    Slot::Divider,
    Slot::Tool(
        "fmt-quote",
        IconName::TextQuote,
        Tool::Block(LineKind::Quote),
    ),
    Slot::Tool("fmt-code", IconName::CodeXml, Tool::Style(Style::CODE)),
    Slot::Tool(
        "fmt-code-block",
        IconName::SquareCode,
        Tool::Block(LineKind::Code),
    ),
    Slot::Tool("fmt-divider", IconName::SeparatorHorizontal, Tool::Divider),
];

/// The popover over a toolbar button: Slack's title and its individual keycap chips
/// (verified against official Slack macOS: 10 shortcut buttons + single-row `"Divider"`;
/// `¶▾` has no hover tooltip).
fn slot_tip(tool: Tool) -> Option<(&'static str, &'static [&'static str])> {
    Some(match tool {
        Tool::Style(style) if style == Style::BOLD => ("Bold", &["⌘", "B"]),
        Tool::Style(style) if style == Style::ITALIC => ("Italic", &["⌘", "I"]),
        Tool::Style(style) if style == Style::UNDERLINE => ("Underline", &["⌘", "U"]),
        Tool::Style(style) if style == Style::STRIKE => ("Strikethrough", &["⌘", "Shift", "X"]),
        Tool::Link => ("Link", &["⌘", "Shift", "U"]),
        Tool::Block(LineKind::Ordered) => ("Ordered list", &["⌘", "Shift", "7"]),
        Tool::Block(LineKind::Bullet) => ("Bulleted list", &["⌘", "Shift", "8"]),
        Tool::Block(LineKind::Quote) => ("Blockquote", &["⌘", "Shift", "9"]),
        Tool::Style(style) if style == Style::CODE => ("Code", &["⌘", "Shift", "C"]),
        Tool::Block(LineKind::Code) => ("Code block", &["⌘", "Option", "Shift", "C"]),
        Tool::Divider => ("Divider", &[]),
        Tool::Style(_)
        | Tool::Block(
            LineKind::Fence | LineKind::Soft | LineKind::Heading(_) | LineKind::Divider,
        )
        | Tool::Mention
        | Tool::Slash
        | Tool::Clear
        | Tool::Heading(_)
        | Tool::Paragraph => return None,
    })
}

/// Slack's centered hover popover: bold title on row 1, and optional bordered keycap chips
/// on row 2 when `keys` is non-empty.
fn tip_view(
    label: &'static str,
    keys: &'static [&'static str],
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    Tooltip::element(move |_, cx| {
        let theme = cx.theme();
        let chip_bg = if theme.mode.is_dark() {
            theme.background
        } else {
            theme.secondary
        };
        let (chip_border, chip_fg) = (theme.foreground.opacity(0.25), theme.popover_foreground);
        v_flex()
            .items_center()
            .gap_1p5()
            .px_1()
            .py_0p5()
            .child(div().text_xs().font_weight(FontWeight::BOLD).child(label))
            .when(!keys.is_empty(), |col| {
                col.child(h_flex().items_center().justify_center().gap_1p5().children(
                    keys.iter().map(|key| {
                        div()
                            .min_w(px(20.))
                            .h(px(20.))
                            .px_1p5()
                            .rounded(px(4.))
                            .border_1()
                            .border_color(chip_border)
                            .bg(chip_bg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(chip_fg)
                            .child(*key)
                    }),
                ))
            })
    })
    .build(window, cx)
}

/// Slack's 2-line centered hover popover with a bold title and plain-text hint below it
/// (e.g. `+` button: `"Attach"` / `"Tip: double click to upload"`).
fn tip_sub(
    label: &'static str,
    subtitle: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    Tooltip::element(move |_, cx| {
        let fg = cx.theme().popover_foreground;
        v_flex()
            .items_center()
            .gap_0p5()
            .px_1()
            .py_0p5()
            .child(div().text_xs().font_weight(FontWeight::BOLD).child(label))
            .child(div().text_xs().text_color(fg.opacity(0.85)).child(subtitle))
    })
    .build(window, cx)
}

/// Conservative logical width (px) of a single formatting-toolbar slot.
fn slot_width(slot: &Slot) -> f32 {
    match slot {
        Slot::Tool(..) => 26.0,
        Slot::Styles => 40.0,
        Slot::Divider => 9.0,
    }
}

/// Computes `(visible_slot_count, show_md_inline)` for a formatting toolbar inside a composer
/// box of `avail_width` logical pixels. When not all controls fit, trailing slots collapse
/// behind a `⋯` `"More options"` menu and any dangling trailing divider is omitted.
pub(super) fn visible_toolbar_layout(avail_width: f32) -> (usize, bool) {
    const GAP: f32 = 2.0;
    const OVERHEAD: f32 = 14.0; // 2px border + 12px horizontal padding (px_1p5)
    const MD_RESERVE: f32 = 38.0; // MD pill (~32px) + breathing gap (6px)
    const MORE_RESERVE: f32 = 28.0; // 26px ellipsis button + 2px gap

    let full_slots_width: f32 = TOOLBAR.iter().map(slot_width).sum::<f32>()
        + GAP * (TOOLBAR.len().saturating_sub(1) as f32);
    if OVERHEAD + full_slots_width + MD_RESERVE <= avail_width {
        return (TOOLBAR.len(), true);
    }

    // Keep the MD pill inline while there's comfortable width; on narrow drawers move it into
    // the "More options" menu so more primary formatting buttons stay directly clickable.
    let show_md = avail_width >= 340.0;
    let budget =
        (avail_width - OVERHEAD - MORE_RESERVE - if show_md { MD_RESERVE } else { 0.0 }).max(0.0);

    let mut used = 0.0_f32;
    let mut visible = 0usize;
    for slot in &TOOLBAR {
        let step = if visible == 0 {
            slot_width(slot)
        } else {
            GAP + slot_width(slot)
        };
        if used + step > budget {
            break;
        }
        used += step;
        visible += 1;
    }

    // Never leave a trailing group divider directly before the `⋯` overflow button.
    while visible > 0 && matches!(TOOLBAR[visible - 1], Slot::Divider) {
        visible -= 1;
    }

    (visible, show_md)
}

/// Returns `true` when the bottom action row does not have room for the clip & shortcut tail
/// buttons alongside `+ Aa 😊 @` and the right-hand Send control, so those tail buttons
/// collapse behind a `⋯` `"More options"` button (matching Slack's narrow thread composer).
pub(super) fn action_row_collapses_tail(avail_width: f32, scheduling: bool) -> bool {
    const OVERHEAD: f32 = 18.0; // 2px border + 16px horizontal padding (px_2)
    const CORE_LEFT: f32 = 110.0; // + (26) + Aa (26) + emoji (26) + @ (26) + 3*2px gaps
    const TAIL_LEFT: f32 = 92.0; // | (1) + video (28) + mic (26) + | (1) + / (26) + 5*2px gaps
    const BREATHING_GAP: f32 = 16.0;
    let right = if scheduling { 59.0 } else { 32.0 };
    avail_width < OVERHEAD + CORE_LEFT + TAIL_LEFT + BREATHING_GAP + right
}

enum HiddenToolbarEntry {
    Separator,
    Styles(u8),
    Tool {
        icon: IconName,
        label: &'static str,
        keys: &'static [&'static str],
        tool: Tool,
        active: bool,
    },
}

/// The square a toolbar or action-row control sits in.
fn control(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(26.))
        .rounded_sm()
        .flex()
        .items_center()
        .justify_center()
}

impl Composer {
    /// The top formatting toolbar, responsively truncating overflowing tools behind a `⋯`
    /// `"More options"` dropdown when the composer box is narrow (e.g. thread or huddle drawers).
    pub(super) fn formatting_toolbar(
        &self,
        focused: bool,
        band: Hsla,
        cx: &mut Context<Self>,
    ) -> Div {
        let (visible, show_md) = self
            .box_width
            .map(visible_toolbar_layout)
            .unwrap_or((TOOLBAR.len(), true));
        let has_overflow = visible < TOOLBAR.len() || !show_md;

        let markup_active = !self.format_markup;
        let theme = cx.theme();
        let md_fg = if markup_active {
            theme.primary
        } else {
            theme.muted_foreground.opacity(0.32)
        };
        let md_bg = if markup_active {
            theme.primary.opacity(0.14)
        } else {
            theme.secondary.opacity(0.)
        };
        let md_hover = theme.secondary_hover;

        h_flex()
            .items_center()
            .justify_between()
            .min_w_0()
            .px_1p5()
            // Slack's band: 4px around 28px buttons.
            .py(px(5.))
            .rounded_t_lg()
            .bg(band)
            .child(
                h_flex()
                    .items_center()
                    .min_w_0()
                    .gap_0p5()
                    .children(
                        TOOLBAR[..visible]
                            .iter()
                            .map(|slot| self.tool_button(slot, focused, cx)),
                    )
                    .when(has_overflow, |row| {
                        row.child(self.formatting_more_button(visible, show_md, focused, cx))
                    }),
            )
            .when(show_md, |bar| {
                bar.child(
                    div()
                        .id("fmt-markup-mode")
                        .flex_none()
                        .cursor_pointer()
                        .h(px(22.))
                        .px_1p5()
                        .rounded(px(4.))
                        .bg(md_bg)
                        .hover(move |el| el.bg(md_hover))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(md_fg)
                        .tooltip(|window, cx| {
                            Tooltip::new("Toggle raw markup mode (show formatting characters)")
                                .build(window, cx)
                        })
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_format_markup(!this.format_markup, cx);
                        }))
                        .child("MD"),
                )
            })
    }

    /// The `⋯` `"More options"` overflow dropdown on the top formatting toolbar.
    fn formatting_more_button(
        &self,
        visible: usize,
        show_md: bool,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let idle = cx.theme().foreground.opacity(0.9);
        let color = if focused { idle } else { idle.opacity(0.3) };
        let active_heading = match self.editor.active_block() {
            Some(LineKind::Heading(level)) => level,
            _ => 0,
        };
        let markup_active = !self.format_markup;

        let mut entries = Vec::new();
        for slot in &TOOLBAR[visible..] {
            match slot {
                Slot::Divider => {
                    if !entries.is_empty() {
                        entries.push(HiddenToolbarEntry::Separator);
                    }
                }
                Slot::Styles => entries.push(HiddenToolbarEntry::Styles(active_heading)),
                Slot::Tool(_, icon, tool) => {
                    let (label, keys) = slot_tip(*tool).unwrap_or(("Format", &[]));
                    entries.push(HiddenToolbarEntry::Tool {
                        icon: *icon,
                        label,
                        keys,
                        tool: *tool,
                        active: self.is_active(*tool),
                    });
                }
            }
        }
        let entries = std::rc::Rc::new(entries);
        let this = cx.entity().downgrade();

        let trigger = Button::new("fmt-more")
            .ghost()
            .with_size(Size::XSmall)
            .size(px(26.))
            .p_0()
            .child(
                Icon::new(IconName::Ellipsis)
                    .text_color(color)
                    .size(px(15.)),
            )
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |mut menu, _, _| {
                let mut had_items = false;
                for entry in entries.iter() {
                    match entry {
                        HiddenToolbarEntry::Separator => {
                            if had_items {
                                menu = menu.separator();
                            }
                        }
                        HiddenToolbarEntry::Styles(active_level) => {
                            for (level, icon, choice, tool) in [
                                (0u8, IconName::Pilcrow, "Paragraph", Tool::Paragraph),
                                (1, IconName::Heading1, "Big heading", Tool::Heading(1)),
                                (2, IconName::Heading2, "Medium heading", Tool::Heading(2)),
                                (3, IconName::Heading3, "Small heading", Tool::Heading(3)),
                            ] {
                                let this = this.clone();
                                let checked = *active_level == level;
                                menu = menu.item(
                                    PopupMenuItem::element(move |_, cx| {
                                        let theme = cx.theme();
                                        let fg = if checked {
                                            theme.link
                                        } else {
                                            theme.popover_foreground
                                        };
                                        h_flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .w(px(14.))
                                                    .flex_none()
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .children(checked.then(|| {
                                                        Icon::new(IconName::Check)
                                                            .size(px(14.))
                                                            .text_color(fg)
                                                    })),
                                            )
                                            .child(Icon::new(icon).size(px(15.)).text_color(fg))
                                            .child(div().text_color(fg).child(choice))
                                    })
                                    .on_click(
                                        move |_, window, cx| {
                                            if let Some(this) = this.upgrade() {
                                                this.update(cx, |composer, cx| {
                                                    composer.run(tool, window, cx);
                                                });
                                            }
                                        },
                                    ),
                                );
                                had_items = true;
                            }
                        }
                        HiddenToolbarEntry::Tool {
                            icon,
                            label,
                            keys,
                            tool,
                            active,
                        } => {
                            let this = this.clone();
                            let icon = *icon;
                            let label = *label;
                            let shortcut = (!keys.is_empty()).then(|| keys.join(" "));
                            let tool = *tool;
                            let checked = *active;
                            menu = menu.item(
                                PopupMenuItem::element(move |_, cx| {
                                    let theme = cx.theme();
                                    let fg = if checked {
                                        theme.link
                                    } else {
                                        theme.popover_foreground
                                    };
                                    let muted = theme.muted_foreground;
                                    let shortcut = shortcut.clone();
                                    h_flex()
                                        .w(px(210.))
                                        .items_center()
                                        .justify_between()
                                        .gap_3()
                                        .child(
                                            h_flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .w(px(14.))
                                                        .flex_none()
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .children(checked.then(|| {
                                                            Icon::new(IconName::Check)
                                                                .size(px(14.))
                                                                .text_color(fg)
                                                        })),
                                                )
                                                .child(Icon::new(icon).size(px(15.)).text_color(fg))
                                                .child(div().text_color(fg).child(label)),
                                        )
                                        .when_some(shortcut, |row, text| {
                                            row.child(div().text_xs().text_color(muted).child(text))
                                        })
                                })
                                .on_click(move |_, window, cx| {
                                    if let Some(this) = this.upgrade() {
                                        this.update(cx, |composer, cx| {
                                            composer.run(tool, window, cx);
                                        });
                                    }
                                }),
                            );
                            had_items = true;
                        }
                    }
                }
                if !show_md {
                    if had_items {
                        menu = menu.separator();
                    }
                    let this = this.clone();
                    menu = menu.item(
                        PopupMenuItem::element(move |_, cx| {
                            let theme = cx.theme();
                            let fg = if markup_active {
                                theme.link
                            } else {
                                theme.popover_foreground
                            };
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .w(px(14.))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .children(markup_active.then(|| {
                                            Icon::new(IconName::Check).size(px(14.)).text_color(fg)
                                        })),
                                )
                                .child(div().text_color(fg).child("Raw markup mode (MD)"))
                        })
                        .on_click(move |_, _, cx| {
                            if let Some(this) = this.upgrade() {
                                this.update(cx, |composer, cx| {
                                    composer.set_format_markup(!composer.format_markup, cx);
                                });
                            }
                        }),
                    );
                }
                menu
            });

        div()
            .id("fmt-more-tip")
            .flex_none()
            .tooltip(|window, cx| tip_view("More options", &[], window, cx))
            .child(trigger)
            .into_any_element()
    }

    /// The `⋯` `"More options"` overflow dropdown on the bottom action row when the composer is
    /// too narrow to show clips and shortcut buttons inline.
    fn action_more_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let this = cx.entity().downgrade();
        let trigger = Button::new("act-more")
            .ghost()
            .with_size(Size::XSmall)
            .size(px(26.))
            .p_0()
            .child(
                Icon::new(IconName::Ellipsis)
                    .text_color(muted)
                    .size(px(15.)),
            )
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                let cam_this = this.clone();
                let scr_this = this.clone();
                let aud_this = this.clone();
                let slash_this = this.clone();
                menu.item(
                    PopupMenuItem::new("Record camera clip")
                        .icon(IconName::Video)
                        .on_click(move |_, _, cx| {
                            if let Some(c) = cam_this.upgrade() {
                                c.update(cx, |composer, cx| {
                                    composer.start_video_clip(
                                        slackless_core::huddle::ClipVideoSource::Camera(None),
                                        cx,
                                    );
                                });
                            }
                        }),
                )
                .item(
                    PopupMenuItem::new("Record screen clip")
                        .icon(IconName::Monitor)
                        .on_click(move |_, _, cx| {
                            if let Some(c) = scr_this.upgrade() {
                                c.update(cx, |composer, cx| {
                                    composer.start_video_clip(
                                        slackless_core::huddle::ClipVideoSource::Screen(
                                            "monitor:primary".to_owned(),
                                        ),
                                        cx,
                                    );
                                });
                            }
                        }),
                )
                .item(
                    PopupMenuItem::new("Record audio clip")
                        .icon(IconName::Mic)
                        .on_click(move |_, _, cx| {
                            if let Some(c) = aud_this.upgrade() {
                                c.update(cx, |composer, cx| {
                                    composer.start_audio_clip(cx);
                                });
                            }
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Run shortcut")
                        .icon(IconName::SquareSlash)
                        .on_click(move |_, window, cx| {
                            if let Some(c) = slash_this.upgrade() {
                                c.update(cx, |composer, cx| {
                                    composer.run(Tool::Slash, window, cx);
                                });
                            }
                        }),
                )
            });

        div()
            .id("act-more-tip")
            .flex_none()
            .tooltip(|window, cx| tip_view("More options", &[], window, cx))
            .child(trigger)
            .into_any_element()
    }

    /// A button that runs `tool`, lit while the tool is in effect at the caret. Like Slack's,
    /// the toolbar's icons are faint until the draft has focus.
    pub(super) fn tool_button(
        &self,
        slot: &Slot,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let Slot::Tool(id, icon, tool) = slot else {
            if let Slot::Styles = slot {
                return self.styles_button(focused, cx);
            }
            return div()
                .mx_1()
                .w(px(1.))
                .h(px(16.))
                .bg(theme.border)
                .into_any_element();
        };
        let tool = *tool;
        let is_active = self.is_active(tool);
        // Slack's icons are the foreground at 90% (#333235 on the light band), dimmed to 30%
        // opacity while the draft lacks focus (#bdbdbe).
        let idle = theme.foreground.opacity(0.9);
        let (color, lit, hover) = (
            if is_active {
                theme.foreground
            } else if focused {
                idle
            } else {
                idle.opacity(0.3)
            },
            theme.secondary_hover,
            theme.secondary_hover,
        );
        let tip = slot_tip(tool);
        control(id)
            .cursor_pointer()
            .when(is_active, |button| button.bg(lit))
            .hover(|button| button.bg(hover))
            .when_some(tip, |button, (label, key)| {
                button.tooltip(move |window, cx| tip_view(label, key, window, cx))
            })
            .child(Icon::new(*icon).text_color(color).size(px(15.)))
            // Keep the draft focused, and with it the selection the tool applies to.
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_click(cx.listener(move |this, _, window, cx| this.run(tool, window, cx)))
            .into_any_element()
    }

    /// The paragraph-style dropdown beside the inline styles: compact `¶`/`H1`/`H2`/`H3`
    /// icon + chevron opening Slack's Paragraph / Big / Medium / Small heading menu upward.
    fn styles_button(&self, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let active = match self.editor.active_block() {
            Some(LineKind::Heading(level)) => level,
            _ => 0,
        };
        let active_icon = match active {
            1 => IconName::Heading1,
            2 => IconName::Heading2,
            3 => IconName::Heading3,
            _ => IconName::Pilcrow,
        };
        let idle = cx.theme().foreground.opacity(0.9);
        let color = if focused { idle } else { idle.opacity(0.3) };
        let this = cx.entity().downgrade();
        Button::new("fmt-styles")
            .ghost()
            .with_size(Size::XSmall)
            .h(px(26.))
            .px_1()
            .child(
                h_flex()
                    .items_center()
                    .gap_0p5()
                    .child(Icon::new(active_icon).text_color(color).size(px(15.)))
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .text_color(color)
                            .size(px(11.)),
                    ),
            )
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |mut menu, _, _| {
                for (level, icon, choice, tool) in [
                    (0u8, IconName::Pilcrow, "Paragraph", Tool::Paragraph),
                    (1, IconName::Heading1, "Big heading", Tool::Heading(1)),
                    (2, IconName::Heading2, "Medium heading", Tool::Heading(2)),
                    (3, IconName::Heading3, "Small heading", Tool::Heading(3)),
                ] {
                    let this = this.clone();
                    let checked = active == level;
                    menu = menu.item(
                        PopupMenuItem::element(move |_, cx| {
                            let theme = cx.theme();
                            let color = if checked {
                                theme.link
                            } else {
                                theme.popover_foreground
                            };
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .w(px(14.))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .children(checked.then(|| {
                                            Icon::new(IconName::Check)
                                                .size(px(14.))
                                                .text_color(color)
                                        })),
                                )
                                .child(Icon::new(icon).size(px(15.)).text_color(color))
                                .child(div().text_color(color).child(choice))
                        })
                        .on_click(move |_, window, cx| {
                            if let Some(this) = this.upgrade() {
                                this.update(cx, |composer, cx| {
                                    composer.run(tool, window, cx);
                                });
                            }
                        }),
                    );
                }
                menu
            })
            .into_any_element()
    }

    /// Slack's "Also send to #channel" checkbox between the reply draft and the action row.
    pub(super) fn also_send_row(&self, cx: &mut Context<Self>) -> Option<Div> {
        let (target, ticked) = self.also_send.clone()?;
        let muted = cx.theme().muted_foreground;
        let label = match target {
            AlsoSendTarget::Channel { name, private } => h_flex()
                .gap_1()
                .items_center()
                .child("Also send to")
                .child(
                    Icon::new(if private {
                        IconName::Lock
                    } else {
                        IconName::Hash
                    })
                    .text_color(muted)
                    .size(px(12.)),
                )
                .child(name),
            AlsoSendTarget::DirectMessage => h_flex().child("Also send as direct message"),
        };
        Some(
            h_flex()
                .px_3()
                .gap_2()
                .items_center()
                .text_xs()
                .text_color(muted)
                .child(
                    Checkbox::new("also-send")
                        .checked(ticked)
                        .on_click(cx.listener(|this, ticked: &bool, _, cx| {
                            if let Some((_, state)) = &mut this.also_send {
                                *state = *ticked;
                            }
                            cx.notify();
                        })),
                )
                .child(label),
        )
    }

    /// The chevron beside Send: Slack's "Schedule for later" menu.
    fn schedule_button(&self, can_send: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let theme = cx.theme();
        let icon_color = if can_send {
            theme.primary_foreground
        } else {
            theme.muted_foreground
        };
        let trigger = Button::new("schedule")
            .when(can_send, |btn| btn.primary())
            .when(!can_send, |btn| btn.ghost().opacity(0.5))
            .with_size(Size::XSmall)
            .h(px(28.))
            .w(px(26.))
            .p_0()
            .rounded_none()
            .rounded_r_sm()
            .child(
                Icon::new(IconName::ChevronDown)
                    .text_color(icon_color)
                    .size(px(14.)),
            )
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |mut menu, _, _| {
                for when in SendLater::ALL {
                    let this = this.clone();
                    menu = menu.item(
                        PopupMenuItem::new(when.label())
                            .icon(IconName::Clock)
                            .on_click(move |_, window, cx| {
                                // The composer is gone only if its view was dropped mid-click.
                                if let Some(this) = this.upgrade() {
                                    this.update(cx, |composer, cx| {
                                        composer.schedule(when, window, cx);
                                    });
                                }
                            }),
                    );
                }
                let custom_this = this.clone();
                menu.separator().item(
                    PopupMenuItem::new("Custom time\u{2026}")
                        .icon(IconName::Calendar)
                        .on_click(move |_, _, cx| {
                            if let Some(this) = custom_this.upgrade() {
                                this.update(cx, |composer, cx| {
                                    composer.schedule_custom(cx);
                                });
                            }
                        }),
                )
            });
        div()
            .id("schedule-tip")
            .when(can_send, |el| {
                el.tooltip(|window, cx| tip_view("Schedule for later", &[], window, cx))
            })
            .child(trigger)
    }

    /// Slack's bottom-left `+` button (turning to `×` while open) and its upward dropdown menu:
    /// File/canvas/list (with blue `New` pill), GIF, Text snippet (`⌘⇧Enter`), Workflow, and
    /// Upload from your computer (`⌘O`). Double-clicking `+` opens the OS file picker directly.
    fn plus_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, secondary) = (theme.muted_foreground, theme.secondary);
        let plus_icon = if self.plus_open {
            IconName::X
        } else {
            IconName::Plus
        };
        let this = cx.entity().downgrade();
        let this_open = this.clone();
        let trigger = Button::new("act-plus")
            .ghost()
            .with_size(Size::XSmall)
            .size(px(26.))
            .p_0()
            .rounded_full()
            .bg(secondary)
            .child(Icon::new(plus_icon).text_color(muted).size(px(15.)))
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                let make_row = |icon: IconName,
                                label: &'static str,
                                badge: Option<&'static str>,
                                shortcut: Option<&'static str>| {
                    PopupMenuItem::element(move |_, cx| {
                        let theme = cx.theme();
                        let (fg, muted) = (theme.popover_foreground, theme.muted_foreground);
                        h_flex()
                            .w(px(260.))
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2p5()
                                    .child(Icon::new(icon).size(px(16.)).text_color(fg))
                                    .child(div().text_sm().text_color(fg).child(label)),
                            )
                            .when_some(badge, |row, text| {
                                let (bg, fg) = crate::theme::new_badge(cx);
                                row.child(
                                    div()
                                        .px_1p5()
                                        .py(px(1.))
                                        .rounded(px(4.))
                                        .bg(bg)
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg)
                                        .child(text),
                                )
                            })
                            .when_some(shortcut, |row, keys| {
                                row.child(div().text_xs().text_color(muted).child(keys))
                            })
                    })
                };
                let t1 = this.clone();
                let t2 = this.clone();
                let t3 = this.clone();
                let t4 = this.clone();
                let t5 = this.clone();
                menu.item(
                    make_row(IconName::Files, "File, canvas, or list", Some("New"), None).on_click(
                        move |_, _, cx| {
                            if let Some(this) = t1.upgrade() {
                                this.update(cx, |composer, cx| composer.pick_files(cx));
                            }
                        },
                    ),
                )
                .item(make_row(IconName::Sticker, "GIF", None, None).on_click(
                    move |_, window, cx| {
                        let pos = window.mouse_position();
                        if let Some(this) = t2.upgrade() {
                            this.update(cx, |_, cx| {
                                cx.emit(ComposerEvent::OpenGifPicker(pos));
                            });
                        }
                    },
                ))
                .item(
                    make_row(IconName::FileType, "Text snippet", None, Some("⌘⇧Enter")).on_click(
                        move |_, window, cx| {
                            if let Some(this) = t3.upgrade() {
                                this.update(cx, |composer, cx| {
                                    composer.open_text_snippet(window, cx);
                                });
                            }
                        },
                    ),
                )
                .item(make_row(IconName::Play, "Workflow", None, None).on_click(
                    move |_, window, cx| {
                        if let Some(this) = t4.upgrade() {
                            this.update(cx, |composer, cx| {
                                composer.run(Tool::Slash, window, cx);
                            });
                        }
                    },
                ))
                .item(
                    make_row(
                        IconName::LaptopMinimal,
                        "Upload from your computer",
                        None,
                        Some("⌘O"),
                    )
                    .on_click(move |_, _, cx| {
                        if let Some(this) = t5.upgrade() {
                            this.update(cx, |composer, cx| composer.pick_files(cx));
                        }
                    }),
                )
            })
            .on_open_change(move |open, _, cx| {
                if let Some(this) = this_open.upgrade() {
                    this.update(cx, |composer, cx| {
                        composer.on_plus_open_change(*open, cx);
                    });
                }
            });
        div()
            .id("act-plus-tip")
            .when(!self.plus_open, |el| {
                el.tooltip(|window, cx| {
                    tip_sub("Attach", "Tip: double click to upload", window, cx)
                })
            })
            .child(trigger)
            .into_any_element()
    }

    /// Staged attachment chips / image thumbnails / GIF preview rendered inside the composer
    /// above the action row.
    pub(super) fn attachments_strip(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.attachments.is_empty() && self.pending_gif.is_none() {
            return None;
        }
        let theme = cx.theme();
        let (border, secondary, foreground, muted, popover, popover_fg) = (
            theme.border,
            theme.secondary,
            theme.foreground,
            theme.muted_foreground,
            theme.popover,
            theme.popover_foreground,
        );
        let gif_card = self.pending_gif.as_ref().map(|gif| {
            let preview_url = gif.preview_url.clone();
            let on_remove = cx.listener(|this, _, _, cx| this.remove_gif(cx));
            crate::actions::HoverOverlay::new("staged-gif-card", move |hover, _, cx| {
                let revealed = hover.revealed(cx);
                div()
                    .id("staged-gif-card")
                    .relative()
                    .flex_none()
                    .size(px(62.))
                    .child(
                        div()
                            .size_full()
                            .rounded_lg()
                            .overflow_hidden()
                            .border_1()
                            .border_color(foreground.opacity(0.12))
                            .bg(secondary)
                            .child(
                                crate::media::Picture::new(preview_url)
                                    .fit(ObjectFit::Cover)
                                    .radius(px(8.)),
                            ),
                    )
                    .when(revealed, |card| {
                        card.child(
                            div()
                                .id("staged-gif-rm")
                                .absolute()
                                .top(px(-6.))
                                .right(px(-6.))
                                .size(px(20.))
                                .rounded_full()
                                .bg(popover)
                                .border_1()
                                .border_color(foreground.opacity(0.22))
                                .shadow_sm()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .block_mouse_except_scroll()
                                .on_hover(hover.overlay_on_hover())
                                .tooltip(|window, cx| tip_view("Remove GIF", &[], window, cx))
                                .child(Icon::new(IconName::X).size(px(12.)).text_color(popover_fg))
                                .on_click(on_remove),
                        )
                    })
                    .on_hover(hover.host_on_hover())
                    .into_any_element()
            })
        });
        Some(
            h_flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .px_3()
                .when(self.pending_gif.is_some(), |el| el.pt_1p5())
                .pb_1p5()
                .children(gif_card)
                .children(self.attachments.iter().enumerate().map(|(ix, file)| {
                    let thumb = match &file.preview {
                        Some(preview) => img(ImageSource::Image(preview.clone()))
                            .size(px(36.))
                            .rounded_sm()
                            .object_fit(ObjectFit::Cover)
                            .flex_none()
                            .into_any_element(),
                        None => {
                            let lower = file.filename.to_ascii_lowercase();
                            let icon_name = if lower.ends_with(".m4a")
                                || lower.ends_with(".wav")
                                || lower.ends_with(".mp3")
                                || lower.ends_with(".ogg")
                            {
                                IconName::Mic
                            } else if lower.ends_with(".mp4")
                                || lower.ends_with(".mov")
                                || lower.ends_with(".webm")
                            {
                                IconName::Video
                            } else {
                                IconName::File
                            };
                            div()
                                .size(px(36.))
                                .flex_none()
                                .rounded_sm()
                                .bg(secondary)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Icon::new(icon_name).size(px(18.)).text_color(muted))
                                .into_any_element()
                        }
                    };
                    h_flex()
                        .id(("attach-chip", ix))
                        .items_center()
                        .gap_2()
                        .pl_1p5()
                        .pr_1()
                        .py_1()
                        .max_w(px(220.))
                        .rounded_md()
                        .border_1()
                        .border_color(border)
                        .bg(secondary.opacity(0.6))
                        .child(thumb)
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(foreground)
                                        .child(file.filename.clone()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(super::format_bytes(file.bytes.len())),
                                ),
                        )
                        .child(
                            Button::new(("attach-rm", ix))
                                .ghost()
                                .with_size(Size::XSmall)
                                .icon(IconName::X)
                                .tooltip("Remove attachment")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_attachment(ix, cx);
                                })),
                        )
                })),
        )
    }

    /// Slack's always-visible row under the draft: attach, format, emoji, mention, clips and
    /// slash on the left; send and schedule on the right.
    pub(super) fn action_row(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (muted, border, hover) = (theme.muted_foreground, theme.border, theme.secondary_hover);
        let secondary = theme.secondary;
        let (primary, primary_foreground, primary_hover) =
            (theme.primary, theme.primary_foreground, theme.primary_hover);
        let can_send = !self.editor.doc().is_blank()
            || (!self.edit_mode && (!self.attachments.is_empty() || self.pending_gif.is_some()));
        let icon = |name: IconName| Icon::new(name).text_color(muted).size(px(15.));
        let separator = || div().w(px(1.)).h(px(16.)).rounded_full().bg(border);
        let button = |id, label: AnyElement| {
            control(id)
                .cursor_pointer()
                .text_color(muted)
                .hover(|el| el.bg(hover))
                .child(label)
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        };
        let aa = div()
            .text_sm()
            .when(self.show_toolbar, |el| el.underline())
            .child("Aa")
            .into_any_element();
        let format_tip = if self.show_toolbar {
            "Hide formatting"
        } else {
            "Show formatting"
        };
        let smiley = icon(IconName::FaceSlightlySmiling).into_any_element();
        let tools = h_flex()
            .items_center()
            .gap_0p5()
            .child(if self.edit_mode {
                control("act-plus")
                    .rounded_full()
                    .bg(secondary)
                    .text_color(muted)
                    .tooltip(|window, cx| {
                        tip_sub("Attach", "Tip: double click to upload", window, cx)
                    })
                    .child(icon(IconName::Plus))
                    .into_any_element()
            } else {
                self.plus_button(cx)
            })
            .child(
                button("act-format", aa)
                    .tooltip(move |window, cx| tip_view(format_tip, &[], window, cx))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_toolbar = !this.show_toolbar;
                        cx.notify();
                    })),
            )
            .child(
                button("act-emoji", smiley)
                    .tooltip(|window, cx| tip_view("Add emoji and GIFs", &[], window, cx))
                    .on_click(cx.listener(|_, event: &ClickEvent, _, cx| {
                        cx.emit(ComposerEvent::OpenEmojiPicker(event.position()));
                    })),
            )
            .when(!self.edit_mode, |row| {
                let collapse_tail = self.clip_recorder.is_none()
                    && self
                        .box_width
                        .is_some_and(|w| action_row_collapses_tail(w, self.scheduling));
                row.child(
                    button("act-mention", icon(IconName::AtSign).into_any_element())
                        .tooltip(|window, cx| tip_view("Mention someone", &[], window, cx))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.run(Tool::Mention, window, cx)),
                        ),
                )
                .when(collapse_tail, |row| row.child(self.action_more_button(cx)))
                .when(!collapse_tail, |row| {
                    row.child(separator())
                        .child({
                            let recording_video = matches!(
                                self.clip_recorder,
                                Some(super::ActiveClipRecorder::Video { .. })
                            );
                            let danger = cx.theme().danger;
                            let this = cx.entity().downgrade();
                            if recording_video {
                                button(
                                    "act-video",
                                    Icon::new(IconName::Square)
                                        .text_color(danger)
                                        .size(px(14.))
                                        .into_any_element(),
                                )
                                .bg(danger.opacity(0.14))
                                .tooltip(|window, cx| tip_view("Stop video clip", &[], window, cx))
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.stop_and_attach_clip(cx)),
                                )
                                .into_any_element()
                            } else {
                                Button::new("act-video")
                                    .ghost()
                                    .with_size(Size::XSmall)
                                    .size(px(28.))
                                    .p_0()
                                    .tooltip("Record video clip")
                                    .child(icon(IconName::Video))
                                    .dropdown_menu_with_anchor(
                                        Anchor::BottomLeft,
                                        move |mut menu, _, _| {
                                            let cam_this = this.clone();
                                            let scr_this = this.clone();
                                            menu = menu
                                                .item(
                                                    PopupMenuItem::new("Record camera clip")
                                                        .icon(IconName::Video)
                                                        .on_click(move |_, _, cx| {
                                                            if let Some(c) = cam_this.upgrade() {
                                                                c.update(cx, |composer, cx| {
                                                                    composer.start_video_clip(
                                                                        slackless_core::huddle::ClipVideoSource::Camera(None),
                                                                        cx,
                                                                    );
                                                                });
                                                            }
                                                        }),
                                                )
                                                .item(
                                                    PopupMenuItem::new(
                                                        "Record screen clip (primary display)",
                                                    )
                                                    .icon(IconName::Monitor)
                                                    .on_click(move |_, _, cx| {
                                                        if let Some(c) = scr_this.upgrade() {
                                                            c.update(cx, |composer, cx| {
                                                                composer.start_video_clip(
                                                                    slackless_core::huddle::ClipVideoSource::Screen(
                                                                        "monitor:primary".to_owned(),
                                                                    ),
                                                                    cx,
                                                                );
                                                            });
                                                        }
                                                    }),
                                                );
                                            let sources =
                                                slackless_core::huddle::list_screen_sources();
                                            if sources.len() > 1 {
                                                menu = menu
                                                    .separator()
                                                    .label("Record display or window");
                                                for src in sources.into_iter().take(6) {
                                                    let src_this = this.clone();
                                                    let src_id = src.id;
                                                    let icon = if src.is_monitor {
                                                        IconName::Monitor
                                                    } else {
                                                        IconName::AppWindow
                                                    };
                                                    menu = menu.item(
                                                        PopupMenuItem::new(src.name)
                                                            .icon(icon)
                                                            .on_click(move |_, _, cx| {
                                                                if let Some(c) = src_this.upgrade()
                                                                {
                                                                    c.update(cx, |composer, cx| {
                                                                        composer.start_video_clip(
                                                                            slackless_core::huddle::ClipVideoSource::Screen(
                                                                                src_id.clone(),
                                                                            ),
                                                                            cx,
                                                                        );
                                                                    });
                                                                }
                                                            }),
                                                    );
                                                }
                                            }
                                            menu
                                        },
                                    )
                                    .into_any_element()
                            }
                        })
                        .child({
                            let recording_audio = matches!(
                                self.clip_recorder,
                                Some(super::ActiveClipRecorder::Audio(_))
                            );
                            let danger = cx.theme().danger;
                            button(
                                "act-mic",
                                if recording_audio {
                                    Icon::new(IconName::Square)
                                        .text_color(danger)
                                        .size(px(14.))
                                        .into_any_element()
                                } else {
                                    icon(IconName::Mic).into_any_element()
                                },
                            )
                            .when(recording_audio, |el| el.bg(danger.opacity(0.14)))
                            .tooltip(move |window, cx| {
                                tip_view(
                                    if recording_audio {
                                        "Stop audio clip"
                                    } else {
                                        "Record audio clip"
                                    },
                                    &[],
                                    window,
                                    cx,
                                )
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.start_audio_clip(cx)))
                        })
                        .child(separator())
                        .child(
                            button("act-slash", icon(IconName::SquareSlash).into_any_element())
                                .tooltip(|window, cx| tip_view("Run shortcut", &[], window, cx))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.run(Tool::Slash, window, cx)
                                })),
                        )
                })
            });
        let right = if self.edit_mode {
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    Button::new("edit-cancel")
                        .outline()
                        .with_size(Size::Small)
                        .label("Cancel")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ComposerEvent::Cancel))),
                )
                .child(
                    Button::new("edit-save")
                        .primary()
                        .with_size(Size::Small)
                        .label("Save")
                        .disabled(!can_send)
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                )
        } else {
            let send_keys: &'static [&'static str] = if self.enter_sends {
                &["Return"]
            } else {
                &["⌘", "Return"]
            };
            let scheduling = self.scheduling;
            let send = div()
                .id("send")
                .flex_none()
                .h(px(28.))
                .w(px(32.))
                .when(scheduling, |el| el.rounded_l_sm())
                .when(!scheduling, |el| el.rounded_sm())
                .flex()
                .items_center()
                .justify_center()
                .map(|send| {
                    if can_send {
                        send.cursor_pointer()
                            .bg(primary)
                            .hover(move |el| el.bg(primary_hover))
                            .tooltip(move |window, cx| tip_view("Send now", send_keys, window, cx))
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                    } else {
                        send.opacity(0.5)
                    }
                })
                .child(
                    Icon::new(IconName::Send)
                        .text_color(if can_send { primary_foreground } else { muted })
                        .size(px(15.)),
                );
            h_flex().items_center().child(send).when(scheduling, |row| {
                row.child(div().w(px(1.)).h(px(28.)).bg(if can_send {
                    primary_foreground.opacity(0.25)
                } else {
                    border.opacity(0.4)
                }))
                .child(self.schedule_button(can_send, cx))
            })
        };
        h_flex()
            .items_center()
            .justify_between()
            .px_2()
            .pb_1p5()
            .pt_1()
            .child(tools)
            .child(right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_matches_slack_twelve_controls_and_two_dividers() {
        let dividers = TOOLBAR
            .iter()
            .filter(|slot| matches!(slot, Slot::Divider))
            .count();
        let controls = TOOLBAR.len() - dividers;
        assert_eq!(controls, 12);
        assert_eq!(dividers, 2);
        // Dividers sit after Styles (index 5) and after Bulleted list (index 9).
        assert!(matches!(TOOLBAR[4], Slot::Styles));
        assert!(matches!(TOOLBAR[5], Slot::Divider));
        assert!(matches!(TOOLBAR[9], Slot::Divider));
        // Trailing tool is Divider (SeparatorHorizontal), not Clear formatting.
        assert!(matches!(
            TOOLBAR[13],
            Slot::Tool("fmt-divider", IconName::SeparatorHorizontal, Tool::Divider)
        ));
    }

    #[test]
    fn hover_popovers_cover_all_eleven_toolbar_tools() {
        let tipped: Vec<(&'static str, &'static [&'static str])> = TOOLBAR
            .iter()
            .filter_map(|slot| match slot {
                Slot::Tool(_, _, tool) => slot_tip(*tool),
                Slot::Divider | Slot::Styles => None,
            })
            .collect();
        assert_eq!(
            tipped,
            [
                ("Bold", &["⌘", "B"][..]),
                ("Italic", &["⌘", "I"][..]),
                ("Underline", &["⌘", "U"][..]),
                ("Strikethrough", &["⌘", "Shift", "X"][..]),
                ("Link", &["⌘", "Shift", "U"][..]),
                ("Ordered list", &["⌘", "Shift", "7"][..]),
                ("Bulleted list", &["⌘", "Shift", "8"][..]),
                ("Blockquote", &["⌘", "Shift", "9"][..]),
                ("Code", &["⌘", "Shift", "C"][..]),
                ("Code block", &["⌘", "Option", "Shift", "C"][..]),
                ("Divider", &[][..]),
            ]
        );
    }

    #[test]
    fn toolbar_truncates_cleanly_without_dangling_dividers() {
        // Wide composer fits every slot plus the inline MD toggle with no overflow.
        assert_eq!(visible_toolbar_layout(600.0), (TOOLBAR.len(), true));

        // Medium composer (~360px) keeps MD inline and collapses trailing slots behind `⋯`.
        let (mid_count, mid_md) = visible_toolbar_layout(360.0);
        assert!(mid_md);
        assert!(mid_count > 5 && mid_count < TOOLBAR.len());
        assert!(!matches!(TOOLBAR[mid_count - 1], Slot::Divider));

        // Narrow drawer composer (~284px) moves MD into the overflow menu and keeps ~7-8 tools.
        let (narrow_count, narrow_md) = visible_toolbar_layout(284.0);
        assert!(!narrow_md);
        assert!((7..=9).contains(&narrow_count));
        assert!(!matches!(TOOLBAR[narrow_count - 1], Slot::Divider));

        // Sweep widths to guarantee we never leave a trailing divider before `⋯`.
        for width in (100..=500).step_by(5) {
            let (count, _) = visible_toolbar_layout(width as f32);
            if count > 0 && count < TOOLBAR.len() {
                assert!(
                    !matches!(TOOLBAR[count - 1], Slot::Divider),
                    "dangling trailing divider at width {width}"
                );
            }
        }
    }

    #[test]
    fn action_row_collapses_tail_at_narrow_widths() {
        assert!(action_row_collapses_tail(280.0, true));
        assert!(!action_row_collapses_tail(420.0, true));
        // Without the schedule chevron, the row fits slightly narrower boxes.
        assert!(!action_row_collapses_tail(280.0, false));
        assert!(action_row_collapses_tail(250.0, false));
    }
}
