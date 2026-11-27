//! The viewer's layout: the header (uploader chip, close X), the body (the stage between the
//! prev/next arrows, with the transcript drawer at its right) and the footer (the zoom controls
//! or the video transport, a PDF's pager, Download).

use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    ScrollDelta, ScrollEvent, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, px,
};

use crate::avatars;
use crate::theme::{self, ViewerChrome};

use super::item::relative_stamp;
use super::video::VideoState;
use super::{ARROW_GUTTER, CONTROL_BUTTON, ImageViewer, ViewerEvent, ViewerMedia};

impl Render for ImageViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = theme::viewer_chrome();
        let video = self.video_state();
        div()
            .id("image-viewer")
            .occlude()
            .on_scroll(cx.listener(|viewer, event: &ScrollEvent, window, cx| {
                cx.stop_propagation();
                let dy = match event.delta {
                    ScrollDelta::Pixels(p) => f32::from(p.y),
                    ScrollDelta::Lines(p) => p.y * 24.0,
                };
                if dy.abs() >= 12.0 {
                    viewer.zoom_step(if dy > 0.0 { 1 } else { -1 }, window, cx);
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(chrome.panel)
            // Header: chip top-left, close X top-right.
            .child(self.render_header(chrome, cx))
            // The fitted rendition; prev/next arrows hang at the edges of the free area,
            // with an optional right-side transcript drawer for videos.
            .child(self.render_body(chrome, &video, cx))
            // Controls: zoom slider bottom-left (images) or centered video transport bar
            // (videos), Download bottom-right.
            .child(self.render_footer(chrome, &video, cx))
    }
}

impl ImageViewer {
    /// The uploader's name (Slack shows the file's uploader on the chip).
    fn uploader_name(&self) -> String {
        match self.current().user.as_ref() {
            Some(user) => self.fixtures.user_name(user).to_owned(),
            None => "Unknown".to_owned(),
        }
    }

    /// The header: the uploader chip ("<relative time> in # <channel> - <file name>") top-left
    /// and the close X top-right.
    fn render_header(&self, chrome: ViewerChrome, cx: &mut Context<Self>) -> impl IntoElement {
        let image = self.current();
        let name = SharedString::from(image.name.clone());
        let stamp = image
            .taken_at
            .map(relative_stamp)
            .unwrap_or_else(|| "uploaded".to_owned());
        let chip = h_flex()
            .min_w_0()
            .items_center()
            .gap_3()
            .pl(px(10.))
            .pr(px(14.))
            .py(px(8.))
            .rounded_lg()
            .bg(chrome.glass)
            .when_some(image.user.clone(), |chip, user| {
                chip.child(avatars::user_chip(&self.fixtures, &user, 36.))
            })
            .child(
                v_flex()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(chrome.text)
                            .truncate()
                            .child(self.uploader_name()),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_1()
                            .text_xs()
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(chrome.text_muted)
                                    .child(SharedString::from(format!("{stamp} in"))),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(chrome.text_muted)
                                    .child(SharedString::from(self.channel.clone())),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .max_w(px(280.))
                                    .text_color(chrome.text_dim)
                                    .child(SharedString::from(format!("- {name}"))),
                            ),
                    ),
            );
        h_flex()
            .items_start()
            .justify_between()
            .flex_none()
            .px(px(12.))
            .pt(px(12.))
            .pb(px(4.))
            .child(chip)
            .child(
                div()
                    .id("viewer-close")
                    .flex_none()
                    .size(px(CONTROL_BUTTON))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(chrome.button)
                    .border_1()
                    .border_color(chrome.border)
                    .cursor_pointer()
                    .hover(move |button| button.bg(chrome.button_hover))
                    .child(Icon::new(IconName::X).text_color(chrome.text).size(px(16.)))
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(ViewerEvent::Close);
                    })),
            )
    }

    /// The stage between the prev/next arrows (only with several items to step through), with
    /// the transcript drawer at its right while it is open.
    fn render_body(
        &self,
        chrome: ViewerChrome,
        video: &VideoState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let multiple = self.images.len() > 1;
        let prev_enabled = self.index > 0;
        let next_enabled = self.index + 1 < self.images.len();
        let stage = self.render_stage(chrome, video, cx);
        h_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .py(px(8.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(ARROW_GUTTER))
                    .overflow_hidden()
                    .child(stage)
                    .when(multiple, |body| {
                        body.child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0()
                                .w(px(ARROW_GUTTER))
                                .pl(px(16.))
                                .flex()
                                .items_center()
                                .child(
                                    Self::arrow_button("viewer-prev", IconName::ArrowLeft, chrome)
                                        .on_click(cx.listener(|viewer, _, window, cx| {
                                            viewer.step(-1, window, cx);
                                        }))
                                        .when(!prev_enabled, |button| button.opacity(0.35)),
                                ),
                        )
                        .child(
                            div()
                                .absolute()
                                .right_0()
                                .top_0()
                                .bottom_0()
                                .w(px(ARROW_GUTTER))
                                .pr(px(16.))
                                .flex()
                                .items_center()
                                .justify_end()
                                .child(
                                    Self::arrow_button("viewer-next", IconName::ArrowRight, chrome)
                                        .on_click(cx.listener(|viewer, _, window, cx| {
                                            viewer.step(1, window, cx);
                                        }))
                                        .when(!next_enabled, |button| button.opacity(0.35)),
                                ),
                        )
                    }),
            )
            .when_some(self.render_transcript_panel(), |row, panel| {
                row.child(panel)
            })
    }

    /// The footer, by media kind: the video transport centered, the zoom controls at the left
    /// (with a PDF's pager centered), and Download at the right.
    fn render_footer(
        &self,
        chrome: ViewerChrome,
        video: &VideoState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let footer = h_flex()
            .items_center()
            .flex_none()
            .px(px(12.))
            .pb(px(12.))
            .pt(px(4.));
        let download_control = self.render_download_control(chrome);
        match &self.current().media {
            ViewerMedia::Video(_) => footer
                .gap_2()
                .child(div().flex_1().min_w_0())
                .child(
                    div()
                        .flex_none()
                        .child(self.render_transport(chrome, video, cx)),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .justify_end()
                        .child(download_control),
                ),
            ViewerMedia::Pdf(_) => footer
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(self.render_zoom_controls(chrome, cx)),
                )
                .child(div().flex_none().child(self.render_pdf_pager(chrome, cx)))
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .justify_end()
                        .child(download_control),
                ),
            ViewerMedia::Image => footer
                .justify_between()
                .child(self.render_zoom_controls(chrome, cx))
                .child(download_control),
        }
    }
}
