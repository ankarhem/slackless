//! The stage between the arrows: the shown item's rendition - a still image at the zoom, the
//! video surface under the overlay its player's state calls for, or the rendered PDF page at the
//! zoom under its rendering or error overlay.

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, Context, FontWeight, ImageSource, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, div, px, relative,
};

use crate::theme::{self, ViewerChrome};

use super::item::poster_source_if_present;
use super::video::VideoState;
use super::{ImageViewer, ViewerMedia, ZOOM_STEPS};

impl ImageViewer {
    /// The shown item's rendition, by media kind; a still image and a PDF page scale to the zoom.
    pub(super) fn render_stage(
        &self,
        chrome: ViewerChrome,
        video: &VideoState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = ZOOM_STEPS[self.zoom_ix];
        match &self.current().media {
            ViewerMedia::Image => self.image_stage(zoom),
            ViewerMedia::Video(_) => self.video_stage(chrome, video, cx),
            ViewerMedia::Pdf(_) => self.pdf_stage(chrome, zoom),
        }
    }

    /// A still image scaled to the zoom.
    fn image_stage(&self, zoom: f32) -> AnyElement {
        let current = self.current();
        let source = current.source.clone();
        let download = current.download.clone();
        let name = current.name.clone();
        div()
            .id("viewer-image-stage")
            .w(relative(zoom))
            .h(relative(zoom))
            .flex()
            .items_center()
            .justify_center()
            .child(
                crate::media::Picture::new(source.clone())
                    .fit(ObjectFit::Contain)
                    .radius(px(8.)),
            )
            .context_menu(move |menu, _, _| {
                crate::media::image_context_menu(
                    menu,
                    source.clone(),
                    download.clone(),
                    None,
                    name.clone(),
                )
            })
            .into_any_element()
    }

    /// The video surface (its poster until the first frame) under the overlay the player's state
    /// calls for - loading, the error, or a play button while paused or ended; a click toggles
    /// play.
    fn video_stage(
        &self,
        chrome: ViewerChrome,
        video: &VideoState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let poster = poster_source_if_present(&self.current().source);
        let fg = theme::media_overlay_foreground();
        let overlay = if video.loading {
            Some(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .px_3()
                            .py_1p5()
                            .rounded_full()
                            .bg(theme::media_overlay_scrim_strong())
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(fg)
                            .child("Loading video\u{2026}"),
                    ),
            )
        } else if let Some(err) = video.error.clone() {
            Some(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .rounded_lg()
                            .bg(theme::media_overlay_scrim_strong())
                            .border_1()
                            .border_color(chrome.border)
                            .text_xs()
                            .text_color(fg)
                            .child(err),
                    ),
            )
        } else if !video.playing || video.ended {
            Some(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(56.))
                            .rounded_full()
                            .bg(theme::media_overlay_scrim())
                            .border_1()
                            .border_color(theme::media_overlay_border())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::Play).size(px(26.)).text_color(fg)),
                    ),
            )
        } else {
            None
        };
        div()
            .id("viewer-video-stage")
            .relative()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .on_click(cx.listener(|viewer, _, window, cx| {
                viewer.toggle_video_play(window, cx);
            }))
            .child(crate::media::render_video_surface(
                video.surface.clone(),
                poster,
                px(8.),
            ))
            .children(overlay)
            .into_any_element()
    }

    /// The rendered PDF page (the thumbnail until the first page lands) scaled to the zoom,
    /// under a rendering or error overlay.
    fn pdf_stage(&self, chrome: ViewerChrome, zoom: f32) -> AnyElement {
        let fg = theme::media_overlay_foreground();
        let (pdf_loading, pdf_page_img, pdf_err) = match self.pdf_playback() {
            Some(pb) => (pb.loading, pb.rendered_page.clone(), pb.error.clone()),
            None => (false, None, None),
        };
        let page_source = pdf_page_img
            .map(ImageSource::Image)
            .or_else(|| poster_source_if_present(&self.current().source));
        let pdf_overlay = if pdf_loading {
            Some(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .px_3()
                            .py_1p5()
                            .rounded_full()
                            .bg(theme::media_overlay_scrim_strong())
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(fg)
                            .child("Rendering PDF\u{2026}"),
                    ),
            )
        } else {
            pdf_err.map(|err| {
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .rounded_lg()
                            .bg(theme::media_overlay_scrim_strong())
                            .border_1()
                            .border_color(chrome.border)
                            .text_xs()
                            .text_color(fg)
                            .child(err),
                    )
            })
        };
        div()
            .id("viewer-pdf-stage")
            .relative()
            .w(relative(zoom))
            .h(relative(zoom))
            .flex()
            .items_center()
            .justify_center()
            .when_some(page_source, |el, src| {
                el.child(
                    crate::media::Picture::new(src)
                        .fit(ObjectFit::Contain)
                        .radius(px(6.)),
                )
            })
            .children(pdf_overlay)
            .into_any_element()
    }
}
