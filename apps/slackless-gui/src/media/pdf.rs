//! Bounded LRU cache and native background rasterizer for in-app PDF previews & multi-page viewer.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::gpui::{App, BackgroundExecutor, Global, Image, ImageFormat, Window};
use slackless_core::huddle::{pdf_page_count, render_pdf_page_png};

use super::audio_player::fetch_cached_media;
use super::downloads::active_fetcher;

/// Maximum rendered PDF page textures kept in memory simultaneously across all open documents.
const MAX_CACHED_PDF_PAGES: usize = 24;
/// Default inline preview thumbnail box for PDF attachment cards.
const INLINE_PDF_THUMB_W: u32 = 480;
const INLINE_PDF_THUMB_H: u32 = 320;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PdfPageKey {
    file_id: String,
    page_index: u32,
    max_w: u32,
    max_h: u32,
}

/// Cached rendered page image.
#[derive(Clone)]
pub struct CachedPdfPage {
    pub image: Arc<Image>,
}

#[derive(Default)]
struct PdfRenderCache {
    pages: HashMap<PdfPageKey, CachedPdfPage>,
    order: VecDeque<PdfPageKey>,
    page_counts: HashMap<String, u32>,
    thumb_in_flight: HashSet<String>,
    thumb_failed: HashSet<String>,
}

impl Global for PdfRenderCache {}

impl PdfRenderCache {
    fn get_page(&mut self, key: &PdfPageKey) -> Option<CachedPdfPage> {
        let hit = self.pages.get(key).cloned()?;
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.clone());
        Some(hit)
    }

    fn insert_page(&mut self, key: PdfPageKey, page: CachedPdfPage) -> Vec<CachedPdfPage> {
        let mut evicted_pages = Vec::new();
        if let Some(pos) = self.order.iter().position(|k| k == &key) {
            self.order.remove(pos);
        }
        if let Some(replaced) = self.pages.insert(key.clone(), page) {
            evicted_pages.push(replaced);
        }
        self.order.push_back(key);
        while self.order.len() > MAX_CACHED_PDF_PAGES {
            if let Some(evicted) = self.order.pop_front()
                && let Some(removed) = self.pages.remove(&evicted)
            {
                evicted_pages.push(removed);
            }
        }
        evicted_pages
    }

    fn take_viewer_pages(&mut self, file_ids: &[String]) -> Vec<CachedPdfPage> {
        if file_ids.is_empty() {
            return Vec::new();
        }
        let mut removed = Vec::new();
        self.order.retain(|k| {
            let is_viewer_page = k.max_w != INLINE_PDF_THUMB_W || k.max_h != INLINE_PDF_THUMB_H;
            let matches = is_viewer_page && file_ids.iter().any(|id| id == &k.file_id);
            if matches && let Some(page) = self.pages.remove(k) {
                removed.push(page);
            }
            !matches
        });
        removed
    }

    fn take_all(&mut self) -> Vec<CachedPdfPage> {
        self.order.clear();
        self.page_counts.clear();
        self.thumb_in_flight.clear();
        self.thumb_failed.clear();
        self.pages.drain().map(|(_, page)| page).collect()
    }
}

/// Drop a decoded PDF page texture from GPUI's window sprite atlas (when a window is available)
/// and remove it from GPUI's asset cache so evicted pages never leak GPU memory.
pub(crate) fn evict_pdf_image(image: Arc<Image>, window: Option<&mut Window>, cx: &mut App) {
    if let Some(window) = window
        && let Some(render_img) = image.clone().get_render_image(window, cx)
    {
        let _ = window.drop_image(render_img);
    }
    image.remove_asset(cx);
}

/// Evict all full-resolution viewer PDF pages for `file_ids` from both the bounded LRU cache and
/// GPUI's window/asset texture caches (keeping compact inline card thumbnails intact).
pub fn evict_viewer_pdf_pages(file_ids: &[String], window: &mut Window, cx: &mut App) {
    let evicted = if cx.has_global::<PdfRenderCache>() {
        cx.global_mut::<PdfRenderCache>()
            .take_viewer_pages(file_ids)
    } else {
        Vec::new()
    };
    for page in evicted {
        evict_pdf_image(page.image, Some(window), cx);
    }
}

/// Clear all cached PDF pages, thumbnails, and failure markers on workspace switch, releasing
/// their GPUI window textures and asset-cache entries.
pub fn clear_pdf_cache(window: &mut Window, cx: &mut App) {
    let evicted = if cx.has_global::<PdfRenderCache>() {
        cx.global_mut::<PdfRenderCache>().take_all()
    } else {
        Vec::new()
    };
    for page in evicted {
        evict_pdf_image(page.image, Some(window), cx);
    }
}

