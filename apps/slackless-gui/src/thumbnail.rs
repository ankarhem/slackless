//! In-memory image thumbnails for fixture files (gpui-kit-guide section 9).
//!
//! Live mode will download Slack's cookie-authenticated `thumb_*` URLs and hand the bytes to
//! [`gpui::Image::from_bytes`]; fixture mode has no network, so it paints small mockup images
//! in memory and encodes them as PNG through the very same bytes path. The encoder writes
//! uncompressed (stored) deflate blocks, which keeps it dependency-free.

use std::sync::Arc;

use gpui_kit::gpui::{Image, ImageFormat};

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
        png(width, height, &pixels),
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
        png(size, size, &pixels),
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

/// Encodes 8-bit RGB `pixels` (row-major, no padding) as a PNG file.
fn png(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let stride = width as usize * 3;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in pixels.chunks(stride) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }

    // zlib stream of stored deflate blocks (max 65,535 bytes each).
    let mut zlib = vec![0x78, 0x01];
    let mut blocks = raw.chunks(usize::from(u16::MAX)).peekable();
    while let Some(block) = blocks.next() {
        zlib.push(u8::from(blocks.peek().is_none()));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit depth, RGB, deflate, no filter/interlace

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}
