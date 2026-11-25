//! Generic file attachment cards and external provider preview cards (`F-COLLAB-586`).

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::{
    App, Div, FontWeight, InteractiveElement as _, MouseButton, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px,
};
use slackless_core::types::File;

use super::meta::{CARD_W, download_url, external_subtitle, file_icon, file_name, human_size};

/// Compact external file title preview card (`F-COLLAB-586`): shows provider badge, clickable
/// title, and an open-in-browser affordance.
pub(super) fn external_file_card(file: &File, cx: &App) -> Div {
    let theme = cx.theme();
    let (icon, tile) = file_icon(file, cx);
    let subtitle = external_subtitle(file);
    let open_url = file
        .permalink
        .clone()
        .or_else(|| file.url_private.clone())
        .or_else(|| file.permalink_public.clone());
    let mut card = h_flex()
        .id(SharedString::from(format!("ext-file-card-{}", file.id)))
        .w(px(CARD_W))
        .max_w_full()
        .items_center()
        .gap_3()
        .p_2()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .child(
            div()
                .size(px(36.))
                .flex_none()
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .bg(tile)
                .child(
                    Icon::new(icon)
                        .text_color(crate::theme::avatar_foreground())
                        .size(px(18.)),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.link)
                        .child(file_name(file)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(subtitle),
                ),
        )
        .child(
            Icon::new(IconName::ExternalLink)
                .text_color(theme.muted_foreground)
                .size(px(15.)),
        );
    if let Some(url) = open_url {
        card = card
            .cursor_pointer()
            .hover(|c| c.bg(theme.secondary))
            .on_click(move |_, _, cx| cx.open_url(&url));
    }
    div().child(card)
}

pub(super) fn file_card(file: &File, cx: &App) -> Div {
    let theme = cx.theme();
    let (icon, tile) = file_icon(file, cx);
    let kind = file
        .pretty_type
        .clone()
        .or_else(|| file.filetype.as_ref().map(|t| t.to_uppercase()))
        .unwrap_or_else(|| "File".to_owned());
    let meta = match file.size {
        Some(size) => format!("{kind} · {}", human_size(size)),
        None => kind,
    };
    let dl_url = download_url(file);
    let name = file_name(file);
    let card = h_flex()
        .id(SharedString::from(format!("file-card-{}", file.id)))
        .w(px(CARD_W))
        .max_w_full()
        .items_center()
        .gap_3()
        .p_2()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .child(
            div()
                .size(px(40.))
                .flex_none()
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .bg(tile)
                .child(
                    Icon::new(icon)
                        .text_color(crate::theme::avatar_foreground())
                        .size(px(20.)),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(file_name(file)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(meta),
                ),
        );
    let save_as = dl_url.clone().map(|url| {
        let name = name.clone();
        div()
            .id(SharedString::from(format!("file-saveas-{}", file.id)))
            .px_1p5()
            .py_0p5()
            .rounded_sm()
            .text_xs()
            .text_color(theme.muted_foreground)
            .cursor_pointer()
            .hover(|b| b.bg(theme.secondary_hover).text_color(theme.foreground))
            .child("Save as\u{2026}")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| {
                crate::media::download_as(url.clone(), name.clone(), window, cx);
            })
    });
    let card = card.children(save_as).child(
        Icon::new(IconName::Download)
            .text_color(theme.muted_foreground)
            .size(px(16.)),
    );
    let card = match dl_url {
        Some(url) => card
            .cursor_pointer()
            .hover(|c| c.bg(theme.secondary))
            .on_click(move |_, window, cx| {
                crate::media::download(url.clone(), name.clone(), window, cx);
            }),
        None => card,
    };
    div().child(card)
}
