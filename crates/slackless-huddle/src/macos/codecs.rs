//! VideoToolbox Hardware H.264 & AV1 Decoders (Zero-Copy `420f` Surface Output).

use std::ffi::c_void;

use super::ffi::{
    SlacklessVtAv1Decoder, SlacklessVtDecoder, slackless_cvpb_release,
    slackless_vt_av1_decoder_decode, slackless_vt_av1_decoder_free, slackless_vt_av1_decoder_new,
    slackless_vt_decoder_decode, slackless_vt_decoder_free, slackless_vt_decoder_new,
};
use super::pixel::MacVideoSurface;
use crate::video::VideoFrame;
use crate::{HuddleError, Result};

unsafe extern "C" fn on_decoded_surface_cb(
    ctx: *mut c_void,
    cv_pb: *mut c_void,
    width: u32,
    height: u32,
) {
    if ctx.is_null() || cv_pb.is_null() {
        if !cv_pb.is_null() {
            unsafe { slackless_cvpb_release(cv_pb) };
        }
        return;
    }
    let out = unsafe { &mut *(ctx as *mut Option<VideoFrame>) };
    if let Some(surface) = unsafe { MacVideoSurface::from_retained(cv_pb, width, height) } {
        *out = Some(VideoFrame::from_mac_surface(surface));
    }
}

/// Hardware-accelerated macOS `VideoToolbox` H.264 Annex-B decoder producing zero-copy `420f` surfaces.
pub struct VtH264Decoder {
    handle: *mut SlacklessVtDecoder,
}

// SAFETY: `SlacklessVtDecoder` is owned exclusively and accessed via `&mut self`.
unsafe impl Send for VtH264Decoder {}

impl VtH264Decoder {
    pub fn new() -> Result<Self> {
        let handle = unsafe { slackless_vt_decoder_new() };
        if handle.is_null() {
            return Err(HuddleError::Codec(
                "failed to allocate VideoToolbox H.264 decoder".into(),
            ));
        }
        Ok(Self { handle })
    }

    pub fn decode_annex_b(&mut self, annex_b: &[u8]) -> Option<VideoFrame> {
        if self.handle.is_null() || annex_b.is_empty() {
            return None;
        }
        let mut frame: Option<VideoFrame> = None;
        let ok = unsafe {
            slackless_vt_decoder_decode(
                self.handle,
                annex_b.as_ptr(),
                annex_b.len(),
                (&mut frame as *mut Option<VideoFrame>).cast(),
                on_decoded_surface_cb,
            )
        };
        if ok { frame } else { None }
    }
}

impl Drop for VtH264Decoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { slackless_vt_decoder_free(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

/// Runtime-gated macOS `VideoToolbox` hardware AV1 decoder (M3+ on macOS 14+; returns `Err` on
/// M1/M2 so callers cleanly fall back to `rav1d`). Produces zero-copy `420f` surfaces.
pub struct VtAv1Decoder {
    handle: *mut SlacklessVtAv1Decoder,
}

// SAFETY: `SlacklessVtAv1Decoder` is owned exclusively and accessed via `&mut self`.
unsafe impl Send for VtAv1Decoder {}

impl VtAv1Decoder {
    pub fn new() -> Result<Self> {
        let handle = unsafe { slackless_vt_av1_decoder_new() };
        if handle.is_null() {
            return Err(HuddleError::Codec(
                "hardware AV1 decode not supported on this Mac".into(),
            ));
        }
        Ok(Self { handle })
    }

    pub fn decode_temporal_unit(&mut self, unit: &[u8]) -> Option<VideoFrame> {
        if self.handle.is_null() || unit.is_empty() {
            return None;
        }
        let mut frame: Option<VideoFrame> = None;
        let ok = unsafe {
            slackless_vt_av1_decoder_decode(
                self.handle,
                unit.as_ptr(),
                unit.len(),
                (&mut frame as *mut Option<VideoFrame>).cast(),
                on_decoded_surface_cb,
            )
        };
        if ok { frame } else { None }
    }
}

impl Drop for VtAv1Decoder {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { slackless_vt_av1_decoder_free(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}
