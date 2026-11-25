//! In-memory image thumbnails for fixture files (gpui-kit-guide section 9).
//!
//! Live mode downloads Slack's cookie-authenticated `thumb_*` URLs and hands the bytes to
//! [`gpui::Image::from_bytes`]; fixture mode has no network, so it paints small mockup images
//! in memory and encodes them as PNG via the `image` crate through the very same bytes path.

use std::sync::Arc;

use gpui_kit::gpui::{Image, ImageFormat};
use image::{ExtendedColorType, ImageEncoder as _, codecs::png::PngEncoder};

/// A `width` x `height` "app screenshot" mockup: dark canvas, a sidebar column tinted with
/// `accent`, a header bar, a few text lines, and an accent bar chart.
pub fn mockup(width: u32, height: u32, accent: u32) -> Arc<Image> {
    let accent = rgb_bytes(accent);
    let mut pixels = Vec::with_capacity((width * height * 3) as usize);
    let sidebar_edge = width / 5;
    let header_edge = height / 8;
    for y in 0..height {
        for x in 0..width {
            let color = if x < sidebar_edge {
                shade(accent, 0.45)
            } else if y < header_edge {
                [0x25, 0x28, 0x2D]
            } else if text_line(x, y, width, height, sidebar_edge) {
                [0x43, 0x47, 0x4D]
            } else if chart_bar(x, y, width, height) {
                accent
            } else {
                [0x1A, 0x1D, 0x21]
            };
            pixels.extend_from_slice(&color);
        }
    }
    Arc::new(Image::from_bytes(
        ImageFormat::Png,
        encode_png(width, height, &pixels),
    ))
}

/// A `size` x `size` round emblem for fixture custom emoji: an `accent` disc with an `inner`
/// core on the dark canvas color (callers clip it round).
pub fn emblem(size: u32, accent: u32, inner: u32) -> Arc<Image> {
    let (accent, inner) = (rgb_bytes(accent), rgb_bytes(inner));
    let center = size as f32 / 2.;
    let mut pixels = Vec::with_capacity((size * size * 3) as usize);
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let distance = (dx * dx + dy * dy).sqrt() / center;
            let color = if distance < 0.42 {
                inner
            } else if distance < 0.92 {
                accent
            } else {
                [0x1A, 0x1D, 0x21]
            };
            pixels.extend_from_slice(&color);
        }
    }
    Arc::new(Image::from_bytes(
        ImageFormat::Png,
        encode_png(size, size, &pixels),
    ))
}

fn text_line(x: u32, y: u32, width: u32, height: u32, left: u32) -> bool {
    let start = left + width / 14;
    (0..4).any(|line| {
        let top = height / 4 + line * height / 10;
        let len = width / 3 - line * width / 20;
        y >= top && y < top + height / 36 && x >= start && x < start + len
    })
}

fn chart_bar(x: u32, y: u32, width: u32, height: u32) -> bool {
    let base = height - height / 8;
    let bar = width / 22;
    let left = width - width / 3;
    (0..5u32).any(|ix| {
        let x0 = left + ix * bar * 3 / 2;
        let tall = height / 6 + (ix * 37 % 5) * height / 18;
        x >= x0 && x < x0 + bar && y < base && y >= base - tall
    })
}

fn rgb_bytes(hex: u32) -> [u8; 3] {
    let [_, r, g, b] = hex.to_be_bytes();
    [r, g, b]
}

fn shade(color: [u8; 3], factor: f32) -> [u8; 3] {
    color.map(|channel| (f32::from(channel) * factor) as u8)
}

/// Encodes 8-bit RGB `pixels` (row-major, no padding) as a PNG byte stream.
fn encode_png(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    if let Err(err) =
        PngEncoder::new(&mut out).write_image(pixels, width, height, ExtendedColorType::Rgb8)
    {
        tracing::warn!("could not encode fixture thumbnail PNG ({width}x{height}): {err}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_png_produces_valid_decodable_png_bytes() {
        let img = mockup(40, 30, 0x11_64_A3);
        assert!(img.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        let decoded = image::load_from_memory_with_format(&img.bytes, image::ImageFormat::Png)
            .expect("mockup PNG decodes cleanly");
        assert_eq!((decoded.width(), decoded.height()), (40, 30));

        let badge = emblem(24, 0x2B_AC_76, 0xFF_FF_FF);
        assert!(badge.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        let decoded = image::load_from_memory_with_format(&badge.bytes, image::ImageFormat::Png)
            .expect("emblem PNG decodes cleanly");
        assert_eq!((decoded.width(), decoded.height()), (24, 24));
    }
}
