//! The composer's frame: the formatting toolbar over the draft over Slack's action row
//! (both in `toolbar.rs`), with the completion menu or the link prompt floating above.

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::{
    Context, Div, Focusable as _, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, Stateful, Styled as _, Window, div, px, relative,
};

use super::tokens::{self, Palette};
use super::toolbar::TOOLBAR;
use super::{Composer, Placeholder};

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
        let this = cx.entity();
        if self.is_stale {
            window.defer(cx, move |window, cx| {
                this.update(cx, |composer, cx| composer.reset(window, cx));
            });
        } else if self.palette.is_some_and(|drawn| drawn != Palette::of(cx)) {
            window.defer(cx, move |_, cx| {
                this.update(cx, |composer, cx| composer.restyle(cx));
            });
        }

        // Slack's formatting bar: a shaded band across the top of the box.
        let focused = self.input.focus_handle(cx).is_focused(window);
        let band = cx.theme().secondary;
        let toolbar = h_flex()
            .items_center()
            .gap_0p5()
            .px_1p5()
            .py_1()
            .rounded_t_lg()
            .bg(band)
            .children(
                TOOLBAR
                    .iter()
                    .map(|slot| self.tool_button(slot, focused, cx)),
            );
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
        // Slack's private-channel prompt, "Message 🔒name", over the empty draft where the
        // textarea's own (plain-text) placeholder would sit.
        let muted = cx.theme().muted_foreground;
        let private_prompt = self
            .private_placeholder
            .clone()
            .filter(|_| self.editor.doc().is_empty())
            .map(|name| {
                h_flex()
                    .absolute()
                    .top(px(4.))
                    .left(px(8.))
                    .items_center()
                    .gap_1()
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
            .px_4()
            .pb_3()
            .capture_action(cx.listener(Self::on_enter))
            .capture_action(cx.listener(Self::on_up))
            .capture_action(cx.listener(Self::on_down))
            .capture_action(cx.listener(Self::on_left))
            .capture_action(cx.listener(Self::on_tab))
            .capture_action(cx.listener(Self::on_escape))
            .capture_action(cx.listener(Self::on_undo))
            .capture_action(cx.listener(Self::on_redo))
            .capture_key_down(cx.listener(Self::on_key))
            .children(overlay)
            .child(
                v_flex()
                    .w_full()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.input)
                    .bg(theme.background)
                    .child(toolbar)
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
    }
}
