//! Copy rendered or remote images to the OS clipboard as native image data (PNG/JPEG/GIF/BMP/TIFF)
//! and build the shared right-click image context menu.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::gpui::{
    App, ClipboardItem, Image, ImageFormat, ImageSource, ImgResourceLoader, RenderImage, Resource,
    Window,
};
use image::ImageEncoder as _;

/// Inspect image magic bytes to identify a GPUI [`ImageFormat`].
pub(crate) fn detect_image_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.starts_with(b"BM") {
        Some(ImageFormat::Bmp)
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some(ImageFormat::Tiff)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else {
        None
    }
}

/// Formats that paste reliably across macOS, Linux, and Windows apps without lossy conversion.
/// WebP on macOS pasteboard is ignored by several native apps, so we prefer encoding decoded
/// WebP frames to PNG when a [`RenderImage`] is available.
fn is_widely_supported_clipboard_format(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::Gif
            | ImageFormat::Bmp
            | ImageFormat::Tiff
    )
}

/// Encode GPUI's BGRA8 frame buffer into a PNG byte stream (swapping B and R channels to RGBA8).
pub(crate) fn encode_bgra_to_png(width: u32, height: u32, bgra: &[u8]) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err("image has zero dimensions".to_owned());
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| "image dimensions overflow".to_owned())?;
    if bgra.len() != expected {
        return Err(format!(
            "image buffer length mismatch (got {}, expected {expected})",
            bgra.len()
        ));
    }

    let mut rgba = vec![0u8; expected];
    for (src, dst) in bgra
        .as_chunks::<4>()
        .0
        .iter()
        .zip(rgba.as_chunks_mut::<4>().0.iter_mut())
    {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }

    let mut out = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut out);
    encoder
        .write_image(&rgba, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|err| format!("PNG encode failed: {err}"))?;
    Ok(out)
}

/// Encode frame 0 of a decoded GPUI [`RenderImage`] as PNG bytes.
pub(crate) fn render_image_to_png(render: &RenderImage) -> Result<Vec<u8>, String> {
    let bytes = render
        .as_bytes(0)
        .ok_or_else(|| "decoded image frame unavailable".to_owned())?;
    let size = render.size(0);
    let width = u32::try_from(size.width.0).map_err(|_| "invalid image width".to_owned())?;
    let height = u32::try_from(size.height.0).map_err(|_| "invalid image height".to_owned())?;
    encode_bgra_to_png(width, height, bytes)
}

fn clipboard_image_from_bytes(
    bytes: Vec<u8>,
    fallback_render: Option<&RenderImage>,
) -> Result<Image, String> {
    match detect_image_format(&bytes) {
        Some(fmt) if is_widely_supported_clipboard_format(fmt) => Ok(Image::from_bytes(fmt, bytes)),
        Some(ImageFormat::Webp) => {
            if let Some(render) = fallback_render
                && let Ok(png) = render_image_to_png(render)
            {
                return Ok(Image::from_bytes(ImageFormat::Png, png));
            }
            Ok(Image::from_bytes(ImageFormat::Webp, bytes))
        }
        Some(_) | None => {
            if let Some(render) = fallback_render {
                render_image_to_png(render).map(|png| Image::from_bytes(ImageFormat::Png, png))
            } else {
                Err("unsupported image format".to_owned())
            }
        }
    }
}

fn finish_copy_result(
    result: Result<Image, String>,
    label: String,
    window: &mut Window,
    cx: &mut App,
) {
    match result {
        Ok(image) => {
            cx.write_to_clipboard(ClipboardItem::new_image(&image));
            let detail = if label.trim().is_empty() {
                "Copied to clipboard".to_owned()
            } else {
                label
            };
            crate::actions::notify("Image copied", detail, window, cx);
        }
        Err(err) => {
            tracing::warn!("slackless: copy image failed for {label:?}: {err}");
            crate::actions::notify("Could not copy image", err, window, cx);
        }
    }
}

