//! The composer's frame: the formatting toolbar over the draft over Slack's action row
//! (both in `toolbar.rs`), with the completion menu or the link prompt floating above.

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::{ActiveTheme as _, Icon, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, Context, Div, Focusable as _, FontWeight, HighlightStyle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Stateful, Styled as _, StyledText,
    Window, div, px, relative, rems,
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
        let also_send = self.also_send_row(cx);
        let actions = self.action_row(cx);
        let overlay = if self.link_open {
            Some(self.link_panel(cx))
        } else {
            self.menu.as_ref().map(|menu| self.menu_panel(menu, cx))
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
        // Slack's private-channel prompt, "Message 🔒name", over the empty draft exactly where
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
                            .gap_px()
                            .child(Icon::new(IconName::Lock).size(px(12.)).text_color(muted))
                            .child(name),
                    )
            });
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
                    .children(toolbar)
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .min_w_0()
                            .px_2()
                            .py_1()
                            .child(draft)
                            .children(private_prompt),
                    )
                    .children(also_send)
                    .child(actions),
            )
            .when(!self.edit_mode, |el| el.child(self.notification_bar(cx)))
    }
}

impl Composer {
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
