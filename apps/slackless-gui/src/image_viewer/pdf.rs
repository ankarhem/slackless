//! Paged PDF display inside the viewer: pages render natively through the media cache's LRU
//! one at a time (page 0 on open, the neighbours through the pager), the previous page staying
//! under the rendering overlay until the next one lands.

use gpui_kit::gpui::{Context, Window};

use super::playback::{PdfPlayback, Playback};
use super::{ImageViewer, ViewerMedia};

/// Target max pixel box for native macOS PDF page rendering inside [`ImageViewer`].
pub(super) const VIEWER_PDF_MAX_W: u32 = 1600;
pub(super) const VIEWER_PDF_MAX_H: u32 = 2200;

impl ImageViewer {
    /// Current zero-based page index when viewing a PDF document.
    pub fn pdf_page(&self) -> u32 {
        self.pdf_playback().map_or(0, |p| p.page_index)
    }

    /// Known total page count for the currently shown PDF document.
    pub fn pdf_page_count(&self) -> Option<u32> {
        self.pdf_playback().and_then(|p| p.page_count)
    }

    /// Step previous (`-1`) or next (`+1`) page inside the currently displayed PDF document.
    /// Returns `true` if the page changed.
    pub fn step_pdf_page(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.is_pdf() {
            return false;
        }
        let Some(total) = self.pdf_page_count() else {
            return false;
        };
        if total <= 1 {
            return false;
        }
        let cur = self.pdf_page() as isize;
        let next = (cur + delta).clamp(0, total.saturating_sub(1) as isize) as u32;
        if next == self.pdf_page() {
            return false;
        }
        self.load_pdf_page(next, window, cx);
        true
    }

    /// Show `page_index` of the shown PDF: the media cache answers synchronously, else the page
    /// renders in the background while the previous one stays under the rendering overlay.
    pub(super) fn load_pdf_page(
        &mut self,
        page_index: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ViewerMedia::Pdf(pdf) = self.current().media.clone() else {
            return;
        };
        self.media_gen = self.media_gen.wrapping_add(1);
        let generation = self.media_gen;
        let (prev_count, prev_page) = self
            .pdf_playback()
            .map_or((None, None), |pb| (pb.page_count, pb.rendered_page.clone()));
        let prev_count =
            prev_count.or_else(|| crate::media::cached_pdf_page_count(&pdf.file_id, cx));

        if let Some(cached) = crate::media::cached_pdf_page(
            &pdf.file_id,
            page_index,
            VIEWER_PDF_MAX_W,
            VIEWER_PDF_MAX_H,
            cx,
        ) {
            self.playback = Some(Playback::Pdf(PdfPlayback {
                loading: false,
                page_index,
                page_count: prev_count.or(Some(page_index + 1)),
                rendered_page: Some(cached.image),
                error: None,
                _load_task: None,
            }));
            cx.notify();
            return;
        }

        // A local file renders without a workspace; a url needs the signed-in fetcher.
        let fetcher = if pdf.local_path.is_some() {
            None
        } else {
            let Some(fetcher) = crate::media::active_fetcher(cx) else {
                self.playback = Some(Playback::Pdf(PdfPlayback {
                    loading: false,
                    page_index,
                    page_count: prev_count,
                    rendered_page: prev_page,
                    error: Some("PDF preview requires a signed-in workspace.".to_owned()),
                    _load_task: None,
                }));
                cx.notify();
                return;
            };
            Some(fetcher)
        };

        let file_id = pdf.file_id;
        let url = pdf.url;
        let local_path = pdf.local_path;
        let load_task = cx.spawn_in(window, async move |weak, cx| {
            let bg = cx.background_executor().clone();
            let res = crate::media::fetch_and_render_pdf_page(
                fetcher,
                file_id.clone(),
                url,
                local_path,
                page_index,
                (VIEWER_PDF_MAX_W, VIEWER_PDF_MAX_H),
                bg,
            )
            .await;
            let _ = weak.update_in(cx, |viewer, window, cx| {
                if viewer.media_gen != generation {
                    return;
                }
                if let Some(Playback::Pdf(pb)) = viewer.playback.as_mut() {
                    match res {
                        Ok((page_count, rendered)) => {
                            let clamped = page_index.min(page_count.saturating_sub(1));
                            crate::media::store_cached_pdf_page(
                                file_id,
                                clamped,
                                VIEWER_PDF_MAX_W,
                                VIEWER_PDF_MAX_H,
                                page_count,
                                rendered.clone(),
                                Some(window),
                                cx,
                            );
                            pb.loading = false;
                            pb.page_index = clamped;
                            pb.page_count = Some(page_count);
                            pb.rendered_page = Some(rendered.image);
                            pb.error = None;
                        }
                        Err(err) => {
                            pb.loading = false;
                            pb.error = Some(err);
                        }
                    }
                }
                cx.notify();
            });
        });
        self.playback = Some(Playback::Pdf(PdfPlayback {
            loading: true,
            page_index,
            page_count: prev_count,
            rendered_page: prev_page,
            error: None,
            _load_task: Some(load_task),
        }));
        cx.notify();
    }
}
