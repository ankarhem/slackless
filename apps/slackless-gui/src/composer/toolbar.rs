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
    Anchor, AnyElement, AnyView, App, ClickEvent, Context, Div, FontWeight, ImageSource,
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
                                .occlude()
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
                        None => div()
                            .size(px(36.))
                            .flex_none()
                            .rounded_sm()
                            .bg(secondary)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::File).size(px(18.)).text_color(muted))
                            .into_any_element(),
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
        // Window chrome: Slack's controls for features slackless does not have yet.
        let chrome = |id, label: AnyElement, tip: &'static str| {
            control(id)
                .text_color(muted)
                .tooltip(move |window, cx| tip_view(tip, &[], window, cx))
                .child(label)
        };
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
                row.child(
                    button("act-mention", icon(IconName::AtSign).into_any_element())
                        .tooltip(|window, cx| tip_view("Mention someone", &[], window, cx))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.run(Tool::Mention, window, cx)),
                        ),
                )
                .child(separator())
                .child(chrome(
                    "act-video",
                    icon(IconName::Video).into_any_element(),
                    "Record video clip",
                ))
                .child(chrome(
                    "act-mic",
                    icon(IconName::Mic).into_any_element(),
                    "Record audio clip",
                ))
                .child(separator())
                .child(
                    button("act-slash", icon(IconName::SquareSlash).into_any_element())
                        .tooltip(|window, cx| tip_view("Run shortcut", &[], window, cx))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.run(Tool::Slash, window, cx)),
                        ),
                )
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
}
