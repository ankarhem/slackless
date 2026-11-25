//! `"Create a text snippet"` dialog for staging code/text snippets as attachments.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::gpui::{
    AppContext as _, Context, Focusable as _, FontWeight, InteractiveElement as _,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use super::{Composer, PendingAttachment};

const SNIPPET_LANGUAGES: [(&str, &str); 9] = [
    ("Plain text", "txt"),
    ("Rust", "rs"),
    ("TypeScript", "ts"),
    ("Python", "py"),
    ("JSON", "json"),
    ("SQL", "sql"),
    ("Bash", "sh"),
    ("Markdown", "md"),
    ("Go", "go"),
];

/// Replace or append `ext` on a snippet filename, defaulting empty stems to `"snippet"`.
pub(super) fn apply_snippet_extension(current: &str, ext: &str) -> String {
    let cur = current.trim();
    let stem = cur.rsplit_once('.').map(|(s, _)| s).unwrap_or(cur);
    let stem = if stem.is_empty() { "snippet" } else { stem };
    format!("{stem}.{ext}")
}

/// Normalize a user-entered snippet filename, defaulting blank names to `"snippet.txt"` and
/// appending `.txt` when no extension is present.
pub(super) fn normalize_snippet_filename(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        "snippet.txt".to_owned()
    } else if trimmed.contains('.') {
        trimmed.to_owned()
    } else {
        format!("{trimmed}.txt")
    }
}

impl Composer {
    /// Open Slack's `"Create a text snippet"` dialog (`+` -> `Text snippet` or `Cmd+Shift+Enter`),
    /// staging the snippet as an attached text/code file ready to upload.
    pub(super) fn open_text_snippet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit_mode {
            return;
        }
        let this = cx.entity().downgrade();
        let filename = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("snippet.txt")
                .auto_grow(1, 1)
        });
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Paste or type your snippet...")
                .auto_grow(6, 14)
        });
        body.read(cx).focus_handle(cx).focus(window, cx);
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (border, muted, bg, secondary, fg) = (
                theme.border,
                theme.muted_foreground,
                theme.background,
                theme.secondary,
                theme.foreground,
            );
            let (filename, body, this) = (filename.clone(), body.clone(), this.clone());
            let lang_pills =
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .children(
                        SNIPPET_LANGUAGES
                            .iter()
                            .enumerate()
                            .map(|(ix, &(label, ext))| {
                                let filename = filename.clone();
                                div()
                                    .id(("snippet-lang", ix))
                                    .px_2()
                                    .py_0p5()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(border)
                                    .bg(secondary)
                                    .text_xs()
                                    .text_color(fg)
                                    .cursor_pointer()
                                    .hover(|el| el.bg(theme.accent))
                                    .child(label)
                                    .on_click(move |_, window, cx| {
                                        filename.update(cx, |state, cx| {
                                            let next = apply_snippet_extension(&state.value(), ext);
                                            state.set_value(next, window, cx);
                                        });
                                    })
                            }),
                    );
            dialog
                .title("Create a text or code snippet")
                .w(px(540.))
                .child(
                    v_flex()
                        .gap_3()
                        .p_4()
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child("Filename"),
                                )
                                .child(
                                    crate::preferences::text_input_box(&filename, false, cx).bg(bg),
                                )
                                .child(lang_pills),
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(muted)
                                        .child("Content"),
                                )
                                .child(crate::preferences::text_input_box(&body, true, cx).bg(bg)),
                        )
                        .child(
                            h_flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    crate::preferences::form_button(
                                        Button::new("snippet-cancel").outline().label("Cancel"),
                                        cx,
                                    )
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    crate::preferences::form_button(
                                        Button::new("snippet-create")
                                            .primary()
                                            .label("Create snippet"),
                                        cx,
                                    )
                                    .on_click(
                                        move |_, window, cx| {
                                            let content = body.read(cx).value().to_string();
                                            if content.trim().is_empty() {
                                                return;
                                            }
                                            let name = normalize_snippet_filename(
                                                &filename.read(cx).value(),
                                            );
                                            if let Some(this) = this.upgrade() {
                                                this.update(cx, |composer, cx| {
                                                    composer.attachments.push(PendingAttachment {
                                                        filename: name,
                                                        bytes: content.into_bytes(),
                                                        preview: None,
                                                    });
                                                    cx.notify();
                                                });
                                            }
                                            window.close_dialog(cx);
                                        },
                                    ),
                                ),
                        ),
                )
        });
    }
}
