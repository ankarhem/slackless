//! Generic file attachment cards and external provider preview cards (`F-COLLAB-586`).

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, Div, FontWeight, ImageSource, InteractiveElement as _, IntoElement, MouseButton,
    ObjectFit, ParentElement as _, RenderOnce, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
};
use slackless_core::types::File;

use crate::fixtures::Fixtures;
use crate::image_viewer::{ImageClick, ImageOrigin};

use super::meta::{
    CARD_W, download_url, external_subtitle, file_icon, file_name, human_size, pdf_thumb_url,
};

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

#[derive(IntoElement)]
struct PdfPreviewCard {
    file: File,
    ix: usize,
    thumb_source: Option<ImageSource>,
    on_open: Option<ImageClick>,
}

impl RenderOnce for PdfPreviewCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let file = self.file;
        let name = file_name(&file);
        let dl_url = download_url(&file);
        let id_str = file.id.to_string();

        let local_thumb = crate::media::cached_pdf_inline_thumb(&id_str, cx);
        let preview_source = local_thumb.map(ImageSource::Image).or(self.thumb_source);
        if preview_source.is_none()
            && let Some(url) = dl_url.clone()
        {
            crate::media::request_pdf_inline_thumb(id_str.clone(), url, window, cx);
        }

        let page_count = crate::media::cached_pdf_page_count(&id_str, cx);
        let (icon, tile) = file_icon(&file, cx);
        let theme = cx.theme();
        let mut meta_parts = vec!["PDF".to_owned()];
        if let Some(count) = page_count {
            meta_parts.push(if count == 1 {
                "1 page".to_owned()
            } else {
                format!("{count} pages")
            });
        }
        if let Some(size) = file.size {
            meta_parts.push(human_size(size));
        }
        let meta = meta_parts.join(" \u{b7} ");

        let save_as = dl_url.clone().map(|url| {
            let name = name.clone();
            div()
                .id(SharedString::from(format!("pdf-saveas-{}", file.id)))
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
                    cx.stop_propagation();
                    crate::media::download_as(url.clone(), name.clone(), window, cx);
                })
        });
        let dl_btn = dl_url.clone().map(|url| {
            let name = name.clone();
            div()
                .id(SharedString::from(format!("pdf-dl-{}", file.id)))
                .size(px(26.))
                .flex_none()
                .rounded_sm()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|b| b.bg(theme.secondary_hover))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Icon::new(IconName::Download)
                        .text_color(theme.muted_foreground)
                        .size(px(15.)),
                )
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    crate::media::download(url.clone(), name.clone(), window, cx);
                })
        });

        let mut card = v_flex()
            .id(SharedString::from(format!("pdf-card-{}", file.id)))
            .w(px(CARD_W))
            .max_w_full()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.muted)
            .overflow_hidden()
            .when_some(preview_source, |col, src| {
                col.child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(176.))
                        .bg(theme.secondary)
                        .border_b_1()
                        .border_color(theme.border)
                        .overflow_hidden()
                        .child(
                            crate::media::Picture::new(src)
                                .fit(ObjectFit::Contain)
                                .radius(px(0.)),
                        )
                        .child(
                            div()
                                .absolute()
                                .top(px(8.))
                                .left(px(8.))
                                .px_1p5()
                                .py(px(1.))
                                .rounded(px(4.))
                                .bg(crate::theme::media_overlay_scrim_strong())
                                .text_size(px(10.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(crate::theme::media_overlay_foreground())
                                .child("PDF"),
                        ),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_3()
                    .p_2()
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
                                    .text_color(theme.foreground)
                                    .child(name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(meta),
                            ),
                    )
                    .children(save_as)
                    .children(dl_btn),
            );

        if let Some(on_open) = self.on_open {
            let origin = ImageOrigin::File(self.ix);
            card = card
                .cursor_pointer()
                .hover(|c| c.bg(theme.secondary))
                .on_click(move |_, window, cx| on_open(origin.clone(), window, cx));
        } else if let Some(url) = dl_url {
            card = card
                .cursor_pointer()
                .hover(|c| c.bg(theme.secondary))
                .on_click(move |_, window, cx| {
                    crate::media::download(url.clone(), name.clone(), window, cx);
                });
        }

        div().child(card)
    }
}

/// Interactive PDF attachment card: shows page 1 preview thumbnail (from Slack or native macOS
/// CoreGraphics cache) and clicks directly into [`crate::image_viewer::ImageViewer`] multi-page
/// preview when `on_open` is present.
pub(super) fn pdf_card(
    fixtures: &Fixtures,
    file: &File,
    ix: usize,
    on_open: Option<&ImageClick>,
) -> Div {
    let thumb_source = match fixtures.thumbnails.get(&file.id) {
        Some(image) => Some(ImageSource::Image(image.clone())),
        None => pdf_thumb_url(file).map(|u| ImageSource::from(SharedString::from(u))),
    };
    div().child(PdfPreviewCard {
        file: file.clone(),
        ix,
        thumb_source,
        on_open: on_open.cloned(),
    })
}
