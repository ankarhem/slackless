//! The composer's controls: the formatting toolbar (buttons lit for what is in effect at
//! the caret) and Slack's action row with the send button. Icons come from the bundled
//! Lucide set.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, AnyElement, AnyView, App, ClickEvent, Context, Div, InteractiveElement as _,
    IntoElement, Keystroke, MouseButton, ParentElement as _, Stateful,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
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

/// Slack's grouping: text style | link | lists | quote and code | clear.
pub(super) const TOOLBAR: [Slot; 16] = [
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
    Slot::Divider,
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
    Slot::Divider,
    Slot::Tool("fmt-clear", IconName::RemoveFormatting, Tool::Clear),
];

/// The popover over a toolbar button: Slack's label and its shortcut, when the tool has one.
fn slot_tip(tool: Tool) -> Option<(&'static str, &'static str)> {
    Some(match tool {
        Tool::Style(style) if style == Style::BOLD => ("Bold", "cmd-b"),
        Tool::Style(style) if style == Style::ITALIC => ("Italic", "cmd-i"),
        Tool::Style(style) if style == Style::UNDERLINE => ("Underline", "cmd-u"),
        Tool::Style(style) if style == Style::STRIKE => ("Strikethrough", "cmd-shift-x"),
        Tool::Style(style) if style == Style::CODE => ("Code", "cmd-shift-c"),
        Tool::Link => ("Link", "cmd-shift-u"),
        Tool::Block(LineKind::Ordered) => ("Ordered list", "cmd-shift-7"),
        Tool::Block(LineKind::Bullet) => ("Bulleted list", "cmd-shift-8"),
        Tool::Block(LineKind::Quote) => ("Blockquote", "cmd-shift-9"),
        Tool::Block(LineKind::Code) => ("Code block", "cmd-alt-shift-c"),
        Tool::Clear => ("Clear formatting", "cmd-\\"),
        Tool::Style(_)
        | Tool::Block(LineKind::Soft | LineKind::Heading(_))
        | Tool::Mention
        | Tool::Slash
        | Tool::Heading(_)
        | Tool::Paragraph => return None,
    })
}

/// The popover itself: the label, with the shortcut as a key chip beside it.
fn tip_view(label: &'static str, key: &'static str, window: &mut Window, cx: &mut App) -> AnyView {
    let kbd = Keystroke::parse_with_os(key, cx.platform_info().os)
        .ok()
        .map(Kbd::new);
    Tooltip::new(label).key_binding(kbd).build(window, cx)
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

    /// The paragraph-style dropdown beside the inline styles: Slack's Paragraph / Big / Medium
    /// / Small heading picker.
    fn styles_button(&self, _focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let active = match self.editor.active_block() {
            Some(LineKind::Heading(level)) => level,
            _ => 0,
        };
        let label = match active {
            1 => "Big heading",
            2 => "Medium heading",
            3 => "Small heading",
            _ => "Paragraph",
        };
        let this = cx.entity().downgrade();
        Button::new("fmt-styles")
            .ghost()
            .with_size(Size::XSmall)
            .label(label)
            .icon(IconName::ChevronDown)
            .tooltip("Text style")
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |mut menu, _, _| {
                for (choice, tool) in [
                    ("Paragraph", Tool::Paragraph),
                    ("Big heading", Tool::Heading(1)),
                    ("Medium heading", Tool::Heading(2)),
                    ("Small heading", Tool::Heading(3)),
                ] {
                    let this = this.clone();
                    let checked = matches!(
                        (choice, active),
                        ("Paragraph", 0)
                            | ("Big heading", 1)
                            | ("Medium heading", 2)
                            | ("Small heading", 3)
                    );
                    menu = menu.item(PopupMenuItem::new(choice).checked(checked).on_click(
                        move |_, window, cx| {
                            if let Some(this) = this.upgrade() {
                                this.update(cx, |composer, cx| {
                                    composer.run(tool, window, cx);
                                });
                            }
                        },
                    ));
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
    fn schedule_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        Button::new("schedule")
            .ghost()
            .with_size(Size::XSmall)
            .icon(IconName::ChevronDown)
            .tooltip("Schedule for later")
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
                menu
            })
    }

    /// Slack's always-visible row under the draft: attach, format, emoji, mention, clips and
    /// slash on the left; send and schedule on the right.
    pub(super) fn action_row(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (muted, border, hover) = (theme.muted_foreground, theme.border, theme.secondary_hover);
        let secondary = theme.secondary;
        let (primary, primary_foreground) = (theme.primary, theme.primary_foreground);
        let can_send = !self.editor.doc().is_blank();
        let icon = |name: IconName| Icon::new(name).text_color(muted).size(px(15.));
        let separator = || div().w(px(1.)).h(px(16.)).rounded_full().bg(border);
        // Window chrome: Slack's controls for features slackless does not have yet.
        let chrome = |id, label: AnyElement| control(id).text_color(muted).child(label);
        let button = |id, label: AnyElement| {
            control(id)
                .cursor_pointer()
                .text_color(muted)
                .hover(|el| el.bg(hover))
                .child(label)
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
        };
        let aa = div().text_sm().underline().child("Aa").into_any_element();
        let smiley = icon(IconName::FaceSlightlySmiling).into_any_element();
        let tools = h_flex()
            .items_center()
            .gap_0p5()
            .child(
                chrome("act-plus", icon(IconName::Plus).into_any_element())
                    .rounded_full()
                    .bg(secondary),
            )
            .child(chrome("act-format", aa))
            .child(button("act-emoji", smiley).on_click(cx.listener(
                |_, event: &ClickEvent, _, cx| {
                    cx.emit(ComposerEvent::OpenEmojiPicker(event.position()));
                },
            )))
            .child(
                button("act-mention", icon(IconName::AtSign).into_any_element()).on_click(
                    cx.listener(|this, _, window, cx| this.run(Tool::Mention, window, cx)),
                ),
            )
            .child(separator())
            .child(chrome(
                "act-video",
                icon(IconName::Video).into_any_element(),
            ))
            .child(chrome("act-mic", icon(IconName::Mic).into_any_element()))
            .child(separator())
            .child(
                button("act-slash", icon(IconName::SquareSlash).into_any_element())
                    .on_click(cx.listener(|this, _, window, cx| this.run(Tool::Slash, window, cx))),
            );
        let send = div()
            .id("send")
            .flex_none()
            .size(px(28.))
            .rounded_sm()
            .flex()
            .items_center()
            .justify_center()
            .map(|send| {
                if can_send {
                    send.cursor_pointer()
                        .bg(primary)
                        .hover(|el| el.opacity(0.9))
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                } else {
                    send.opacity(0.5)
                }
            })
            .child(
                Icon::new(IconName::Send)
                    .text_color(if can_send { primary_foreground } else { muted })
                    .size(px(16.)),
            );
        h_flex()
            .items_center()
            .justify_between()
            .px_2()
            .pb_1p5()
            .pt_1()
            .child(tools)
            .child(
                h_flex()
                    .items_center()
                    .gap_0p5()
                    .child(send)
                    .when(self.scheduling, |row| row.child(self.schedule_button(cx))),
            )
    }
}
