//! Docked Canvas Comments drawer (`F-COLLAB-197`, `F-COLLAB-199`).

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::{
    App, Div, FontWeight, ParentElement as _, SharedString, Styled as _, div, px,
};

use crate::canvas::{CanvasUiState, mutate_ui};

pub(super) fn render_comments_drawer(key: &str, ui: &CanvasUiState, cx: &App) -> Div {
    let theme = cx.theme();
    let key_close = key.to_owned();
    let key_submit = key.to_owned();
    let input_opt = ui.comment_input.clone();

    let mut body = v_flex().flex_1().min_h_0().gap_2();
    if ui.comments.is_empty() {
        body = body.child(
            div()
                .py_4()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(
                    "No comments on this canvas yet. Add a note below to discuss sections with teammates.",
                ),
        );
    } else {
        for (author, text) in &ui.comments {
            body = body.child(
                v_flex()
                    .gap_0p5()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.secondary.opacity(0.3))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(author.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(text.clone()),
                    ),
            );
        }
    }

    let composer = input_opt.map(|input| {
        let input_send = input.clone();
        v_flex()
            .gap_1p5()
            .pt_2()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .px_2()
                    .py_1()
                    .child(Textarea::new(&input).appearance(false).bordered(false)),
            )
            .child(
                h_flex().justify_end().child(
                    Button::new(SharedString::from(format!("canvas-comment-post-{key}")))
                        .primary()
                        .with_size(Size::XSmall)
                        .label("Comment")
                        .on_click(move |_, window, cx| {
                            let body_text = input_send.read(cx).value().trim().to_owned();
                            if body_text.is_empty() {
                                return;
                            }
                            input_send.update(cx, |st, cx| st.set_value(String::new(), window, cx));
                            mutate_ui(
                                &key_submit,
                                |s| s.comments.push(("You".to_owned(), body_text)),
                                window,
                                cx,
                            );
                        }),
                ),
            )
    });

    v_flex()
        .w(px(300.))
        .flex_none()
        .h_full()
        .border_l_1()
        .border_color(theme.border)
        .bg(theme.background)
        .p_3()
        .gap_3()
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .pb_2()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            Icon::new(IconName::MessageSquare)
                                .size(px(14.))
                                .text_color(theme.muted_foreground),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Comments"),
                        ),
                )
                .child(
                    Button::new(SharedString::from(format!("canvas-comments-close-{key}")))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::X)
                        .on_click(move |_, window, cx| {
                            mutate_ui(&key_close, |s| s.comments_open = false, window, cx);
                        }),
                ),
        )
        .child(body)
        .children(composer)
}
