//! Native H.264 video pipeline: webcam capture (`nokhwa`), software portrait background blur,
//! Cisco OpenH264 encode/decode (`openh264`), and RTP H.264 depacketization for Chime SFU.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nokhwa::Camera;
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
use openh264::OpenH264API;
use openh264::encoder::{
    BitRate, Complexity, Encoder as H264Encoder, EncoderConfig, FrameRate, IntraFramePeriod,
    RateControlMode, UsageType,
};
use openh264::formats::{RgbSliceU8, YUVBuffer, YUVSlices};
use rtc::rtp::codec::h264::H264Packet;
use rtc::rtp::packetizer::Depacketizer as _;
use slackless_types::UserId;
use tokio::sync::mpsc;

use crate::backend::{CameraConfig, CameraHandle, VideoDecoderHandle, platform_backend};
use crate::{HuddleError, Result};

/// Maximum dimensions for webcam capture (720p, even numbers required for YUV420).
pub const CAMERA_MAX_W: u32 = 1280;
pub const CAMERA_MAX_H: u32 = 720;
/// Target webcam frame rate for hardware capture.
pub const CAMERA_MAX_FPS: u32 = 24;
/// Target average H.264 bitrate for webcam streams (1.2 Mbps).
pub const CAMERA_BITRATE_BPS: u32 = 1_200_000;

/// Platform-extensible backing storage for a decoded or captured [`VideoFrame`].
#[derive(Debug, Clone)]
pub enum VideoFrameData {
    /// Portable tightly-packed BGRA8888 CPU pixel buffer.
    Bgra(Arc<Vec<u8>>),
    /// Zero-copy macOS `IOSurface`-backed `420f` `CVPixelBuffer` rendered directly by Metal.
    #[cfg(target_os = "macos")]
    MacSurface(Arc<crate::macos_media::MacVideoSurface>),
}

/// One decoded or captured video frame (zero-copy GPU surface on macOS, or portable BGRA8888).
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub data: VideoFrameData,
}

impl VideoFrame {
    /// Construct a portable BGRA8888 [`VideoFrame`].
    pub fn from_bgra(width: u32, height: u32, bgra: Vec<u8>) -> Self {
        Self {
            width,
            height,
            data: VideoFrameData::Bgra(Arc::new(bgra)),
        }
    }

    /// Wrap a zero-copy macOS `IOSurface`-backed `CVPixelBuffer`.
    #[cfg(target_os = "macos")]
    pub fn from_mac_surface(surface: crate::macos_media::MacVideoSurface) -> Self {
        Self {
            width: surface.width(),
            height: surface.height(),
            data: VideoFrameData::MacSurface(Arc::new(surface)),
        }
    }

    /// Borrow the underlying zero-copy macOS GPU surface if this frame is GPU-backed.
    #[cfg(target_os = "macos")]
    pub fn mac_surface(&self) -> Option<&Arc<crate::macos_media::MacVideoSurface>> {
        match &self.data {
            VideoFrameData::MacSurface(s) => Some(s),
            VideoFrameData::Bgra(_) => None,
        }
    }

    /// Extract owned BGRA8888 bytes without cloning when uniquely owned (`Arc::try_unwrap`),
    /// or convert from a GPU surface via hardware-accelerated readback.
    pub fn into_bgra(self) -> Option<(u32, u32, Vec<u8>)> {
        match self.data {
            VideoFrameData::Bgra(arc) => {
                let bytes = Arc::try_unwrap(arc).unwrap_or_else(|a| (*a).clone());
                Some((self.width, self.height, bytes))
            }
            #[cfg(target_os = "macos")]
            VideoFrameData::MacSurface(surface) => surface.to_bgra_fit(0, 0),
        }
    }

    /// Obtain BGRA8888 bytes fit within `(max_w, max_h)` (pass `0, 0` for native size).
    /// On macOS GPU surfaces, uses NEON `vImage` YCbCr->BGRA + high-quality downscaling.
    pub fn to_bgra_fit(&self, max_w: u32, max_h: u32) -> Option<(u32, u32, Vec<u8>)> {
        match &self.data {
            VideoFrameData::Bgra(arc) => Some((self.width, self.height, (**arc).clone())),
            #[cfg(target_os = "macos")]
            VideoFrameData::MacSurface(surface) => surface.to_bgra_fit(max_w, max_h),
        }
    }
}

