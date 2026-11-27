//! The viewer's controls: the video transport bar, the zoom controls, the download tile, the
//! PDF pager, and the chrome buttons they share.

use gpui_kit::assets::IconName;
use gpui_kit::component::slider::Slider;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::theme::ViewerChrome;

use super::video::VideoState;
use super::{ARROW_BUTTON, CONTROL_BUTTON, ImageViewer, SLIDER_WIDTH, ZOOM_STEPS};

impl ImageViewer {
    /// The video transport: play/pause, the scrub bar, elapsed / duration, the playback rate,
    /// mute with its volume, and the transcript drawer toggle.
    pub(super) fn render_transport(
        &self,
        chrome: ViewerChrome,
        video: &VideoState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let progress = crate::media::progress_fraction(video.position_secs, video.duration_secs);
        let time_readout = if video.duration_secs > 0.0 {
            format!(
                "{} / {}",
                crate::media::format_video_time(video.position_secs),
                crate::media::format_video_time(video.duration_secs)
            )
        } else {
            crate::media::format_video_time(video.position_secs)
        };
        let weak = cx.weak_entity();
        let scrub = crate::media::video_scrub_bar(
            SharedString::from("viewer-video-scrub"),
            progress,
            video.duration_secs,
            chrome.border,
            chrome.text,
            move |secs, window, cx| {
                let _ = weak.update(cx, |viewer, cx| {
                    viewer.seek_video(secs, window, cx);
                });
            },
        );
        let transcript_active = self.show_transcript;
        h_flex()
            .items_center()
            .gap_2()
            .px_2p5()
            .h(px(CONTROL_BUTTON))
            .w(px(500.))
            .max_w_full()
            .rounded_lg()
            .bg(chrome.glass)
            .border_1()
            .border_color(chrome.border)
            .child(
                div()
                    .id("viewer-video-play")
                    .flex_none()
                    .size(px(28.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(move |b| b.bg(chrome.button_hover))
                    .child(
                        Icon::new(if video.playing {
                            IconName::Pause
                        } else {
                            IconName::Play
                        })
                        .text_color(chrome.text)
                        .size(px(15.)),
                    )
                    .on_click(cx.listener(|viewer, _, window, cx| {
                        viewer.toggle_video_play(window, cx);
                    })),
            )
            .child(scrub)
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(chrome.text_muted)
                    .child(time_readout),
            )
            .child(
                div()
                    .id("viewer-video-rate")
                    .flex_none()
                    .px_1p5()
                    .h(px(24.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(chrome.text)
                    .cursor_pointer()
                    .hover(move |b| b.bg(chrome.button_hover))
                    .child(crate::media::format_playback_rate(video.rate))
                    .on_click(cx.listener(|viewer, _, _, cx| {
                        viewer.cycle_video_rate(cx);
                    })),
            )
            .child(
                h_flex()
                    .id("viewer-video-mute")
                    .flex_none()
                    .h(px(28.))
                    .px_1p5()
                    .gap_1()
                    .rounded_md()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(move |b| b.bg(chrome.button_hover))
                    .child(
                        Icon::new(if video.muted || video.volume <= 0.001 {
                            IconName::VolumeX
                        } else {
                            IconName::Volume2
                        })
                        .text_color(chrome.text)
                        .size(px(15.)),
                    )
                    .when(!video.muted && (video.volume - 1.0).abs() > 0.01, |row| {
                        row.child(
                            div()
                                .text_size(px(10.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(chrome.text_muted)
                                .child(format!("{}%", (video.volume * 100.0).round() as u32)),
                        )
                    })
                    .on_click(cx.listener(|viewer, _, _, cx| {
                        viewer.toggle_video_mute(cx);
                    })),
            )
            .child(
                div()
                    .id("viewer-video-transcript")
                    .flex_none()
                    .size(px(28.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(transcript_active, move |b| {
                        b.bg(chrome.button_hover)
                            .border_1()
                            .border_color(chrome.border)
                    })
                    .hover(move |b| b.bg(chrome.button_hover))
                    .child(
                        Icon::new(IconName::Captions)
                            .text_color(if transcript_active {
                                chrome.text
                            } else {
                                chrome.text_muted
                            })
                            .size(px(15.)),
                    )
                    .on_click(cx.listener(|viewer, _, window, cx| {
                        viewer.toggle_transcript(window, cx);
                    })),
            )
    }

    /// Zoom out, the slider and zoom in; the buttons dim at the ends of the zoom steps.
    pub(super) fn render_zoom_controls(
        &self,
        chrome: ViewerChrome,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let zoom_out_enabled = self.zoom_ix > 0;
        let zoom_in_enabled = self.zoom_ix + 1 < ZOOM_STEPS.len();
        h_flex()
            .items_center()
            .child(
                div()
                    .id("viewer-zoom-out")
                    .flex_none()
                    .size(px(CONTROL_BUTTON))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .when(zoom_out_enabled, |button| {
                        button.hover(move |button| button.bg(chrome.button))
                    })
                    .child(
                        Icon::new(IconName::Minus)
                            .text_color(if zoom_out_enabled {
                                chrome.text_muted
                            } else {
                                chrome.text_dim
                            })
                            .size(px(16.)),
                    )
                    .on_click(cx.listener(|viewer, _, window, cx| {
                        viewer.zoom_step(-1, window, cx);
                    })),
            )
            .child(
                div()
                    .w(px(SLIDER_WIDTH))
                    .mx_1()
                    .child(Slider::new(&self.slider)),
            )
            .child(
                div()
                    .id("viewer-zoom-in")
                    .flex_none()
                    .size(px(CONTROL_BUTTON))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .when(zoom_in_enabled, |button| {
                        button.hover(move |button| button.bg(chrome.button))
                    })
                    .child(
                        Icon::new(IconName::Plus)
                            .text_color(if zoom_in_enabled {
                                chrome.text_muted
                            } else {
                                chrome.text_dim
                            })
                            .size(px(16.)),
                    )
                    .on_click(cx.listener(|viewer, _, window, cx| {
                        viewer.zoom_step(1, window, cx);
                    })),
            )
    }

    /// Download, or its dimmed tile when the item has nothing to save.
    pub(super) fn render_download_control(&self, chrome: ViewerChrome) -> AnyElement {
        match self.current().download.clone() {
            Some((url, name)) => Self::tile_button("viewer-download", IconName::Download, chrome)
                .on_click(move |_, window, cx| {
                    crate::media::download(url.clone(), name.clone(), window, cx)
                })
                .into_any_element(),
            None => {
                let button: Div = div()
                    .flex_none()
                    .size(px(CONTROL_BUTTON))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .bg(chrome.button)
                    .border_1()
                    .border_color(chrome.border)
                    .opacity(0.35)
                    .child(
                        Icon::new(IconName::Download)
                            .text_color(chrome.text_dim)
                            .size(px(16.)),
                    );
                button.into_any_element()
            }
        }
    }

    /// The PDF pager: previous / "Page N of M" / next, dimmed at the document's ends.
    pub(super) fn render_pdf_pager(
        &self,
        chrome: ViewerChrome,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let page_ix = self.pdf_page();
        let page_count = self.pdf_page_count();
        let prev_page_enabled = page_ix > 0;
        let next_page_enabled = page_count.is_some_and(|n| page_ix + 1 < n);
        let page_label = match page_count {
            Some(total) => format!("Page {} / {}", page_ix + 1, total),
            None => format!("Page {}", page_ix + 1),
        };
        h_flex()
            .id("viewer-pdf-pager")
            .items_center()
            .gap_1()
            .px_1p5()
            .h(px(CONTROL_BUTTON))
            .rounded_lg()
            .bg(chrome.glass)
            .border_1()
            .border_color(chrome.border)
            .child(
                div()
                    .id("viewer-pdf-prev")
                    .flex_none()
                    .size(px(26.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(prev_page_enabled, |b| {
                        b.cursor_pointer().hover(move |h| h.bg(chrome.button_hover))
                    })
                    .when(!prev_page_enabled, |b| b.opacity(0.35))
                    .child(
                        Icon::new(IconName::ChevronLeft)
                            .size(px(15.))
                            .text_color(chrome.text),
                    )
                    .on_click(cx.listener(|viewer, _, window, cx| {
                        viewer.step_pdf_page(-1, window, cx);
                    })),
            )
            .child(
                div()
                    .px_1p5()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(chrome.text)
                    .child(page_label),
            )
            .child(
                div()
                    .id("viewer-pdf-next")
                    .flex_none()
                    .size(px(26.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(next_page_enabled, |b| {
                        b.cursor_pointer().hover(move |h| h.bg(chrome.button_hover))
                    })
                    .when(!next_page_enabled, |b| b.opacity(0.35))
                    .child(
                        Icon::new(IconName::ChevronRight)
                            .size(px(15.))
                            .text_color(chrome.text),
                    )
                    .on_click(cx.listener(|viewer, _, window, cx| {
                        viewer.step_pdf_page(1, window, cx);
                    })),
            )
    }

    /// A round chrome arrow button: dark glass, hairline border, hover lift.
    pub(super) fn arrow_button(
        id: &'static str,
        icon: IconName,
        chrome: ViewerChrome,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex_none()
            .size(px(ARROW_BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(chrome.button)
            .border_1()
            .border_color(chrome.border)
            .cursor_pointer()
            .hover(move |button| button.bg(chrome.button_hover))
            .child(Icon::new(icon).text_color(chrome.text).size(px(18.)))
    }

    /// A square glass button (the download control, Slack's bottom-right tiles).
    fn tile_button(id: &'static str, icon: IconName, chrome: ViewerChrome) -> Stateful<Div> {
        div()
            .id(id)
            .flex_none()
            .size(px(CONTROL_BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .rounded_lg()
            .bg(chrome.button)
            .border_1()
            .border_color(chrome.border)
            .cursor_pointer()
            .hover(move |button| button.bg(chrome.button_hover))
            .child(Icon::new(icon).text_color(chrome.text).size(px(16.)))
    }
}
