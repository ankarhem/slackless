//! Pure-Rust audio pipeline: `cpal` capture/playback + `sonora` (AEC3 / NS / AGC2) + `opus` 48kHz.

mod chimes;
mod devices;
mod dsp;
mod engine;
mod jitter;
mod streams;
#[cfg(test)]
mod tests;

/// Chime / Opus native sample rate (48 kHz).
pub const SAMPLE_RATE: u32 = 48_000;
/// 10ms frame size for `sonora` audio processing (480 samples at 48 kHz).
pub const FRAME_10MS: usize = 480;
/// 20ms frame size for Opus encoding/decoding (960 samples at 48 kHz).
pub const FRAME_20MS: usize = 960;

pub use chimes::{AmbientMusicPlayer, HuddleChime, play_huddle_chime, slack_lounge_music_filename};
pub(crate) use devices::cpal_device_name;
pub use devices::{
    AudioDeviceInfo, AudioDevices, MicLevelProbe, list_audio_devices, play_speaker_test_tone,
};
pub use dsp::{LoudnessNormalizer, compute_rms_mono, normalized_gain_for_rms, resample_linear};
pub use engine::{AudioEngine, AudioSharedState};
pub(crate) use jitter::{AudioJitterBuffer, JitterStep, PlaybackBuffer, decode_opus_or_plc};
