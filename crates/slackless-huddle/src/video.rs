//! Native H.264 video pipeline: webcam capture (`nokhwa`), software portrait background blur,
//! Cisco OpenH264 encode/decode (`openh264`), and RTP H.264 depacketization for Chime SFU.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nokhwa::Camera;
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType};
use openh264::OpenH264API;
use openh264::decoder::Decoder as H264Decoder;
use openh264::encoder::{
    BitRate, Complexity, Encoder as H264Encoder, EncoderConfig, FrameRate, IntraFramePeriod,
    RateControlMode, UsageType,
};
use openh264::formats::{RgbSliceU8, YUVBuffer, YUVSlices, YUVSource as _};
use slackless_types::UserId;
use tokio::sync::mpsc;
use webrtc::rtp::codecs::h264::H264Packet;
use webrtc::rtp::packetizer::Depacketizer as _;

use crate::{HuddleError, Result};

/// Maximum dimensions for webcam capture (720p, even numbers required for YUV420).
pub const CAMERA_MAX_W: u32 = 1280;
pub const CAMERA_MAX_H: u32 = 720;

/// One decoded or captured video frame in BGRA8888 byte order (ready for GPUI `RenderImage`).
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Arc<Vec<u8>>,
}

impl PartialEq for VideoFrame {
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width
            && self.height == other.height
            && Arc::ptr_eq(&self.bgra, &other.bgra)
    }
}

/// Identifies which tile a [`VideoFrame`] belongs to in the Huddle window.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum VideoTileKey {
    LocalCamera,
    LocalScreen,
    RemoteCamera(UserId),
    RemoteScreen(UserId),
}

/// One enumerated local camera device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraDeviceInfo {
    pub index: usize,
    pub name: String,
}

/// Enumerate all host cameras (`AVFoundation` on macOS, V4L2 on Linux, MSMF on Windows).
pub fn list_cameras() -> Vec<CameraDeviceInfo> {
    nokhwa::query(ApiBackend::Auto)
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(fallback_ix, info)| {
            let index = match info.index() {
                CameraIndex::Index(i) => *i as usize,
                CameraIndex::String(_) => fallback_ix,
            };
            CameraDeviceInfo {
                index,
                name: info.human_name(),
            }
        })
        .collect()
}

/// Encode RGB888 or I420 YUV frames into Annex-B H.264 NAL units via Cisco OpenH264.
pub struct H264VideoEncoder {
    encoder: H264Encoder,
    frame_count: u64,
    keyframe_interval: u64,
}

impl H264VideoEncoder {
    /// Camera-tuned H.264 encoder (1.5 Mbps, 24 fps, multi-threaded).
    pub fn new() -> Result<Self> {
        Self::new_camera()
    }

    pub fn new_camera() -> Result<Self> {
        let cfg = EncoderConfig::new()
            .usage_type(UsageType::CameraVideoRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(1_500_000))
            .max_frame_rate(FrameRate::from_hz(24.0))
            .complexity(Complexity::Low)
            .num_threads(2)
            .skip_frames(true)
            .intra_frame_period(IntraFramePeriod::from_num_frames(72));
        let encoder = H264Encoder::with_api_config(OpenH264API::from_source(), cfg)
            .map_err(|e| HuddleError::Codec(e.to_string()))?;
        Ok(Self {
            encoder,
            frame_count: 0,
            keyframe_interval: 72,
        })
    }