/// Cached page count for `file_id`, if already inspected.
pub fn cached_pdf_page_count(file_id: &str, cx: &App) -> Option<u32> {
    cx.try_global::<PdfRenderCache>()?
        .page_counts
        .get(file_id)
        .copied()
}

/// Cached rendered page for `(file_id, page_index, max_w, max_h)`, if present in LRU memory.
pub fn cached_pdf_page(
    file_id: &str,
    page_index: u32,
    max_w: u32,
    max_h: u32,
    cx: &mut App,
) -> Option<CachedPdfPage> {
    let key = PdfPageKey {
        file_id: file_id.to_owned(),
        page_index,
        max_w,
        max_h,
    };
    cx.default_global::<PdfRenderCache>().get_page(&key)
}

/// Store a rendered PDF page and page count in the bounded LRU cache, evicting displaced GPU
/// textures from GPUI's window & asset caches.
#[allow(clippy::too_many_arguments)]
pub fn store_cached_pdf_page(
    file_id: String,
    page_index: u32,
    max_w: u32,
    max_h: u32,
    page_count: u32,
    page: CachedPdfPage,
    window: Option<&mut Window>,
    cx: &mut App,
) {
    let evicted = {
        let cache = cx.default_global::<PdfRenderCache>();
        if page_count > 0 {
            cache.page_counts.insert(file_id.clone(), page_count);
        }
        cache.insert_page(
            PdfPageKey {
                file_id,
                page_index,
                max_w,
                max_h,
            },
            page,
        )
    };
    if let Some(window) = window {
        for old in evicted {
            evict_pdf_image(old.image, Some(window), cx);
        }
    } else {
        for old in evicted {
            evict_pdf_image(old.image, None, cx);
        }
    }
}

/// Cached page-1 inline thumbnail for a PDF card, if already rendered natively.
pub fn cached_pdf_inline_thumb(file_id: &str, cx: &App) -> Option<Arc<Image>> {
    let key = PdfPageKey {
        file_id: file_id.to_owned(),
        page_index: 0,
        max_w: INLINE_PDF_THUMB_W,
        max_h: INLINE_PDF_THUMB_H,
    };
    cx.try_global::<PdfRenderCache>()?
        .pages
        .get(&key)
        .map(|p| p.image.clone())
}

/// Lazily fetch and rasterize page 1 of a PDF file for its inline chat attachment card when
/// Slack did not provide a static PNG preview thumbnail.
pub fn request_pdf_inline_thumb(file_id: String, url: String, window: &mut Window, cx: &mut App) {
    if !cfg!(target_os = "macos") || url.is_empty() {
        return;
    }
    let key = PdfPageKey {
        file_id: file_id.clone(),
        page_index: 0,
        max_w: INLINE_PDF_THUMB_W,
        max_h: INLINE_PDF_THUMB_H,
    };
    {
        let cache = cx.default_global::<PdfRenderCache>();
        if cache.pages.contains_key(&key)
            || cache.thumb_in_flight.contains(&file_id)
            || cache.thumb_failed.contains(&file_id)
        {
            return;
        }
        cache.thumb_in_flight.insert(file_id.clone());
    }
    let Some(fetcher) = active_fetcher(cx) else {
        cx.default_global::<PdfRenderCache>()
            .thumb_in_flight
            .remove(&file_id);
        return;
    };
    let win = window.window_handle();
    let fid = file_id.clone();
    cx.spawn(async move |cx| {
        let bg = cx.background_executor().clone();
        let path_res =
            fetch_cached_media(fetcher, fid.clone(), url, "pdf".into(), bg.clone()).await;
        let rendered = match path_res {
            Ok(path) => {
                bg.spawn(async move {
                    let count = pdf_page_count(&path).map_err(|e| e.to_string())?;
                    let page =
                        render_pdf_page_png(&path, 0, INLINE_PDF_THUMB_W, INLINE_PDF_THUMB_H)
                            .map_err(|e| e.to_string())?;
                    Ok::<_, String>((count, page))
                })
                .await
            }
            Err(err) => Err(err),
        };
        cx.update(|cx| {
            let evicted = {
                let cache = cx.default_global::<PdfRenderCache>();
                cache.thumb_in_flight.remove(&fid);
                match rendered {
                    Ok((count, page)) => {
                        let img = Arc::new(Image::from_bytes(ImageFormat::Png, page.png_bytes));
                        cache.page_counts.insert(fid.clone(), count);
                        cache.insert_page(key, CachedPdfPage { image: img })
                    }
                    Err(_) => {
                        cache.thumb_failed.insert(fid);
                        Vec::new()
                    }
                }
            };
            let dropped_in_win = win
                .update(cx, |_, window, cx| {
                    for old in &evicted {
                        evict_pdf_image(old.image.clone(), Some(window), cx);
                    }
                    window.refresh();
                })
                .is_ok();
            if !dropped_in_win {
                for old in evicted {
                    evict_pdf_image(old.image, None, cx);
                }
            }
        });
    })
    .detach();
}

