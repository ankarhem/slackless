//! Native audio and video clip recording (`F-CORE-127`, `F-CORE-128`, `F-CORE-131`):
//! - [`AudioClipRecorder`]: captures microphone via `cpal` + `sonora` DSP, computes live RMS
//!   bars, encodes PCM WAV (or compact AAC `.m4a` via macOS `afconvert` when available).
//! - [`VideoClipRecorder`]: captures webcam (`AVFoundation` on macOS, `nokhwa` on Linux/Windows)
//!   or screen/window (`xcap`), encodes H.264 via Cisco OpenH264, and muxes a valid ISO-BMFF
//!   `.mp4` (`ftyp`/`mdat`/`moov`/`avcC`) in pure Rust with zero external binary requirements.

mod audio;
mod mp4;
mod video;

#[cfg(test)]
mod tests;

use std::time::{SystemTime, UNIX_EPOCH};

use crate::audio::SAMPLE_RATE;
use crate::video::VideoFrame;

pub use audio::{
    AacAudioTrack, AudioClipRecorder, downsample_waveform, encode_wav_pcm16, finish_audio_clip,
    parse_adts_frames,
};
pub use mp4::{mux_h264_aac_mp4, mux_h264_mp4};
pub use video::VideoClipRecorder;

/// Maximum clip length (5 minutes, matching Slack's clip duration cap `F-COLLAB-107`).
pub const MAX_CLIP_DURATION_MS: u64 = 5 * 60 * 1_000;
/// Maximum audio clip length: 5 minutes at 48 kHz mono.
pub(super) const MAX_AUDIO_SAMPLES: usize = (SAMPLE_RATE as usize) * 60 * 5;
/// Maximum video clip frames (5 minutes at 12 fps, `F-COLLAB-107`).
pub(super) const MAX_VIDEO_FRAMES: usize = 12 * 300;
pub(super) const LIVE_BARS: usize = 24;
pub(super) const CLIP_FPS: u32 = 12;

/// Output of a finished audio or video clip recording, ready to attach and upload.
#[derive(Debug, Clone)]
pub struct RecordedClip {
    pub filename: String,
    pub mime: String,
    pub bytes: Vec<u8>,
    pub duration_ms: u64,
    pub waveform: Vec<f32>,
    pub preview_frame: Option<VideoFrame>,
    /// Up to 4 candidate thumbnail keyframes sampled across the video clip (`F-COLLAB-099`).
    pub thumbnail_frames: Vec<VideoFrame>,
}

/// Source for [`VideoClipRecorder`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipVideoSource {
    Camera(Option<usize>),
    Screen(String),
}

pub(super) fn timestamp_slug() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