    /// Screen-content-tuned H.264 encoder (`ScreenContentRealTime`, 2.8 Mbps, 20 fps, sharp text).
    pub fn new_screen() -> Result<Self> {
        let cfg = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(2_800_000))
            .max_frame_rate(FrameRate::from_hz(20.0))
            .complexity(Complexity::Low)
            .num_threads(2)
            .skip_frames(true)
            .intra_frame_period(IntraFramePeriod::from_num_frames(100));
        let encoder = H264Encoder::with_api_config(OpenH264API::from_source(), cfg)
            .map_err(|e| HuddleError::Codec(e.to_string()))?;
        Ok(Self {
            encoder,
            frame_count: 0,
            keyframe_interval: 100,
        })
    }

    /// Request an immediate IDR keyframe on the next encoded frame.
    pub fn force_keyframe(&mut self) {
        self.encoder.force_intra_frame();
    }

    /// Encode one RGB888 frame (dimensions MUST be even) into Annex-B H.264 bytes.
    pub fn encode_rgb(&mut self, rgb: &[u8], width: usize, height: usize) -> Result<Vec<u8>> {
        if self.frame_count.is_multiple_of(self.keyframe_interval) {
            self.encoder.force_intra_frame();
        }
        self.frame_count = self.frame_count.wrapping_add(1);
        let yuv = YUVBuffer::from_rgb_source(RgbSliceU8::new(rgb, (width, height)));
        let bs = self
            .encoder
            .encode(&yuv)
            .map_err(|e| HuddleError::Codec(e.to_string()))?;
        Ok(bs.to_vec())
    }

    /// Encode one planar I420 YUV420 frame directly (zero intermediate RGB allocation).
    pub fn encode_i420(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        width: usize,
        height: usize,
    ) -> Result<Vec<u8>> {
        if self.frame_count.is_multiple_of(self.keyframe_interval) {
            self.encoder.force_intra_frame();
        }
        self.frame_count = self.frame_count.wrapping_add(1);
        let slices = YUVSlices::new((y, u, v), (width, height), (width, width / 2, width / 2));
        let bs = self
            .encoder
            .encode(&slices)
            .map_err(|e| HuddleError::Codec(e.to_string()))?;
        Ok(bs.to_vec())
    }
}

/// Depacketize RTP H.264 payloads (Single NAL, STAP-A, FU-A) and decode to BGRA8888 [`VideoFrame`].
/// Uses Apple Silicon `VideoToolbox` hardware decoding on macOS with automatic OpenH264 fallback.
pub struct H264VideoDecoder {
    #[cfg(target_os = "macos")]
    vt: Option<crate::macos_media::VtH264Decoder>,
    decoder: H264Decoder,
    depacketizer: H264Packet,
    annex_b_buf: Vec<u8>,
}

impl H264VideoDecoder {
    pub fn new() -> Result<Self> {
        let decoder = H264Decoder::new().map_err(|e| HuddleError::Codec(e.to_string()))?;
        #[cfg(target_os = "macos")]
        let vt = crate::macos_media::VtH264Decoder::new().ok();
        Ok(Self {
            #[cfg(target_os = "macos")]
            vt,
            decoder,
            depacketizer: H264Packet::default(),
            annex_b_buf: Vec::with_capacity(64 * 1024),
        })
    }

    pub fn backend_name(&self) -> &'static str {
        #[cfg(target_os = "macos")]
        if self.vt.is_some() {
            return "H.264 (VideoToolbox HW)";
        }
        "H.264 (OpenH264 SW)"
    }

    /// Feed one RTP payload (`marker` = RTP marker bit indicating end of access unit).
    pub fn push_rtp_payload(&mut self, payload: &[u8], marker: bool) -> Option<VideoFrame> {
        let raw = bytes::Bytes::copy_from_slice(payload);
        if let Ok(nal) = self.depacketizer.depacketize(&raw) {
            if !nal.is_empty() {
                self.annex_b_buf.extend_from_slice(&nal);
            }
        }
        if !marker || self.annex_b_buf.is_empty() {
            return None;
        }
        let bitstream = std::mem::take(&mut self.annex_b_buf);
        let frame = self.decode_annex_b(&bitstream);
        self.annex_b_buf = bitstream;
        self.annex_b_buf.clear();
        frame
    }

    /// Decode a complete Annex-B H.264 access unit into a BGRA8888 [`VideoFrame`].
    pub fn decode_annex_b(&mut self, bitstream: &[u8]) -> Option<VideoFrame> {
        #[cfg(target_os = "macos")]
        if let Some(vt) = self.vt.as_mut() {
            if let Some(frame) = vt.decode_annex_b(bitstream) {
                return Some(frame);
            }
        }
        let yuv = self.decoder.decode(bitstream).ok()??;
        let (w, h) = yuv.dimensions();
        if w == 0 || h == 0 {
            return None;
        }
        let mut rgba = vec![0u8; w * h * 4];
        yuv.write_rgba8(&mut rgba);
        // Convert RGBA8888 -> BGRA8888 in-place for GPUI `RenderImage`.
        for px in rgba.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        Some(VideoFrame {
            width: w as u32,
            height: h as u32,
            bgra: Arc::new(rgba),
        })
    }
}