/// Copy an image to the system clipboard.
///
/// Prefers full-resolution original bytes when `original_url` is provided and an authenticated
/// fetcher is active; otherwise uses in-memory [`Image`] bytes, local file bytes, or GPUI's
/// already-decoded [`RenderImage`] (encoded to PNG off the UI thread).
pub fn copy_image_to_clipboard(
    source: ImageSource,
    original_url: Option<String>,
    label: String,
    window: &mut Window,
    cx: &mut App,
) {
    let cached_render: Option<Arc<RenderImage>> = match &source {
        ImageSource::Render(render) => Some(render.clone()),
        ImageSource::Resource(resource) => window
            .get_asset::<ImgResourceLoader>(resource, cx)
            .and_then(Result::ok),
        _ => None,
    };

    match source {
        ImageSource::Image(img) => {
            if is_widely_supported_clipboard_format(img.format) || cached_render.is_none() {
                finish_copy_result(Ok((*img).clone()), label, window, cx);
                return;
            }
            if let Some(render) = cached_render {
                window
                    .spawn(cx, async move |cx| {
                        let res = cx
                            .background_executor()
                            .spawn(async move {
                                match render_image_to_png(&render) {
                                    Ok(png) => Ok(Image::from_bytes(ImageFormat::Png, png)),
                                    Err(_) => Ok((*img).clone()),
                                }
                            })
                            .await;
                        let _ = cx.update(|window, cx| {
                            finish_copy_result(res, label, window, cx);
                        });
                    })
                    .detach();
            }
        }
        ImageSource::Resource(Resource::Path(path)) => {
            window
                .spawn(cx, async move |cx| {
                    let res = cx
                        .background_executor()
                        .spawn(async move {
                            match std::fs::read(&path) {
                                Ok(bytes) => {
                                    clipboard_image_from_bytes(bytes, cached_render.as_deref())
                                }
                                Err(err) => match cached_render.as_deref() {
                                    Some(render) => render_image_to_png(render)
                                        .map(|png| Image::from_bytes(ImageFormat::Png, png)),
                                    None => Err(format!("could not read image file: {err}")),
                                },
                            }
                        })
                        .await;
                    let _ = cx.update(|window, cx| {
                        finish_copy_result(res, label, window, cx);
                    });
                })
                .detach();
        }
        ImageSource::Resource(Resource::Uri(uri)) => {
            let fetch_url = original_url.unwrap_or_else(|| uri.to_string());
            if let Some(fetcher) = super::active_fetcher(cx) {
                let pending = fetcher.get(fetch_url);
                window
                    .spawn(cx, async move |cx| {
                        let fetched = pending.await;
                        let res = cx
                            .background_executor()
                            .spawn(async move {
                                match fetched {
                                    Ok(bytes) => clipboard_image_from_bytes(
                                        bytes.to_vec(),
                                        cached_render.as_deref(),
                                    ),
                                    Err(err) => match cached_render.as_deref() {
                                        Some(render) => render_image_to_png(render)
                                            .map(|png| Image::from_bytes(ImageFormat::Png, png)),
                                        None => Err(err),
                                    },
                                }
                            })
                            .await;
                        let _ = cx.update(|window, cx| {
                            finish_copy_result(res, label, window, cx);
                        });
                    })
                    .detach();
            } else if let Some(render) = cached_render {
                window
                    .spawn(cx, async move |cx| {
                        let res = cx
                            .background_executor()
                            .spawn(async move {
                                render_image_to_png(&render)
                                    .map(|png| Image::from_bytes(ImageFormat::Png, png))
                            })
                            .await;
                        let _ = cx.update(|window, cx| {
                            finish_copy_result(res, label, window, cx);
                        });
                    })
                    .detach();
            } else {
                finish_copy_result(Err("image is not loaded yet".to_owned()), label, window, cx);
            }
        }
        ImageSource::Resource(Resource::Embedded(_))
        | ImageSource::Render(_)
        | ImageSource::Custom(_) => {
            if let Some(render) = cached_render {
                window
                    .spawn(cx, async move |cx| {
                        let res = cx
                            .background_executor()
                            .spawn(async move {
                                render_image_to_png(&render)
                                    .map(|png| Image::from_bytes(ImageFormat::Png, png))
                            })
                            .await;
                        let _ = cx.update(|window, cx| {
                            finish_copy_result(res, label, window, cx);
                        });
                    })
                    .detach();
            } else {
                finish_copy_result(Err("image is not loaded yet".to_owned()), label, window, cx);
            }
        }
    }
}

