//! Audio pipeline: `cpal` capture/playback + `sonora` (AEC3 / NS / AGC2) + `opus` 48kHz, with
//! remote audio received through WebRTC NetEq (`neteq`).

mod chimes;
mod devices;
mod dsp;
mod engine;
mod playout;
pub mod ring;
mod streams;
#[cfg(test)]
mod tests;

/// Chime / Opus native sample rate (48 kHz).
pub const SAMPLE_RATE: u32 = 48_000;
/// 10ms frame size for `sonora` audio processing (480 samples at 48 kHz).
pub const FRAME_10MS: usize = 480;
/// 20ms frame size for Opus encoding/decoding (960 samples at 48 kHz).
pub const FRAME_20MS: usize = 960;

pub(crate) use chimes::AmbientMusicPlayer;
pub use chimes::{HuddleChime, play_huddle_chime};
pub(crate) use devices::cpal_device_name;
pub use devices::{
    AudioDeviceInfo, AudioDevices, MicLevelProbe, list_audio_devices, play_speaker_test_tone,
};
pub(crate) use dsp::RateConverter;
pub use dsp::{LoudnessNormalizer, compute_rms_mono, normalized_gain_for_rms};
pub(crate) use engine::{AudioEngine, AudioSharedState};
pub use playout::AudioRxSnapshot;
pub(crate) use playout::PlayoutRing;