/// Area-averaged box downscale of an RGB888 buffer into even `(dst_w, dst_h)` dimensions
/// fitting within `(max_w, max_h)` — preserves crisp text and avoids nearest-neighbor aliasing.
pub fn resize_rgb_even(
    rgb: &[u8],
    src_w: u32,
    src_h: u32,
    max_w: u32,
    max_h: u32,
) -> (Vec<u8>, u32, u32) {
    if src_w == 0 || src_h == 0 {
        return (Vec::new(), 0, 0);
    }
    let scale = (max_w as f32 / src_w as f32)
        .min(max_h as f32 / src_h as f32)
        .min(1.0);
    let dst_w = (((src_w as f32 * scale) as u32) & !1).max(2);
    let dst_h = (((src_h as f32 * scale) as u32) & !1).max(2);
    if dst_w == src_w && dst_h == src_h {
        return (rgb.to_vec(), src_w, src_h);
    }
    let sw = src_w as usize;
    let sh = src_h as usize;
    let dw = dst_w as usize;
    let dh = dst_h as usize;
    let mut out = vec![0u8; dw * dh * 3];
    for y in 0..dh {
        let sy0 = (y * sh) / dh;
        let sy1 = (((y + 1) * sh) / dh).max(sy0 + 1).min(sh);
        for x in 0..dw {
            let sx0 = (x * sw) / dw;
            let sx1 = (((x + 1) * sw) / dw).max(sx0 + 1).min(sw);
            let mut r_sum = 0u32;
            let mut g_sum = 0u32;
            let mut b_sum = 0u32;
            let mut cnt = 0u32;
            for sy in sy0..sy1 {
                let row = sy * sw;
                for sx in sx0..sx1 {
                    let off = (row + sx) * 3;
                    if off + 3 <= rgb.len() {
                        r_sum += rgb[off] as u32;
                        g_sum += rgb[off + 1] as u32;
                        b_sum += rgb[off + 2] as u32;
                        cnt += 1;
                    }
                }
            }
            if let Some(r) = r_sum.checked_div(cnt) {
                let d_off = (y * dw + x) * 3;
                out[d_off] = r as u8;
                out[d_off + 1] = g_sum.checked_div(cnt).unwrap_or(0) as u8;
                out[d_off + 2] = b_sum.checked_div(cnt).unwrap_or(0) as u8;
            }
        }
    }
    (out, dst_w, dst_h)
}