/// Shared right-click context menu for images across messages, galleries, Block Kit, the image
/// viewer, and canvases.
pub fn image_context_menu(
    mut menu: PopupMenu,
    source: ImageSource,
    download: Option<(String, String)>,
    link_url: Option<String>,
    label: String,
) -> PopupMenu {
    let original_url = download
        .as_ref()
        .map(|(url, _)| url.clone())
        .or_else(|| link_url.clone());
    menu = menu.item(
        PopupMenuItem::new("Copy image")
            .icon(IconName::Copy)
            .on_click(move |_, window, cx| {
                copy_image_to_clipboard(
                    source.clone(),
                    original_url.clone(),
                    label.clone(),
                    window,
                    cx,
                );
            }),
    );

    if let Some(url) = link_url {
        menu = menu.item(
            PopupMenuItem::new("Copy image link")
                .icon(IconName::Link)
                .on_click(move |_, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                    crate::actions::notify("Link copied", url.clone(), window, cx);
                }),
        );
    }

    if let Some((url, name)) = download {
        let save_url = url.clone();
        let save_name = name.clone();
        menu = menu
            .separator()
            .item(
                PopupMenuItem::new("Download")
                    .icon(IconName::Download)
                    .on_click(move |_, window, cx| {
                        super::download(url.clone(), name.clone(), window, cx);
                    }),
            )
            .item(
                PopupMenuItem::new("Save image as\u{2026}").on_click(move |_, window, cx| {
                    super::download_as(save_url.clone(), save_name.clone(), window, cx);
                }),
            );
    }

    menu
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui_kit::base::Root;
    use gpui_kit::gpui::{
        AppContext as _, ClipboardEntry, Empty, Image, ImageFormat, ImageSource, RenderImage,
        TestAppContext,
    };

    use super::{
        clipboard_image_from_bytes, copy_image_to_clipboard, detect_image_format,
        encode_bgra_to_png,
    };

    #[test]
    fn detects_common_image_magic_headers() {
        assert_eq!(
            detect_image_format(b"\x89PNG\r\n\x1a\nrest"),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            detect_image_format(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(detect_image_format(b"GIF89a..."), Some(ImageFormat::Gif));
        assert_eq!(detect_image_format(b"GIF87a..."), Some(ImageFormat::Gif));
        assert_eq!(detect_image_format(b"BM\x00\x00"), Some(ImageFormat::Bmp));
        assert_eq!(detect_image_format(b"II*\0rest"), Some(ImageFormat::Tiff));
        assert_eq!(detect_image_format(b"MM\0*rest"), Some(ImageFormat::Tiff));
        assert_eq!(
            detect_image_format(b"RIFF\x12\x34\x56\x78WEBP"),
            Some(ImageFormat::Webp)
        );
        assert_eq!(detect_image_format(b"not an image"), None);
        assert_eq!(detect_image_format(b""), None);
    }

    #[test]
    fn encodes_bgra_pixels_to_valid_rgba_png() {
        // 1x1 red pixel in BGRA8: B=0, G=0, R=255, A=255.
        let png = encode_bgra_to_png(1, 1, &[0, 0, 255, 255]).expect("encode 1x1 BGRA");
        assert_eq!(detect_image_format(&png), Some(ImageFormat::Png));
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("decode PNG")
            .to_rgba8();
        assert_eq!(decoded.dimensions(), (1, 1));
        assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0, 255]);

        assert!(encode_bgra_to_png(0, 1, &[]).is_err());
        assert!(encode_bgra_to_png(2, 2, &[0; 4]).is_err());
    }

    #[test]
    fn clipboard_image_from_bytes_accepts_png_and_rejects_garbage() {
        let png = encode_bgra_to_png(1, 1, &[10, 20, 30, 255]).expect("encode png");
        let img = clipboard_image_from_bytes(png.clone(), None).expect("png accepted");
        assert_eq!(img.format, ImageFormat::Png);
        assert_eq!(img.bytes, png);

        assert!(clipboard_image_from_bytes(b"<!doctype html>".to_vec(), None).is_err());
    }

    #[gpui_kit::gpui::test]
    fn copy_image_to_clipboard_writes_in_memory_image_entry(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
        });
        let pixels = [0u8, 128, 255, 255].repeat(4);
        let png = encode_bgra_to_png(2, 2, &pixels).expect("encode png");
        let image = Arc::new(Image::from_bytes(ImageFormat::Png, png.clone()));
        let (_root, visual_cx) = cx.add_window_view(|window, cx| {
            let empty = cx.new(|_| Empty);
            Root::new(empty, window, cx)
        });
        visual_cx.update(|window, cx| {
            copy_image_to_clipboard(
                ImageSource::Image(image.clone()),
                None,
                "chart.png".to_owned(),
                window,
                cx,
            );
        });
        let item = visual_cx
            .read_from_clipboard()
            .expect("clipboard item written");
        let copied = item.entries().iter().find_map(|entry| match entry {
            ClipboardEntry::Image(img) => Some(img),
            _ => None,
        });
        let copied = copied.expect("clipboard entry is an image");
        assert_eq!(copied.format, ImageFormat::Png);
        assert_eq!(copied.bytes, png);
    }

    #[gpui_kit::gpui::test]
    fn copy_image_to_clipboard_encodes_render_image_to_png_off_ui_thread(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
        });
        // 1x1 blue pixel in BGRA8: B=255, G=0, R=0, A=255 -> RGBA [0, 0, 255, 255].
        let buf = image::RgbaImage::from_raw(1, 1, vec![255, 0, 0, 255]).expect("1x1 frame");
        let render = Arc::new(RenderImage::new(vec![image::Frame::new(buf)]));
        let (_root, visual_cx) = cx.add_window_view(|window, cx| {
            let empty = cx.new(|_| Empty);
            Root::new(empty, window, cx)
        });
        visual_cx.update(|window, cx| {
            copy_image_to_clipboard(
                ImageSource::Render(render.clone()),
                None,
                "frame.png".to_owned(),
                window,
                cx,
            );
        });
        visual_cx.run_until_parked();
        let item = visual_cx
            .read_from_clipboard()
            .expect("clipboard item written");
        let copied = item.entries().iter().find_map(|entry| match entry {
            ClipboardEntry::Image(img) => Some(img),
            _ => None,
        });
        let copied = copied.expect("clipboard entry is an image");
        assert_eq!(copied.format, ImageFormat::Png);
        let decoded = image::load_from_memory_with_format(&copied.bytes, image::ImageFormat::Png)
            .expect("decode clipboard png")
            .to_rgba8();
        assert_eq!(decoded.get_pixel(0, 0).0, [0, 0, 255, 255]);
    }
}