impl PartialEq for VideoFrame {
    fn eq(&self, other: &Self) -> bool {
        if self.width != other.width || self.height != other.height {
            return false;
        }
        match (&self.data, &other.data) {
            (VideoFrameData::Bgra(a), VideoFrameData::Bgra(b)) => Arc::ptr_eq(a, b),
            #[cfg(target_os = "macos")]
            (VideoFrameData::MacSurface(a), VideoFrameData::MacSurface(b)) => Arc::ptr_eq(a, b),
            #[cfg(target_os = "macos")]
            _ => false,
        }
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

/// Enumerate all host cameras via the active platform [`crate::backend::MediaBackend`].
pub fn list_cameras() -> Vec<CameraDeviceInfo> {
    crate::backend::platform_backend().list_cameras()
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
            .adaptive_quantization(false)
            .background_detection(false)
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

/// Depacketize RTP H.264 payloads (Single NAL, STAP-A, FU-A) and decode via the active platform
/// [`crate::backend::MediaBackend`] (hardware VideoToolbox zero-copy on macOS with lazy OpenH264 fallback).
pub struct H264VideoDecoder {
    inner: Box<dyn VideoDecoderHandle>,
    depacketizer: H264Packet,
    annex_b_buf: Vec<u8>,
    last_seq: Option<u16>,
    broken: bool,
    seq_gaps: u64,
    needs_keyframe: bool,
}

impl H264VideoDecoder {
    pub fn new() -> Result<Self> {
        let inner = platform_backend().create_h264_decoder()?;
        Ok(Self {
            inner,
            depacketizer: H264Packet::default(),
            annex_b_buf: Vec::with_capacity(64 * 1024),
            last_seq: None,
            broken: false,
            seq_gaps: 0,
            needs_keyframe: false,
        })
    }

    pub fn backend_name(&self) -> &'static str {
        self.inner.backend_name()
    }

    /// Total number of RTP 16-bit sequence gaps observed on this stream.
    pub fn seq_gaps(&self) -> u64 {
        self.seq_gaps
    }

    /// Returns `true` (and clears the flag) if a sequence gap was detected and the caller should
    /// request an immediate PLI keyframe from the sender.
    pub fn take_needs_keyframe(&mut self) -> bool {
        std::mem::take(&mut self.needs_keyframe)
    }

    /// Feed one RTP packet (`seq` = 16-bit RTP sequence number, `marker` = RTP marker bit
    /// indicating end of access unit).
    ///
    /// Tracks 16-bit wrapping sequence continuity so mid-frame loss discards damaged partial
    /// access units, ignores trailing fragments until `marker == true` completes the damaged unit,
    /// flags `needs_keyframe`, and recovers cleanly on the next access unit.
    pub fn push_rtp_packet(
        &mut self,
        seq: u16,
        payload: &[u8],
        marker: bool,
    ) -> Option<VideoFrame> {
        let is_fua_continuation =
            payload.len() >= 2 && (payload[0] & 0x1F) == 28 && (payload[1] & 0x80) == 0;

        let missing_pkts = match self.last_seq {
            None => {
                self.last_seq = Some(seq);
                if is_fua_continuation {
                    self.broken = !marker;
                    return None;
                }
                0
            }
            Some(prev) => {
                let diff = seq.wrapping_sub(prev) as i16;
                if diff <= 0 {
                    // Late/reordered or duplicate RTP packet; drop without inflating loss or
                    // regressing `last_seq`.
                    return None;
                }
                self.last_seq = Some(seq);
                (diff - 1) as u64
            }
        };

        if missing_pkts > 0 {
            self.seq_gaps = self.seq_gaps.saturating_add(missing_pkts);
            self.needs_keyframe = true;
            self.depacketizer = H264Packet::default();
            if !self.annex_b_buf.is_empty() {
                self.annex_b_buf.clear();
                self.broken = true;
            } else if is_fua_continuation {
                self.broken = !marker;
                return None;
            }
        }

        if self.broken {
            if marker {
                self.broken = false;
                self.annex_b_buf.clear();
                self.depacketizer = H264Packet::default();
            }
            return None;
        }

        let raw = bytes::Bytes::copy_from_slice(payload);
        if let Ok(nal) = self.depacketizer.depacketize(&raw)
            && !nal.is_empty()
        {
            self.annex_b_buf.extend_from_slice(&nal);
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

    /// Feed one RTP payload (`marker` = RTP marker bit indicating end of access unit) using
    /// monotonically synthesized sequence numbers for backwards compatibility.
    pub fn push_rtp_payload(&mut self, payload: &[u8], marker: bool) -> Option<VideoFrame> {
        let next_seq = self.last_seq.map_or(0, |prev| prev.wrapping_add(1));
        self.push_rtp_packet(next_seq, payload, marker)
    }

    /// Decode a complete Annex-B H.264 access unit into a [`VideoFrame`].
    pub fn decode_annex_b(&mut self, bitstream: &[u8]) -> Option<VideoFrame> {
        self.inner.decode_unit(bitstream)
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

/// Active webcam capture & H.264 encoder backed by the active platform [`crate::backend::MediaBackend`].
///
/// - macOS: hardware `AVCaptureSession` (`420f`) + `VideoToolbox` H.264 High + Vision/Metal blur
/// - Linux/Windows/fallback: `nokhwa` + `OpenH264`
pub struct CameraCapture {
    inner: Box<dyn CameraHandle>,
}

impl CameraCapture {
    pub fn start(
        device_index: Option<usize>,
        blur_background: bool,
        compact: bool,
        outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
        preview_tx: mpsc::UnboundedSender<VideoFrame>,
    ) -> Result<Self> {
        let (max_w, max_h, bitrate_bps) = if compact {
            (640, 480, 500_000)
        } else {
            (CAMERA_MAX_W, CAMERA_MAX_H, CAMERA_BITRATE_BPS)
        };
        let inner = platform_backend().start_camera(
            CameraConfig {
                device_index,
                max_w,
                max_h,
                max_fps: CAMERA_MAX_FPS,
                bitrate_bps,
                blur_background,
            },
            outbound_h264_tx,
            preview_tx,
        )?;
        Ok(Self { inner })
    }

    pub fn set_blur(&self, enabled: bool) {
        self.inner.set_blur(enabled);
    }

    pub fn set_compact(&self, compact: bool) {
        self.inner.set_compact(compact);
    }

    pub fn force_keyframe(&self) {
        self.inner.force_keyframe();
    }

    pub fn set_target_bitrate(&self, bitrate_bps: u32) {
        self.inner.set_target_bitrate(bitrate_bps);
    }

    pub fn stop(&mut self) {
        self.inner.stop();
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
                    if let Ok(frame) = cam.frame()
                        && let Ok(img) = frame.decode_image::<RgbFormat>()
                    {
                        let (mut rgb, w, h) =
                            resize_rgb_even(img.as_raw(), img.width(), img.height(), 320, 180);
                        if w >= 2 && h >= 2 {
                            if blur_flag.load(Ordering::Relaxed) {
                                apply_portrait_blur(&mut rgb, w as usize, h as usize);
                            }
                            let vf = VideoFrame::from_bgra(w, h, rgb_to_bgra(&rgb));
                            if let Ok(mut guard) = latest_cb.lock() {
                                *guard = Some(vf);
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
        let (bw, bh, bgra) = decoded.into_bgra().expect("frame should convert to BGRA");
        assert_eq!((bw, bh), (w as u32, h as u32));
        assert_eq!(bgra.len(), w * h * 4);
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

    fn split_annex_b_for_test(data: &[u8]) -> Vec<&[u8]> {
        let mut starts = Vec::new();
        let mut i = 0;
        while i + 3 <= data.len() {
            if data[i] == 0 && data[i + 1] == 0 {
                if i + 4 <= data.len() && data[i + 2] == 0 && data[i + 3] == 1 {
                    starts.push((i, 4));
                    i += 4;
                    continue;
                }
                if data[i + 2] == 1 {
                    starts.push((i, 3));
                    i += 3;
                    continue;
                }
            }
            i += 1;
        }
        starts
            .iter()
            .enumerate()
            .filter_map(|(idx, &(pos, sc))| {
                let end = starts.get(idx + 1).map_or(data.len(), |&(n, _)| n);
                let nal = &data[pos + sc..end];
                (!nal.is_empty()).then_some(nal)
            })
            .collect()
    }

    fn encode_test_keyframe(w: usize, h: usize, r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut enc = H264VideoEncoder::new_camera().unwrap();
        let mut rgb = vec![0u8; w * h * 3];
        for px in rgb.as_chunks_mut::<3>().0 {
            px[0] = r;
            px[1] = g;
            px[2] = b;
        }
        enc.encode_rgb(&rgb, w, h).unwrap()
    }

    #[test]
    fn h264_decoder_push_rtp_single_nal_stap_a_fua_reset_and_malformed() {
        let (w, h) = (96usize, 64usize);
        let annex_b = encode_test_keyframe(w, h, 40, 180, 220);
        let nals = split_annex_b_for_test(&annex_b);
        assert!(nals.len() >= 3, "expected SPS, PPS, IDR NALs");

        // 1. Single NAL RTP payloads (one NAL per packet, marker=true on final NAL).
        let mut dec = H264VideoDecoder::new().unwrap();
        let mut decoded = None;
        for (i, nal) in nals.iter().enumerate() {
            let marker = i + 1 == nals.len();
            let out = dec.push_rtp_payload(nal, marker);
            if !marker {
                assert!(out.is_none());
            } else {
                decoded = out;
            }
        }
        let frame = decoded.expect("Single NAL sequence must decode");
        assert_eq!((frame.width, frame.height), (w as u32, h as u32));
        // Buffer resets after decode: malformed or empty marker must return None.
        assert!(dec.push_rtp_payload(&[], true).is_none());

        // 2. STAP-A (NAL type 24) aggregating all NALs into a single RTP packet.
        let mut stap_a = vec![24u8];
        for nal in &nals {
            stap_a.extend_from_slice(&(nal.len() as u16).to_be_bytes());
            stap_a.extend_from_slice(nal);
        }
        let mut dec_stap = H264VideoDecoder::new().unwrap();
        let frame_stap = dec_stap
            .push_rtp_payload(&stap_a, true)
            .expect("STAP-A packet must decode");
        assert_eq!((frame_stap.width, frame_stap.height), (w as u32, h as u32));

        // 3. Multi-packet FU-A fragmentation (NAL type 28) on the final IDR NAL.
        let mut dec_fua = H264VideoDecoder::new().unwrap();
        for nal in &nals[..nals.len() - 1] {
            assert!(dec_fua.push_rtp_payload(nal, false).is_none());
        }
        let idr = nals[nals.len() - 1];
        assert!(idr.len() > 6, "IDR should be large enough to fragment");
        let nri = idr[0] & 0x60;
        let nal_type = idr[0] & 0x1F;
        let body = &idr[1..];
        let c1 = body.len() / 3;
        let c2 = (body.len() * 2) / 3;
        let chunks = [&body[..c1], &body[c1..c2], &body[c2..]];
        let mut fua_decoded = None;
        for (i, chunk) in chunks.iter().enumerate() {
            let mut pkt = Vec::with_capacity(2 + chunk.len());
            pkt.push(nri | 28); // FU indicator
            let mut fu_hdr = nal_type;
            if i == 0 {
                fu_hdr |= 0x80; // Start bit
            }
            if i + 1 == chunks.len() {
                fu_hdr |= 0x40; // End bit
            }
            pkt.push(fu_hdr);
            pkt.extend_from_slice(chunk);
            let marker = i + 1 == chunks.len();
            let res = dec_fua.push_rtp_payload(&pkt, marker);
            if !marker {
                assert!(res.is_none());
            } else {
                fua_decoded = res;
            }
        }
        let frame_fua = fua_decoded.expect("FU-A fragmented IDR must decode on marker");
        assert_eq!((frame_fua.width, frame_fua.height), (w as u32, h as u32));

        // 4. Malformed payloads (truncated STAP-A, truncated FU-A) never panic.
        assert!(
            dec_fua
                .push_rtp_payload(&[24, 0x00, 0x20, 0x01], true)
                .is_none()
        );
        assert!(dec_fua.push_rtp_payload(&[28], true).is_none());
        assert!(dec_fua.push_rtp_payload(&[0xFF], false).is_none());
    }

    #[test]
    fn downscale_rgba_area_bt601_bgra_odd_dims_scratch_reuse_and_screen_round_trip() {
        let mut scratch = ScreenScaleScratch::default();

        // Short-buffer & zero-dim guards.
        assert_eq!(
            downscale_rgba_area_to_bgra_and_i420(&[], 0, 10, 64, 64, &mut scratch),
            (0, 0)
        );
        assert_eq!(
            downscale_rgba_area_to_bgra_and_i420(&[0u8; 12], 2, 2, 64, 64, &mut scratch),
            (0, 0)
        );

        // Pure white (255,255,255,255) -> BT.601 Y=235, U=128, V=128, BGRA=[255,255,255,255].
        let white = vec![255u8; 8 * 8 * 4];
        let (dw, dh) = downscale_rgba_area_to_bgra_and_i420(&white, 8, 8, 8, 8, &mut scratch);
        assert_eq!((dw, dh), (8, 8));
        assert!(scratch.y.iter().all(|&y| y == 235));
        assert!(scratch.u.iter().all(|&u| u == 128));
        assert!(scratch.v.iter().all(|&v| v == 128));
        assert!(
            scratch
                .bgra
                .as_chunks::<4>()
                .0
                .iter()
                .all(|px| *px == [255, 255, 255, 255])
        );

        // Pure red (255,0,0,123) -> BGRA=[0,0,255,255], Y=81, U=90, V=240 (within +-1).
        let mut red = Vec::with_capacity(13 * 9 * 4);
        for _ in 0..(13 * 9) {
            red.extend_from_slice(&[255, 0, 0, 123]);
        }
        // Odd source dims (13x9) downscaled to even target dims inside (10x8).
        let (dw2, dh2) = downscale_rgba_area_to_bgra_and_i420(&red, 13, 9, 10, 8, &mut scratch);
        assert!((2..=10).contains(&dw2) && dw2 % 2 == 0);
        assert!((2..=8).contains(&dh2) && dh2 % 2 == 0);
        assert_eq!(scratch.bgra.len(), (dw2 * dh2 * 4) as usize);
        assert_eq!(scratch.y.len(), (dw2 * dh2) as usize);
        assert_eq!(scratch.u.len(), ((dw2 / 2) * (dh2 / 2)) as usize);
        assert_eq!(scratch.v.len(), ((dw2 / 2) * (dh2 / 2)) as usize);
        assert_eq!(&scratch.bgra[0..4], &[0, 0, 255, 255]);
        assert!((scratch.y[0] as i32 - 81).abs() <= 1);
        assert!((scratch.u[0] as i32 - 90).abs() <= 1);
        assert!((scratch.v[0] as i32 - 240).abs() <= 1);

        // Reuse scratch for larger 128x96 -> 64x48 gradient and round-trip via H264 screen encoder.
        let mut grad = vec![0u8; 128 * 96 * 4];
        for y in 0..96usize {
            for x in 0..128usize {
                let off = (y * 128 + x) * 4;
                grad[off] = (x * 2) as u8;
                grad[off + 1] = (y * 2) as u8;
                grad[off + 2] = 160;
                grad[off + 3] = 255;
            }
        }
        let (sw, sh) = downscale_rgba_area_to_bgra_and_i420(&grad, 128, 96, 64, 48, &mut scratch);
        assert_eq!((sw, sh), (64, 48));
        let mut enc = H264VideoEncoder::new_screen().unwrap();
        let h264 = enc
            .encode_i420(&scratch.y, &scratch.u, &scratch.v, sw as usize, sh as usize)
            .expect("screen encode_i420");
        assert!(!h264.is_empty());
        let mut dec = H264VideoDecoder::new().unwrap();
        let decoded = dec.decode_annex_b(&h264).expect("decode screen frame");
        assert_eq!((decoded.width, decoded.height), (64, 48));
    }

    #[test]
    fn pixel_helpers_video_frame_into_bgra_and_blur_guards() {
        // resize_rgb_even: zero dims, passthrough, and odd downscale.
        assert_eq!(resize_rgb_even(&[], 0, 10, 64, 64), (Vec::new(), 0, 0));
        let passthrough = vec![10u8, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120];
        let (same, w, h) = resize_rgb_even(&passthrough, 2, 2, 10, 10);
        assert_eq!((w, h), (2, 2));
        assert_eq!(same, passthrough);
        let solid = vec![100u8; 15 * 11 * 3];
        let (resized, rw, rh) = resize_rgb_even(&solid, 15, 11, 8, 8);
        assert!((2..=8).contains(&rw) && rw % 2 == 0);
        assert!((2..=8).contains(&rh) && rh % 2 == 0);
        assert_eq!(resized.len(), (rw * rh * 3) as usize);
        assert!(resized.iter().all(|&b| b == 100));

        // rgb_to_bgra & rgba_to_rgb_and_bgra.
        assert_eq!(
            rgb_to_bgra(&[10, 20, 30, 40, 50, 60, 99]),
            vec![30, 20, 10, 255, 60, 50, 40, 255]
        );
        let (rgb, bgra) = rgba_to_rgb_and_bgra(&[11, 22, 33, 44, 55, 66, 77, 88, 99]);
        assert_eq!(rgb, vec![11, 22, 33, 55, 66, 77]);
        assert_eq!(bgra, vec![33, 22, 11, 44, 77, 66, 55, 88]);

        // VideoFrame::into_bgra: unique Arc moves without cloning; shared Arc clones cleanly.
        let unique_bytes = vec![1u8, 2, 3, 255];
        let orig_ptr = unique_bytes.as_ptr();
        let vf_unique = VideoFrame::from_bgra(1, 1, unique_bytes);
        let (uw, uh, moved) = vf_unique.into_bgra().unwrap();
        assert_eq!((uw, uh), (1, 1));
        assert_eq!(
            moved.as_ptr(),
            orig_ptr,
            "unique Arc should unwrap without reallocation"
        );

        let vf_shared_a = VideoFrame::from_bgra(1, 1, vec![9, 8, 7, 255]);
        let vf_shared_b = vf_shared_a.clone();
        assert_eq!(vf_shared_a, vf_shared_b);
        let (_, _, cloned) = vf_shared_a.into_bgra().unwrap();
        assert_eq!(cloned, vec![9, 8, 7, 255]);
        assert_eq!(vf_shared_b.to_bgra_fit(0, 0).unwrap().2, vec![9, 8, 7, 255]);

        // apply_portrait_blur guards: small dims and short buffers are untouched.
        let mut small = vec![42u8; 8 * 8 * 3];
        apply_portrait_blur(&mut small, 8, 8);
        assert!(small.iter().all(|&b| b == 42));
        let mut short = vec![77u8; 16 * 16 * 3 - 1];
        apply_portrait_blur(&mut short, 16, 16);
        assert!(short.iter().all(|&b| b == 77));
    }

    fn build_stap_a(nals: &[&[u8]]) -> Vec<u8> {
        let mut stap_a = vec![24u8];
        for nal in nals {
            stap_a.extend_from_slice(&(nal.len() as u16).to_be_bytes());
            stap_a.extend_from_slice(nal);
        }
        stap_a
    }

    fn build_fua_packets(nal: &[u8], parts: usize) -> Vec<Vec<u8>> {
        let nri = nal[0] & 0x60;
        let nal_type = nal[0] & 0x1F;
        let body = &nal[1..];
        let mut out = Vec::with_capacity(parts);
        for i in 0..parts {
            let start = (body.len() * i) / parts;
            let end = (body.len() * (i + 1)) / parts;
            let chunk = &body[start..end];
            let mut pkt = Vec::with_capacity(2 + chunk.len());
            pkt.push(nri | 28);
            let mut fu_hdr = nal_type;
            if i == 0 {
                fu_hdr |= 0x80; // Start bit
            }
            if i + 1 == parts {
                fu_hdr |= 0x40; // End bit
            }
            pkt.push(fu_hdr);
            pkt.extend_from_slice(chunk);
            out.push(pkt);
        }
        out
    }

    #[test]
    fn h264_decoder_push_rtp_packet_single_nal_stap_a_fua_and_seq_wraparound() {
        let (w, h) = (96usize, 64usize);
        let annex_b = encode_test_keyframe(w, h, 30, 150, 210);
        let nals = split_annex_b_for_test(&annex_b);
        assert!(nals.len() >= 3, "expected SPS, PPS, IDR NALs");

        // 1. Single NAL sequence via push_rtp_packet.
        let mut dec = H264VideoDecoder::new().unwrap();
        let mut decoded = None;
        for (i, nal) in nals.iter().enumerate() {
            let marker = i + 1 == nals.len();
            let out = dec.push_rtp_packet(100 + i as u16, nal, marker);
            if marker {
                decoded = out;
            } else {
                assert!(out.is_none());
            }
        }
        let f1 = decoded.expect("Single NAL packets should decode cleanly");
        assert_eq!((f1.width, f1.height), (w as u32, h as u32));
        assert_eq!(dec.seq_gaps(), 0);
        assert!(!dec.take_needs_keyframe());

        // 2. STAP-A packet at next contiguous seq.
        let stap_a = build_stap_a(&nals);
        let next_seq = 100 + nals.len() as u16;
        let f2 = dec
            .push_rtp_packet(next_seq, &stap_a, true)
            .expect("STAP-A packet should decode cleanly");
        assert_eq!((f2.width, f2.height), (w as u32, h as u32));
        assert_eq!(dec.seq_gaps(), 0);
        assert!(!dec.take_needs_keyframe());

        // 3. Multi-packet FU-A spanning 16-bit sequence wraparound (65534 -> 65535 -> 0 -> 1).
        let mut dec_wrap = H264VideoDecoder::new().unwrap();
        let prefix = &nals[..nals.len() - 1];
        let idr = nals[nals.len() - 1];
        let fua_pkts = build_fua_packets(idr, 3);
        let mut seq: u16 = 65535u16.wrapping_sub(prefix.len() as u16);
        for nal in prefix {
            assert!(dec_wrap.push_rtp_packet(seq, nal, false).is_none());
            seq = seq.wrapping_add(1);
        }
        // seq is now 65535; FU-A chunks traverse 65535 -> 0 -> 1 with zero gaps!
        assert_eq!(seq, 65535);
        assert!(dec_wrap.push_rtp_packet(seq, &fua_pkts[0], false).is_none());
        seq = seq.wrapping_add(1);
        assert_eq!(seq, 0);
        assert!(dec_wrap.push_rtp_packet(seq, &fua_pkts[1], false).is_none());
        seq = seq.wrapping_add(1);
        assert_eq!(seq, 1);
        let wrapped_frame = dec_wrap
            .push_rtp_packet(seq, &fua_pkts[2], true)
            .expect("FU-A crossing 65535 -> 0 wraparound must decode without gap");
        assert_eq!(
            (wrapped_frame.width, wrapped_frame.height),
            (w as u32, h as u32)
        );
        assert_eq!(dec_wrap.seq_gaps(), 0);
        assert!(!dec_wrap.take_needs_keyframe());
    }

    #[test]
    fn h264_decoder_mid_frame_seq_gap_discards_partial_fua_and_recovers_cleanly() {
        let (w, h) = (96usize, 64usize);
        let annex_b = encode_test_keyframe(w, h, 200, 80, 50);
        let nals = split_annex_b_for_test(&annex_b);
        let prefix = &nals[..nals.len() - 1];
        let idr = nals[nals.len() - 1];
        let fua4 = build_fua_packets(idr, 4);
        let stap_a = build_stap_a(&nals);

        // Prime decoder with one clean frame at seq 10.
        let mut dec = H264VideoDecoder::new().unwrap();
        assert!(dec.push_rtp_packet(10, &stap_a, true).is_some());
        assert_eq!(dec.seq_gaps(), 0);
        assert!(!dec.take_needs_keyframe());

        // Start a fragmented frame: prefix NALs + FU-A chunk 0, then DROP chunk 1!
        let mut seq = 11u16;
        for nal in prefix {
            assert!(dec.push_rtp_packet(seq, nal, false).is_none());
            seq = seq.wrapping_add(1);
        }
        assert!(dec.push_rtp_packet(seq, &fua4[0], false).is_none());
        // Skip chunk 1 (`seq + 1`), deliver chunk 2 (`seq + 2`, marker=false) and chunk 3 (`seq + 3`, marker=true).
        let seq_after_drop = seq.wrapping_add(2);
        assert!(
            dec.push_rtp_packet(seq_after_drop, &fua4[2], false)
                .is_none()
        );
        assert_eq!(dec.seq_gaps(), 1);
        assert!(dec.take_needs_keyframe(), "gap must flag needs_keyframe");
        assert!(
            !dec.take_needs_keyframe(),
            "take_needs_keyframe must clear flag after reading"
        );
        // Trailing end fragment of damaged AU must also be discarded, clearing `broken` on marker=true.
        assert!(
            dec.push_rtp_packet(seq_after_drop.wrapping_add(1), &fua4[3], true)
                .is_none()
        );
        assert_eq!(dec.seq_gaps(), 1);

        // Next clean frame recovers immediately!
        let recovered = dec
            .push_rtp_packet(seq_after_drop.wrapping_add(2), &stap_a, true)
            .expect("decoder must recover cleanly after damaged AU ends");
        assert_eq!((recovered.width, recovered.height), (w as u32, h as u32));
        assert!(!dec.take_needs_keyframe());
    }

    #[test]
    fn h264_decoder_gap_on_empty_buf_fua_continuation_drops_damaged_au_and_recovers() {
        let (w, h) = (96usize, 64usize);
        let annex_b = encode_test_keyframe(w, h, 60, 220, 90);
        let nals = split_annex_b_for_test(&annex_b);
        let idr = nals[nals.len() - 1];
        let fua3 = build_fua_packets(idr, 3);
        let stap_a = build_stap_a(&nals);

        let mut dec = H264VideoDecoder::new().unwrap();
        // Clean frame at seq 50 leaves `annex_b_buf` empty.
        assert!(dec.push_rtp_packet(50, &stap_a, true).is_some());
        assert_eq!(dec.seq_gaps(), 0);

        // Case A: Gap lands on mid-FU-A continuation (S=0, marker=false) -> drops it and sets broken=true.
        // Packet 51 (FU-A S=1) was lost; packet 52 (FU-A S=0, marker=false) arrives.
        assert!(dec.push_rtp_packet(52, &fua3[1], false).is_none());
        assert_eq!(dec.seq_gaps(), 1);
        assert!(dec.take_needs_keyframe());
        // Packet 53 (FU-A S=0, E=1, marker=true) closes the damaged AU and clears broken.
        assert!(dec.push_rtp_packet(53, &fua3[2], true).is_none());
        assert_eq!(dec.seq_gaps(), 1);
        assert!(!dec.take_needs_keyframe());
        // Packet 54 clean STAP-A decodes cleanly.
        assert!(dec.push_rtp_packet(54, &stap_a, true).is_some());

        // Case B: Gap lands on final FU-A continuation (S=0, E=1, marker=true) -> drops it, broken=false immediately.
        assert!(dec.push_rtp_packet(56, &fua3[2], true).is_none());
        assert_eq!(dec.seq_gaps(), 2);
        assert!(dec.take_needs_keyframe());
        // Immediately next packet 57 decodes cleanly.
        let f = dec
            .push_rtp_packet(57, &stap_a, true)
            .expect("broken must already be cleared when S=0 continuation had marker=true");
        assert_eq!((f.width, f.height), (w as u32, h as u32));

        // Case C: Inter-frame burst gap (57 -> 62 skips 4 packets: 58..=61) landing on a clean
        // STAP-A keyframe: accumulates all 4 missing packets + needs_keyframe, yet decodes cleanly.
        let f_clean = dec
            .push_rtp_packet(62, &stap_a, true)
            .expect("clean new keyframe after inter-frame gap should decode");
        assert_eq!((f_clean.width, f_clean.height), (w as u32, h as u32));
        assert_eq!(dec.seq_gaps(), 6);
        assert!(dec.take_needs_keyframe());

        // Case D: Late/reordered packet (seq 60 < 62) is dropped without inflating loss or
        // regressing `last_seq`, so contiguous packet 63 still decodes with zero extra gaps.
        assert!(dec.push_rtp_packet(60, &stap_a, true).is_none());
        assert_eq!(dec.seq_gaps(), 6);
        assert!(!dec.take_needs_keyframe());
        assert!(dec.push_rtp_packet(63, &stap_a, true).is_some());
        assert_eq!(dec.seq_gaps(), 6);
        assert!(!dec.take_needs_keyframe());
    }

    #[test]
    #[ignore = "benchmark; run with --release -- --ignored --nocapture"]
    fn bench_video_scale_and_h264_encode() {
        use std::time::Instant;

        const FRAMES: usize = 30;
        let (src_w, src_h) = (1280u32, 720u32);
        let (max_w, max_h) = (640u32, 360u32);

        let mut rgba = vec![0u8; (src_w * src_h * 4) as usize];
        for y in 0..src_h as usize {
            for x in 0..src_w as usize {
                let off = (y * src_w as usize + x) * 4;
                rgba[off] = ((x * 255) / src_w as usize) as u8;
                rgba[off + 1] = ((y * 255) / src_h as usize) as u8;
                rgba[off + 2] = ((x ^ y) & 0xff) as u8;
                rgba[off + 3] = 255;
            }
        }

        let mut scratch = ScreenScaleScratch::default();
        let mut enc = H264VideoEncoder::new_camera().unwrap();

        // Warm up scratch allocation & OpenH264 internal buffers.
        let (dst_w, dst_h) =
            downscale_rgba_area_to_bgra_and_i420(&rgba, src_w, src_h, max_w, max_h, &mut scratch);
        let _ = enc
            .encode_i420(
                &scratch.y,
                &scratch.u,
                &scratch.v,
                dst_w as usize,
                dst_h as usize,
            )
            .unwrap();

        let mut scale_ns = 0u128;
        let mut enc_ns = 0u128;
        let mut total_bytes = 0usize;
        let t_all = Instant::now();

        for i in 0..FRAMES {
            // Mutate a small stripe per frame so temporal compression behaves realistically.
            let stripe_row = (i * 17) % src_h as usize;
            let row_start = stripe_row * src_w as usize * 4;
            for b in &mut rgba[row_start..row_start + 256] {
                *b = b.wrapping_add(29);
            }

            let t0 = Instant::now();
            let (dw, dh) = downscale_rgba_area_to_bgra_and_i420(
                &rgba,
                src_w,
                src_h,
                max_w,
                max_h,
                &mut scratch,
            );
            scale_ns += t0.elapsed().as_nanos();

            let t1 = Instant::now();
            let bs = enc
                .encode_i420(&scratch.y, &scratch.u, &scratch.v, dw as usize, dh as usize)
                .unwrap();
            enc_ns += t1.elapsed().as_nanos();
            total_bytes += bs.len();
        }

        let total_secs = t_all.elapsed().as_secs_f64().max(1e-9);
        let total_ms = total_secs * 1000.0;
        let fps = FRAMES as f64 / total_secs;
        let ms_per_frame = total_ms / FRAMES as f64;
        let scale_ms = (scale_ns as f64 / 1_000_000.0) / FRAMES as f64;
        let encode_ms = (enc_ns as f64 / 1_000_000.0) / FRAMES as f64;
        let avg_bytes = total_bytes / FRAMES;
        println!(
            "bench_video_scale_and_h264_encode: {FRAMES} frames ({src_w}x{src_h} -> {dst_w}x{dst_h}) in {total_ms:.2} ms | {fps:.1} frames/sec | {ms_per_frame:.2} ms/frame (scale {scale_ms:.2} ms, encode {encode_ms:.2} ms, avg {avg_bytes} B/frame)"
        );
        assert_eq!((dst_w, dst_h), (640, 360));
        assert!(total_bytes > 0);
    }
}