/// Reusable scratch buffers for single-pass area-averaged RGBA -> BGRA + I420 downscaling
/// (zero per-frame heap churn for software screen sharing).
#[derive(Default)]
pub struct ScreenScaleScratch {
    pub bgra: Vec<u8>,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

/// Single-pass integer area-averaged downscale directly from RGBA8888 into BGRA8888 (for preview)
/// and planar I420 YUV (for OpenH264), returning `(dst_w, dst_h)`.
pub fn downscale_rgba_area_to_bgra_and_i420(
    rgba: &[u8],
    src_w: u32,
    src_h: u32,
    max_w: u32,
    max_h: u32,
    scratch: &mut ScreenScaleScratch,
) -> (u32, u32) {
    if src_w == 0 || src_h == 0 || rgba.len() < (src_w as usize * src_h as usize * 4) {
        return (0, 0);
    }
    let scale = (max_w as f32 / src_w as f32)
        .min(max_h as f32 / src_h as f32)
        .min(1.0);
    let dst_w = (((src_w as f32 * scale) as u32) & !1).max(2);
    let dst_h = (((src_h as f32 * scale) as u32) & !1).max(2);
    let sw = src_w as usize;
    let sh = src_h as usize;
    let dw = dst_w as usize;
    let dh = dst_h as usize;

    scratch.bgra.resize(dw * dh * 4, 0);
    scratch.y.resize(dw * dh, 0);
    let cw = dw / 2;
    let ch = dh / 2;
    scratch.u.resize(cw * ch, 128);
    scratch.v.resize(cw * ch, 128);

    // Precompute source x intervals once per frame.
    let mut x_bounds = Vec::with_capacity(dw);
    for x in 0..dw {
        let sx0 = (x * sw) / dw;
        let sx1 = (((x + 1) * sw) / dw).max(sx0 + 1).min(sw);
        x_bounds.push((sx0, sx1));
    }

    for y in 0..dh {
        let sy0 = (y * sh) / dh;
        let sy1 = (((y + 1) * sh) / dh).max(sy0 + 1).min(sh);
        let dy_row = y * dw;
        for (x, &(sx0, sx1)) in x_bounds.iter().enumerate() {
            let mut r_sum = 0u32;
            let mut g_sum = 0u32;
            let mut b_sum = 0u32;
            let cnt = ((sy1 - sy0) * (sx1 - sx0)) as u32;
            for sy in sy0..sy1 {
                let row_off = sy * sw;
                for sx in sx0..sx1 {
                    let off = (row_off + sx) * 4;
                    r_sum += rgba[off] as u32;
                    g_sum += rgba[off + 1] as u32;
                    b_sum += rgba[off + 2] as u32;
                }
            }
            let r = (r_sum / cnt) as i32;
            let g = (g_sum / cnt) as i32;
            let b = (b_sum / cnt) as i32;

            let px_idx = dy_row + x;
            let boff = px_idx * 4;
            scratch.bgra[boff] = b as u8;
            scratch.bgra[boff + 1] = g as u8;
            scratch.bgra[boff + 2] = r as u8;
            scratch.bgra[boff + 3] = 255;

            // BT.601 studio-swing integer Y.
            let y_val = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
            scratch.y[px_idx] = y_val.clamp(0, 255) as u8;
        }
    }

    // 2x2 chroma planes from the already anti-aliased BGRA buffer.
    for cy in 0..ch {
        let y0 = cy * 2;
        let y1 = y0 + 1;
        let c_row = cy * cw;
        for cx in 0..cw {
            let x0 = cx * 2;
            let p00 = (y0 * dw + x0) * 4;
            let p01 = p00 + 4;
            let p10 = (y1 * dw + x0) * 4;
            let p11 = p10 + 4;
            let b = (scratch.bgra[p00] as i32
                + scratch.bgra[p01] as i32
                + scratch.bgra[p10] as i32
                + scratch.bgra[p11] as i32
                + 2)
                >> 2;
            let g = (scratch.bgra[p00 + 1] as i32
                + scratch.bgra[p01 + 1] as i32
                + scratch.bgra[p10 + 1] as i32
                + scratch.bgra[p11 + 1] as i32
                + 2)
                >> 2;
            let r = (scratch.bgra[p00 + 2] as i32
                + scratch.bgra[p01 + 2] as i32
                + scratch.bgra[p10 + 2] as i32
                + scratch.bgra[p11 + 2] as i32
                + 2)
                >> 2;
            let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
            let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
            scratch.u[c_row + cx] = u.clamp(0, 255) as u8;
            scratch.v[c_row + cx] = v.clamp(0, 255) as u8;
        }
    }

    (dst_w, dst_h)
}

/// Convert RGB888 into BGRA8888 for GPUI `RenderImage`.
pub fn rgb_to_bgra(rgb: &[u8]) -> Vec<u8> {
    let mut bgra = Vec::with_capacity((rgb.len() / 3) * 4);
    for px in rgb.as_chunks::<3>().0 {
        bgra.push(px[2]);
        bgra.push(px[1]);
        bgra.push(px[0]);
        bgra.push(255);
    }
    bgra
}

/// Convert RGBA8888 into BGRA8888 for GPUI `RenderImage` and RGB888 for OpenH264 encoding.
pub fn rgba_to_rgb_and_bgra(rgba: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let pixels = rgba.len() / 4;
    let mut rgb = Vec::with_capacity(pixels * 3);
    let mut bgra = Vec::with_capacity(pixels * 4);
    for px in rgba.as_chunks::<4>().0 {
        rgb.push(px[0]);
        rgb.push(px[1]);
        rgb.push(px[2]);
        bgra.push(px[2]);
        bgra.push(px[1]);
        bgra.push(px[0]);
        bgra.push(px[3]);
    }
    (rgb, bgra)
}

/// Fast software portrait background blur (`F-COLLAB-038`): computes a downsampled box-blurred copy
/// and blends it radially outside a centered head-and-shoulders ellipse.
pub fn apply_portrait_blur(rgb: &mut [u8], w: usize, h: usize) {
    if w < 16 || h < 16 || rgb.len() < w * h * 3 {
        return;
    }
    let bw = (w / 8).max(4);
    let bh = (h / 8).max(4);
    let mut small = vec![0u8; bw * bh * 3];
    for y in 0..bh {
        let sy0 = (y * h) / bh;
        let sy1 = (((y + 1) * h) / bh).max(sy0 + 1).min(h);
        for x in 0..bw {
            let sx0 = (x * w) / bw;
            let sx1 = (((x + 1) * w) / bw).max(sx0 + 1).min(w);
            let mut acc = [0u32; 3];
            let mut count = 0u32;
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let s = (sy * w + sx) * 3;
                    acc[0] += rgb[s] as u32;
                    acc[1] += rgb[s + 1] as u32;
                    acc[2] += rgb[s + 2] as u32;
                    count += 1;
                }
            }
            let d = (y * bw + x) * 3;
            small[d] = (acc[0] / count) as u8;
            small[d + 1] = (acc[1] / count) as u8;
            small[d + 2] = (acc[2] / count) as u8;
        }
    }
    // Two quick 3x3 box-blur passes on the downsampled image.
    let mut tmp = small.clone();
    for _ in 0..2 {
        for y in 0..bh {
            for x in 0..bw {
                let mut acc = [0u32; 3];
                let mut count = 0u32;
                for dy in -1i32..=1 {
                    let ny = (y as i32 + dy).clamp(0, bh as i32 - 1) as usize;
                    for dx in -1i32..=1 {
                        let nx = (x as i32 + dx).clamp(0, bw as i32 - 1) as usize;
                        let off = (ny * bw + nx) * 3;
                        acc[0] += small[off] as u32;
                        acc[1] += small[off + 1] as u32;
                        acc[2] += small[off + 2] as u32;
                        count += 1;
                    }
                }
                let dst = (y * bw + x) * 3;
                tmp[dst] = (acc[0] / count) as u8;
                tmp[dst + 1] = (acc[1] / count) as u8;
                tmp[dst + 2] = (acc[2] / count) as u8;
            }
        }
        small.copy_from_slice(&tmp);
    }

    let cx = w as f32 * 0.5;
    let cy = h as f32 * 0.52;
    let rx = w as f32 * 0.30;
    let ry = h as f32 * 0.44;

    for y in 0..h {
        let dy = (y as f32 - cy) / ry;
        let by = ((y * bh) / h).min(bh - 1);
        for x in 0..w {
            let dx = (x as f32 - cx) / rx;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= 0.85 {
                continue;
            }
            let alpha = ((dist - 0.85) / 0.45).clamp(0.0, 1.0);
            let inv = 1.0 - alpha;
            let bx = ((x * bw) / w).min(bw - 1);
            let boff = (by * bw + bx) * 3;
            let off = (y * w + x) * 3;
            rgb[off] = (rgb[off] as f32 * inv + small[boff] as f32 * alpha) as u8;
            rgb[off + 1] = (rgb[off + 1] as f32 * inv + small[boff + 1] as f32 * alpha) as u8;
            rgb[off + 2] = (rgb[off + 2] as f32 * inv + small[boff + 2] as f32 * alpha) as u8;
        }
    }
}

