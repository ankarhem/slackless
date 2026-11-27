//! The shown item's live playback: nothing for a still image, the native player for a video,
//! the rendered page for a PDF - exactly one at a time, owned by the viewer and dropped with it.
//! [`video`] and [`pdf`] drive their kind.

use std::sync::Arc;

use gpui_kit::gpui::{Context, Image, ImageSource, Task, Window};
use slackless_core::huddle::NativeVideoPlayer;

use super::{ImageViewer, ViewerMedia};

/// The shown item's live playback, when it has any.
pub(super) enum Playback {
    Video(VideoPlayback),
    Pdf(PdfPlayback),
}

/// A video's native player, its latest frame and the tasks that drive it.
pub(super) struct VideoPlayback {
    pub(super) loading: bool,
    pub(super) player: Option<NativeVideoPlayer>,
    pub(super) last_frame: Option<Arc<crate::media::VideoSurface>>,
    pub(super) error: Option<String>,
    /// The duration the transport shows until the player reports its own.
    pub(super) fallback_duration_secs: f64,
    /// The fetch-and-open task, held so dropping the playback cancels it.
    pub(super) _load_task: Option<Task<()>>,
    /// The frame tick; restarting it (play, seek) cancels the previous one.
    pub(super) tick_task: Option<Task<()>>,
}

/// A PDF's rendered page and the task rendering the next one.
pub(super) struct PdfPlayback {
    pub(super) loading: bool,
    pub(super) page_index: u32,
    pub(super) page_count: Option<u32>,
    pub(super) rendered_page: Option<Arc<Image>>,
    pub(super) error: Option<String>,
    /// The page render, held so dropping the playback cancels it.
    pub(super) _load_task: Option<Task<()>>,
}

impl ImageViewer {
    /// The shown video's playback, if a video is shown.
    pub(super) fn video_playback(&self) -> Option<&VideoPlayback> {
        match &self.playback {
            Some(Playback::Video(pb)) => Some(pb),
            Some(Playback::Pdf(_)) | None => None,
        }
    }

    /// The shown PDF's playback, if a PDF is shown.
    pub(super) fn pdf_playback(&self) -> Option<&PdfPlayback> {
        match &self.playback {
            Some(Playback::Pdf(pb)) => Some(pb),
            Some(Playback::Video(_)) | None => None,
        }
    }

    /// Sync playback to the shown item: stop whatever played before, then load page 0 of a PDF
    /// or open the video.
    pub(super) fn sync_current_media(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_playback();
        match self.current().media.clone() {
            ViewerMedia::Image => {}
            ViewerMedia::Pdf(_) => self.load_pdf_page(0, window, cx),
            ViewerMedia::Video(video) => self.start_video(video, window, cx),
        }
    }

    /// Stop and drop any active video player and background tasks immediately.
    pub fn stop_video(&mut self) {
        if matches!(self.playback, Some(Playback::Video(_))) {
            self.stop_playback();
        }
    }

    /// Stop and drop any active PDF render task and page state immediately.
    pub fn stop_pdf(&mut self) {
        if matches!(self.playback, Some(Playback::Pdf(_))) {
            self.stop_playback();
        }
    }

    /// Drop the playback and its tasks; a load still in flight lands on a newer generation.
    pub(super) fn stop_playback(&mut self) {
        self.media_gen = self.media_gen.wrapping_add(1);
        self.playback = None;
    }

    /// Evict decoded full-resolution image and PDF page textures loaded by this viewer from GPUI's
    /// window & asset caches and immediately stop any active video playback or PDF render tasks.
    pub fn evict_assets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_video();
        let active_pdf_page = self.pdf_playback().and_then(|pb| pb.rendered_page.clone());
        self.stop_pdf();
        let pdf_ids: Vec<String> = self
            .images
            .iter()
            .filter_map(|entry| match &entry.media {
                ViewerMedia::Pdf(pdf) => Some(pdf.file_id.clone()),
                ViewerMedia::Image | ViewerMedia::Video(_) => None,
            })
            .collect();
        if !pdf_ids.is_empty() {
            crate::media::evict_viewer_pdf_pages(&pdf_ids, window, cx);
        }
        if let Some(img) = active_pdf_page {
            crate::media::evict_pdf_image(img, Some(window), cx);
        }
        for entry in self.images.iter() {
            if let ImageSource::Resource(ref resource) = entry.source {
                if let Some(Ok(data)) =
                    window.get_asset::<gpui_kit::gpui::ImgResourceLoader>(resource, cx)
                {
                    let _ = window.drop_image(data);
                }
                entry.source.remove_asset(cx);
            }
        }
    }
}