/// Fetch (or reuse local path / disk cache) and rasterize one PDF page off the UI thread.
pub(crate) async fn fetch_and_render_pdf_page(
    fetcher: Option<slackless_core::Fetcher>,
    file_id: String,
    url: String,
    local_path: Option<PathBuf>,
    page_index: u32,
    max_dims: (u32, u32),
    bg: BackgroundExecutor,
) -> Result<(u32, CachedPdfPage), String> {
    let (max_w, max_h) = max_dims;
    let path = if let Some(local) = local_path {
        local
    } else {
        let fetcher =
            fetcher.ok_or_else(|| "PDF preview requires a signed-in workspace.".to_owned())?;
        fetch_cached_media(fetcher, file_id, url, "pdf".into(), bg.clone()).await?
    };

    bg.spawn(async move {
        let count = pdf_page_count(&path).map_err(|e| e.to_string())?;
        let clamped_page = page_index.min(count.saturating_sub(1));
        let rendered =
            render_pdf_page_png(&path, clamped_page, max_w, max_h).map_err(|e| e.to_string())?;
        let image = Arc::new(Image::from_bytes(ImageFormat::Png, rendered.png_bytes));
        Ok((count, CachedPdfPage { image }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_render_cache_evicts_oldest_entries_beyond_capacity() {
        let mut cache = PdfRenderCache::default();
        let dummy_img = Arc::new(Image::from_bytes(ImageFormat::Png, vec![1, 2, 3]));
        for i in 0..(MAX_CACHED_PDF_PAGES + 4) {
            let key = PdfPageKey {
                file_id: format!("F{i}"),
                page_index: 0,
                max_w: 800,
                max_h: 800,
            };
            let displaced = cache.insert_page(
                key,
                CachedPdfPage {
                    image: dummy_img.clone(),
                },
            );
            if i >= MAX_CACHED_PDF_PAGES {
                assert_eq!(displaced.len(), 1);
            } else {
                assert!(displaced.is_empty());
            }
        }
        assert_eq!(cache.pages.len(), MAX_CACHED_PDF_PAGES);
        assert_eq!(cache.order.len(), MAX_CACHED_PDF_PAGES);
        // First 4 entries were evicted; newest remain.
        assert!(
            cache
                .get_page(&PdfPageKey {
                    file_id: "F0".into(),
                    page_index: 0,
                    max_w: 800,
                    max_h: 800,
                })
                .is_none()
        );
        assert!(
            cache
                .get_page(&PdfPageKey {
                    file_id: format!("F{}", MAX_CACHED_PDF_PAGES + 3),
                    page_index: 0,
                    max_w: 800,
                    max_h: 800,
                })
                .is_some()
        );
    }

    #[gpui::test]
    fn evict_viewer_pdf_pages_clears_full_res_and_retains_inline_thumbs(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_, cx) = cx.add_window_view(|_, _| gpui::Empty);
        let inline_img = Arc::new(Image::from_bytes(ImageFormat::Png, vec![1, 2, 3]));
        let viewer_img = Arc::new(Image::from_bytes(ImageFormat::Png, vec![4, 5, 6]));

        cx.update(|window, cx| {
            store_cached_pdf_page(
                "F_EVICT".into(),
                0,
                INLINE_PDF_THUMB_W,
                INLINE_PDF_THUMB_H,
                3,
                CachedPdfPage {
                    image: inline_img.clone(),
                },
                Some(window),
                cx,
            );
            store_cached_pdf_page(
                "F_EVICT".into(),
                0,
                1600,
                1600,
                3,
                CachedPdfPage {
                    image: viewer_img.clone(),
                },
                Some(window),
                cx,
            );
            assert!(
                cached_pdf_page("F_EVICT", 0, INLINE_PDF_THUMB_W, INLINE_PDF_THUMB_H, cx).is_some()
            );
            assert!(cached_pdf_page("F_EVICT", 0, 1600, 1600, cx).is_some());

            evict_viewer_pdf_pages(&["F_EVICT".to_owned()], window, cx);

            assert!(
                cached_pdf_page("F_EVICT", 0, 1600, 1600, cx).is_none(),
                "full-res viewer page must be evicted on viewer close"
            );
            assert!(!viewer_img.is_asset_cached(cx));
            assert!(
                cached_pdf_page("F_EVICT", 0, INLINE_PDF_THUMB_W, INLINE_PDF_THUMB_H, cx).is_some(),
                "inline thumbnail must stay cached after viewer close"
            );

            clear_pdf_cache(window, cx);
            assert!(
                cached_pdf_page("F_EVICT", 0, INLINE_PDF_THUMB_W, INLINE_PDF_THUMB_H, cx).is_none()
            );
        });
    }
}