/// Active webcam capture & H.264 encoder running on a dedicated OS thread.
pub struct CameraCapture {
    running: Arc<AtomicBool>,
    blur: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl CameraCapture {
    pub fn start(
        device_index: Option<usize>,
        blur_background: bool,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        preview_tx: mpsc::UnboundedSender<VideoFrame>,
    ) -> Result<Self> {
        let running = Arc::new(AtomicBool::new(true));
        let blur = Arc::new(AtomicBool::new(blur_background));
        let run_flag = Arc::clone(&running);
        let blur_flag = Arc::clone(&blur);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);
        let idx = device_index.unwrap_or(0) as u32;

        let worker = std::thread::Builder::new()
            .name("slackless-huddle-camera".to_owned())
            .spawn(move || {
                let open = (|| -> Result<(Camera, H264VideoEncoder)> {
                    let req = RequestedFormat::new::<RgbFormat>(
                        RequestedFormatType::AbsoluteHighestFrameRate,
                    );
                    let mut cam = Camera::new(CameraIndex::Index(idx), req)
                        .map_err(|e| HuddleError::Video(format!("camera open failed: {e}")))?;
                    cam.open_stream()
                        .map_err(|e| HuddleError::Video(format!("camera stream failed: {e}")))?;
                    let enc = H264VideoEncoder::new()?;
                    Ok((cam, enc))
                })();

                let (mut cam, mut enc) = match open {
                    Ok(pair) => {
                        let _ = ready_tx.send(Ok(()));
                        pair
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };

                while run_flag.load(Ordering::Relaxed) {
                    if let Ok(frame) = cam.frame() {
                        if let Ok(img) = frame.decode_image::<RgbFormat>() {
                            let (mut rgb, w, h) = resize_rgb_even(
                                img.as_raw(),
                                img.width(),
                                img.height(),
                                CAMERA_MAX_W,
                                CAMERA_MAX_H,
                            );
                            if w >= 2 && h >= 2 {
                                if blur_flag.load(Ordering::Relaxed) {
                                    apply_portrait_blur(&mut rgb, w as usize, h as usize);
                                }
                                let bgra = rgb_to_bgra(&rgb);
                                let _ = preview_tx.send(VideoFrame {
                                    width: w,
                                    height: h,
                                    bgra: Arc::new(bgra),
                                });
                                if let Ok(h264) = enc.encode_rgb(&rgb, w as usize, h as usize) {
                                    if !h264.is_empty() {
                                        let _ = outbound_h264_tx.send(h264);
                                    }
                                }
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(66)); // ~15 fps
                }
                let _ = cam.stop_stream();
            })
            .map_err(|e| HuddleError::Video(e.to_string()))?;

        if let Err(err) = ready_rx
            .recv()
            .map_err(|e| HuddleError::Video(e.to_string()))
            .and_then(|r| r)
        {
            running.store(false, Ordering::Relaxed);
            let _ = worker.join();
            return Err(err);
        }

        Ok(Self {
            running,
            blur,
            worker: Some(worker),
        })
    }

    pub fn set_blur(&self, enabled: bool) {
        self.blur.store(enabled, Ordering::Relaxed);
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CameraCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Lightweight local camera preview probe for Preferences -> Audio & video (`F-COLLAB-046`).
pub struct CameraPreviewProbe {
    running: Arc<AtomicBool>,
    blur: Arc<AtomicBool>,
    latest: Arc<Mutex<Option<VideoFrame>>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl CameraPreviewProbe {
    pub fn start(device_index: Option<usize>, blur_background: bool) -> Result<Self> {
        let running = Arc::new(AtomicBool::new(true));
        let blur = Arc::new(AtomicBool::new(blur_background));
        let latest: Arc<Mutex<Option<VideoFrame>>> = Arc::new(Mutex::new(None));
        let run_flag = Arc::clone(&running);
        let blur_flag = Arc::clone(&blur);
        let latest_cb = Arc::clone(&latest);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);
        let idx = device_index.unwrap_or(0) as u32;

        let worker = std::thread::Builder::new()
            .name("slackless-cam-probe".to_owned())
            .spawn(move || {
                let open = (|| -> Result<Camera> {
                    let req = RequestedFormat::new::<RgbFormat>(
                        RequestedFormatType::AbsoluteHighestFrameRate,
                    );
                    let mut cam = Camera::new(CameraIndex::Index(idx), req)
                        .map_err(|e| HuddleError::Video(e.to_string()))?;
                    cam.open_stream()
                        .map_err(|e| HuddleError::Video(e.to_string()))?;
                    Ok(cam)
                })();
                let mut cam = match open {
                    Ok(c) => {
                        let _ = ready_tx.send(Ok(()));
                        c
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                while run_flag.load(Ordering::Relaxed) {
                    if let Ok(frame) = cam.frame() {
                        if let Ok(img) = frame.decode_image::<RgbFormat>() {
                            let (mut rgb, w, h) =
                                resize_rgb_even(img.as_raw(), img.width(), img.height(), 320, 180);
                            if w >= 2 && h >= 2 {
                                if blur_flag.load(Ordering::Relaxed) {
                                    apply_portrait_blur(&mut rgb, w as usize, h as usize);
                                }
                                let vf = VideoFrame {
                                    width: w,
                                    height: h,
                                    bgra: Arc::new(rgb_to_bgra(&rgb)),
                                };
                                if let Ok(mut guard) = latest_cb.lock() {
                                    *guard = Some(vf);
                                }
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(80));
                }
                let _ = cam.stop_stream();
            })
            .map_err(|e| HuddleError::Video(e.to_string()))?;

        ready_rx
            .recv()
            .map_err(|e| HuddleError::Video(e.to_string()))??;

        Ok(Self {
            running,
            blur,
            latest,
            worker: Some(worker),
        })
    }

    pub fn set_blur(&self, enabled: bool) {
        self.blur.store(enabled, Ordering::Relaxed);
    }

    pub fn latest_frame(&self) -> Option<VideoFrame> {
        self.latest.lock().ok().and_then(|g| g.clone())
    }
}

impl Drop for CameraPreviewProbe {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h264_encode_decode_round_trip_produces_bgra_frame() {
        let mut enc = H264VideoEncoder::new().unwrap();
        let mut dec = H264VideoDecoder::new().unwrap();
        let (w, h) = (160usize, 120usize);
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * 3;
                rgb[off] = ((x * 255) / w) as u8;
                rgb[off + 1] = ((y * 255) / h) as u8;
                rgb[off + 2] = 140;
            }
        }
        let bitstream = enc.encode_rgb(&rgb, w, h).unwrap();
        assert!(!bitstream.is_empty());
        let decoded = dec
            .decode_annex_b(&bitstream)
            .expect("keyframe should decode immediately");
        assert_eq!(decoded.width, w as u32);
        assert_eq!(decoded.height, h as u32);
        assert_eq!(decoded.bgra.len(), w * h * 4);
    }

    #[test]
    fn portrait_blur_preserves_center_and_smooths_outer_edges() {
        let (w, h) = (64usize, 64usize);
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * 3;
                let checker = if (x + y) % 2 == 0 { 240 } else { 20 };
                rgb[off] = checker;
                rgb[off + 1] = checker;
                rgb[off + 2] = checker;
            }
        }
        let center_before = rgb[(32 * w + 32) * 3];
        apply_portrait_blur(&mut rgb, w, h);
        // Center pixel inside subject oval stays crisp.
        assert_eq!(rgb[(32 * w + 32) * 3], center_before);
        // Corner pixel outside subject oval is blurred toward mid-gray.
        let corner = rgb[0];
        assert!(corner > 40 && corner < 220, "corner was {corner}");
    }
}
