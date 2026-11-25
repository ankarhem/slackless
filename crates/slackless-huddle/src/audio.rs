//! Pure-Rust audio pipeline: `cpal` capture/playback + `sonora` (AEC3 / NS / AGC2) + `opus` 48kHz.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use cpal::{SampleFormat, Stream, StreamConfig};
use opus::{Application, Bitrate, Channels, Decoder, Encoder};
use sonora::config::{Config as SonoraConfig, EchoCanceller, GainController2, NoiseSuppression};
use sonora::{AudioProcessing, StreamConfig as SonoraStreamConfig};
use tokio::sync::mpsc;

use crate::backend::{AudioDeviceChangeKind, HwAudioDuplexHandle, platform_backend};
use crate::rtc::InboundAudioPacket;
use crate::{HuddleAudioConfig, HuddleError, Result};

/// Chime / Opus native sample rate (48 kHz).
pub const SAMPLE_RATE: u32 = 48_000;
/// 10ms frame size for `sonora` audio processing (480 samples at 48 kHz).
pub const FRAME_10MS: usize = 480;
/// 20ms frame size for Opus encoding/decoding (960 samples at 48 kHz).
pub const FRAME_20MS: usize = 960;

/// One enumerated audio device (mic or speaker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub is_default: bool,
}

/// Available input and output audio devices on the current host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioDevices {
    pub inputs: Vec<AudioDeviceInfo>,
    pub outputs: Vec<AudioDeviceInfo>,
}

/// Human-readable device name via `cpal 0.18` `DeviceTrait::description`.
pub(crate) fn cpal_device_name(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|d| d.name().to_owned())
}

/// Enumerate all host microphones and speakers.
pub fn list_audio_devices() -> AudioDevices {
    let host = cpal::default_host();
    let default_in = host
        .default_input_device()
        .and_then(|d| cpal_device_name(&d));
    let default_out = host
        .default_output_device()
        .and_then(|d| cpal_device_name(&d));

    let inputs = host
        .input_devices()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|d| cpal_device_name(&d))
        .map(|name| AudioDeviceInfo {
            is_default: default_in.as_deref() == Some(name.as_str()),
            name,
        })
        .collect();

    let outputs = host
        .output_devices()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|d| cpal_device_name(&d))
        .map(|name| AudioDeviceInfo {
            is_default: default_out.as_deref() == Some(name.as_str()),
            name,
        })
        .collect();

    AudioDevices { inputs, outputs }
}

const SLACK_CDN_AUDIO_BASE: &str = "https://a.slack-edge.com/bv1-13";

/// Official Slack huddle lifecycle sound effects (`https://a.slack-edge.com/bv1-13/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HuddleChime {
    /// You joined / connected to a huddle (`you_joined_call_v2-7029418.mp3`).
    Join,
    /// You left / disconnected from a huddle (`you_left_call_v2-1bc1290.mp3`).
    Leave,
    /// Another participant joined the huddle (`they_joined_call_v2-570b169.mp3`).
    PeerJoin,
    /// Another participant left the huddle (`they_left_call_v2-95d3260.mp3`).
    PeerLeave,
    /// Outgoing / incoming DM huddle invite ring (`outgoing_ring_v2-6b3697e.mp3`).
    InviteRing,
}

impl HuddleChime {
    /// Official Slack CDN filename for this huddle lifecycle sound.
    pub const fn slack_asset_filename(self) -> &'static str {
        match self {
            Self::Join => "you_joined_call_v2-7029418.mp3",
            Self::Leave => "you_left_call_v2-1bc1290.mp3",
            Self::PeerJoin => "they_joined_call_v2-570b169.mp3",
            Self::PeerLeave => "they_left_call_v2-95d3260.mp3",
            Self::InviteRing => "outgoing_ring_v2-6b3697e.mp3",
        }
    }

    /// Full official Slack CDN URL for this huddle lifecycle sound.
    pub fn slack_cdn_url(self) -> String {
        format!("{SLACK_CDN_AUDIO_BASE}/{}", self.slack_asset_filename())
    }
}

/// Official Slack huddle lounge / idle music track filename on `a.slack-edge.com/bv1-13/`.
/// Defaults to Official Slack's iconic Smooth Jazz (`waiting_muzak-9187b04.mp3`).
pub fn slack_lounge_music_filename(song_key: Option<&str>) -> &'static str {
    match song_key.unwrap_or("jazz") {
        "waiting" => "waiting_room-d5f7069.mp3",
        "subway" => "subway_music-ff03dfb.mp3",
        "whistle" => "whistle_tune-894c024.mp3",
        "focus" => "focus_beats-ee1e4a9.mp3",
        "lofi" => "huddles_focus_lofihaus_music_mixdown_stereo-07cf114.mp3",
        "disco" => "disco_party-6f29a34.mp3",
        "zen" => "moment_of_zen-7047c77.mp3",
        "nature" => "nature_meditation-495e2c5.mp3",
        "piano" => "piano_concerto-d00d84e.mp3",
        _ => "waiting_muzak-9187b04.mp3",
    }
}

fn slack_sound_cache_dir() -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::PathBuf::from(home)
                .join("Library")
                .join("Caches")
                .join("Slackless")
                .join("sounds");
        }
    }
    std::env::temp_dir().join("slackless-sounds")
}

/// Ensure official Slack MP3 `filename` is cached on disk, downloading it once from Slack's
/// public CDN (`a.slack-edge.com/bv1-13/`). Returns `None` if offline or download fails.
fn ensure_cached_slack_audio(filename: &str) -> Option<std::path::PathBuf> {
    let dir = slack_sound_cache_dir();
    let dest = dir.join(filename);
    if std::fs::metadata(&dest).is_ok_and(|m| m.len() > 256) {
        return Some(dest);
    }
    let _ = std::fs::create_dir_all(&dir);
    let url = format!("{SLACK_CDN_AUDIO_BASE}/{filename}");
    let tmp = dir.join(format!(".{filename}.{}.tmp", std::process::id()));
    let ok = std::process::Command::new("/usr/bin/curl")
        .args(["-fsSL", "--max-time", "8", "-o"])
        .arg(&tmp)
        .arg(&url)
        .status()
        .is_ok_and(|s| s.success());
    if ok && std::fs::metadata(&tmp).is_ok_and(|m| m.len() > 256) {
        let _ = std::fs::rename(&tmp, &dest);
        if std::fs::metadata(&dest).is_ok_and(|m| m.len() > 256) {
            return Some(dest);
        }
    }
    let _ = std::fs::remove_file(&tmp);
    None
}

/// Real-time generalized audio loudness normalizer (voice-activity-gated RMS leveler + soft-knee
/// peak limiter).
///
/// Keeps both remote speakers' voices and local microphone output at a consistent, comfortable
/// conversational loudness (`-18 dBFS` target RMS) without boosting background silence or clipping
/// loud transients.
#[derive(Debug, Clone)]
pub struct LoudnessNormalizer {
    target_rms: f32,
    gate_rms: f32,
    min_gain: f32,
    max_gain: f32,
    attack: f32,
    release: f32,
    current_gain: f32,
}

impl LoudnessNormalizer {
    /// Speech-tuned leveler for 20ms VoIP frames (`-18 dBFS` target, `-44 dBFS` silence gate,
    /// fast ~80ms attack against shouting, gentle ~800ms release to lift quiet mics cleanly).
    pub fn speech() -> Self {
        Self {
            target_rms: 0.125, // ~ -18 dBFS
            gate_rms: 0.006,   // ~ -44 dBFS (ignores room noise / pauses)
            min_gain: 0.25,    // -12 dB max attenuation
            max_gain: 4.0,     // +12 dB max boost
            attack: 0.24,
            release: 0.03,
            current_gain: 1.0,
        }
    }

    /// Smoothly normalize a 20ms mono PCM frame in-place with soft-knee peak limiting.
    pub fn process_frame(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        let rms = (sum_sq / samples.len() as f32).sqrt();

        let prev_gain = self.current_gain;
        if rms > self.gate_rms {
            let desired = (self.target_rms / rms).clamp(self.min_gain, self.max_gain);
            let alpha = if desired < self.current_gain {
                self.attack
            } else {
                self.release
            };
            self.current_gain += (desired - self.current_gain) * alpha;
        } else {
            // Gently drift back toward unity during extended silence so the next utterance starts
            // cleanly without an aggressive residual boost.
            self.current_gain += (1.0 - self.current_gain) * 0.005;
        }

        let next_gain = self.current_gain;
        let denom = samples.len().max(1) as f32;
        for (i, s) in samples.iter_mut().enumerate() {
            // Interpolate gain across the 20ms frame to prevent zipper noise.
            let g = prev_gain + (next_gain - prev_gain) * (i as f32 / denom);
            *s = soft_limit_sample(*s * g);
        }
    }

    #[cfg(test)]
    fn gain(&self) -> f32 {
        self.current_gain
    }
}

#[inline]
fn soft_limit_sample(x: f32) -> f32 {
    const KNEE: f32 = 0.80;
    const HEADROOM: f32 = 0.18;
    let abs_x = x.abs();
    if abs_x <= KNEE {
        x
    } else {
        let excess = (abs_x - KNEE) / HEADROOM;
        let limited = KNEE + HEADROOM * excess.tanh();
        x.signum() * limited.min(0.98)
    }
}

/// Compute a bounded playback volume scaling factor to match `target_rms` given a file's measured
/// active RMS loudness.
pub fn normalized_gain_for_rms(rms: f32, target_rms: f32, min_gain: f32, max_gain: f32) -> f32 {
    if !rms.is_finite() || rms <= 1e-5 {
        return target_rms.clamp(min_gain, max_gain);
    }
    (target_rms / rms).clamp(min_gain, max_gain)
}

#[cfg(target_os = "macos")]
fn normalized_file_volume(
    path: &std::path::Path,
    target_rms: f32,
    min_gain: f32,
    max_gain: f32,
    fallback: f32,
) -> f32 {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    static FILE_RMS_CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<f32>>>> = OnceLock::new();
    let cache = FILE_RMS_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let cached = cache.lock().ok().and_then(|g| g.get(path).copied());
    let measured = match cached {
        Some(v) => v,
        None => {
            let rms = crate::macos_media::measure_audio_file_rms(path);
            if let Ok(mut g) = cache.lock() {
                g.insert(path.to_path_buf(), rms);
            }
            rms
        }
    };
    match measured {
        Some(rms) => {
            let vol = normalized_gain_for_rms(rms, target_rms, min_gain, max_gain);
            let gain_db = 20.0 * vol.log10();
            tracing::info!(
                file = %path.file_name().and_then(|n| n.to_str()).unwrap_or("audio"),
                measured_rms_db = format!("{:.1}", 20.0 * rms.log10()),
                gain_db = format!("{gain_db:+.1}"),
                linear_gain = format!("{vol:.2}x"),
                "normalized huddle audio file loudness"
            );
            vol
        }
        None => fallback,
    }
}

/// Play an official Slack huddle lifecycle sound (`you_joined_call_v2`, `you_left_call_v2`,
/// `they_joined_call_v2`, `they_left_call_v2`, `outgoing_ring_v2`) without blocking the caller.
/// Uses the cached official MP3 via native macOS `NSSound` routed to `output_device`, with loudness
/// normalization and fallback to `afplay` or a synthesized chime.
pub fn play_huddle_chime(chime: HuddleChime, output_device: Option<&str>) -> Result<()> {
    let device_name = output_device.map(ToOwned::to_owned);
    std::thread::Builder::new()
        .name("slackless-huddle-chime".to_owned())
        .spawn(move || {
            #[cfg(target_os = "macos")]
            {
                if let Some(path) = ensure_cached_slack_audio(chime.slack_asset_filename()) {
                    // Normalize chimes to ~ -22 dBFS weighted RMS (0.080 linear), supporting real
                    // boost up to +20 dB (10.0x) via AVAudioEngine + AVAudioUnitEQ so quiet MP3s
                    // match louder ones cleanly.
                    let volume = normalized_file_volume(&path, 0.080, 0.15, 10.0, 1.0);
                    if crate::macos_media::play_routed_audio_oneshot(
                        &path,
                        device_name.as_deref(),
                        volume,
                    ) {
                        return;
                    }
                    let vol_arg = format!("{volume:.3}");
                    if std::process::Command::new("/usr/bin/afplay")
                        .args(["-v", &vol_arg])
                        .arg(&path)
                        .status()
                        .is_ok_and(|s| s.success())
                    {
                        return;
                    }
                }
            }
            play_synthesized_huddle_chime(chime, device_name.as_deref());
        })
        .map_err(|e| HuddleError::Audio(e.to_string()))?;
    Ok(())
}

fn play_synthesized_huddle_chime(chime: HuddleChime, output_device: Option<&str>) {
    let Ok(device) = select_output_device(output_device) else {
        return;
    };
    let Ok(supported) = device.default_output_config() else {
        return;
    };
    let rate = supported.sample_rate() as f32;
    let channels = supported.channels() as usize;
    let duration_secs: f32 = match chime {
        HuddleChime::Join | HuddleChime::PeerJoin => 0.34,
        HuddleChime::Leave | HuddleChime::PeerLeave => 0.28,
        HuddleChime::InviteRing => 0.44,
    };
    let total_frames = (rate * duration_secs) as usize;
    let frame_ix = Arc::new(AtomicU32::new(0));
    let ix_cb = Arc::clone(&frame_ix);

    let config: StreamConfig = supported.into();
    let Ok(stream) = device.build_output_stream(
        config,
        move |data: &mut [f32], _| {
            let mut ix = ix_cb.load(Ordering::Relaxed) as usize;
            for frame in data.chunks_mut(channels) {
                let sample = if ix < total_frames {
                    let t = ix as f32 / rate;
                    let (freq, gain) = match chime {
                        HuddleChime::Join | HuddleChime::PeerJoin => {
                            let f = if t < 0.11 {
                                523.25 // C5
                            } else if t < 0.22 {
                                659.25 // E5
                            } else {
                                783.99 // G5
                            };
                            (f, 0.18)
                        }
                        HuddleChime::Leave | HuddleChime::PeerLeave => {
                            let f = if t < 0.13 { 587.33 } else { 440.0 }; // D5 -> A4
                            (f, 0.16)
                        }
                        HuddleChime::InviteRing => {
                            let pulse = (t < 0.18) || (t > 0.24 && t < 0.42);
                            let f = if t < 0.18 { 783.99 } else { 987.77 }; // G5 -> B5
                            (f, if pulse { 0.20 } else { 0.0 })
                        }
                    };
                    let env = (1.0 - (t / duration_secs)).clamp(0.0, 1.0) * gain;
                    (2.0 * std::f32::consts::PI * freq * t).sin() * env
                } else {
                    0.0
                };
                for ch in frame.iter_mut() {
                    *ch = sample;
                }
                ix += 1;
            }
            ix_cb.store(ix as u32, Ordering::Relaxed);
        },
        |_| {},
        None,
    ) else {
        return;
    };
    if stream.play().is_ok() {
        std::thread::sleep(Duration::from_millis((duration_secs * 1000.0) as u64 + 80));
    }
    drop(stream);
}

/// Play a short two-note harmonic chime on `output_device` (or the default speaker) for `F-COLLAB-040`.
pub fn play_speaker_test_tone(output_device: Option<&str>) -> Result<()> {
    let device_name = output_device.map(ToOwned::to_owned);
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);

    std::thread::Builder::new()
        .name("slackless-speaker-test".to_owned())
        .spawn(move || {
            let start = (|| -> Result<Stream> {
                let device = select_output_device(device_name.as_deref())?;
                let supported = device
                    .default_output_config()
                    .map_err(|e| HuddleError::Audio(e.to_string()))?;
                let rate = supported.sample_rate() as f32;
                let channels = supported.channels() as usize;
                let total_frames = (rate * 0.45) as usize;
                let frame_ix = Arc::new(AtomicU32::new(0));
                let ix_cb = Arc::clone(&frame_ix);

                let config: StreamConfig = supported.into();
                let stream = device
                    .build_output_stream(
                        config,
                        move |data: &mut [f32], _| {
                            let mut ix = ix_cb.load(Ordering::Relaxed) as usize;
                            for frame in data.chunks_mut(channels) {
                                let sample = if ix < total_frames {
                                    let t = ix as f32 / rate;
                                    let freq = if t < 0.22 { 523.25 } else { 659.25 }; // C5 -> E5
                                    let env = (1.0 - (t / 0.45)).clamp(0.0, 1.0) * 0.22;
                                    (2.0 * std::f32::consts::PI * freq * t).sin() * env
                                } else {
                                    0.0
                                };
                                for ch in frame.iter_mut() {
                                    *ch = sample;
                                }
                                ix += 1;
                            }
                            ix_cb.store(ix as u32, Ordering::Relaxed);
                        },
                        |_| {},
                        None,
                    )
                    .map_err(|e| HuddleError::Audio(e.to_string()))?;

                stream
                    .play()
                    .map_err(|e| HuddleError::Audio(e.to_string()))?;
                Ok(stream)
            })();

            match start {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    std::thread::sleep(Duration::from_millis(520));
                    drop(stream);
                }
                Err(err) => {
                    let _ = ready_tx.send(Err(err));
                }
            }
        })
        .map_err(|e| HuddleError::Audio(e.to_string()))?;

    ready_rx
        .recv()
        .map_err(|e| HuddleError::Audio(e.to_string()))?
}

/// Lightweight live microphone level probe for Preferences -> Audio & video (`F-COLLAB-048`).
pub struct MicLevelProbe {
    running: Arc<AtomicBool>,
    rms_bits: Arc<AtomicU32>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl MicLevelProbe {
    pub fn start(input_device: Option<&str>) -> Result<Self> {
        let device_name = input_device.map(ToOwned::to_owned);
        let running = Arc::new(AtomicBool::new(true));
        let rms_bits = Arc::new(AtomicU32::new(0));
        let run_flag = Arc::clone(&running);
        let rms_cb = Arc::clone(&rms_bits);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);

        let worker = std::thread::Builder::new()
            .name("slackless-mic-probe".to_owned())
            .spawn(move || {
                let start = (|| -> Result<Stream> {
                    let device = select_input_device(device_name.as_deref())?;
                    let supported = device
                        .default_input_config()
                        .map_err(|e| HuddleError::Audio(e.to_string()))?;
                    let channels = supported.channels() as usize;
                    let config: StreamConfig = supported.into();

                    let stream = device
                        .build_input_stream(
                            config,
                            move |data: &[f32], _| {
                                let rms = compute_rms_interleaved(data, channels);
                                rms_cb.store(rms.to_bits(), Ordering::Relaxed);
                            },
                            |_| {},
                            None,
                        )
                        .map_err(|e| HuddleError::Audio(e.to_string()))?;
                    stream
                        .play()
                        .map_err(|e| HuddleError::Audio(e.to_string()))?;
                    Ok(stream)
                })();

                match start {
                    Ok(stream) => {
                        let _ = ready_tx.send(Ok(()));
                        while run_flag.load(Ordering::Relaxed) {
                            std::thread::sleep(Duration::from_millis(25));
                        }
                        drop(stream);
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                    }
                }
            })
            .map_err(|e| HuddleError::Audio(e.to_string()))?;

        ready_rx
            .recv()
            .map_err(|e| HuddleError::Audio(e.to_string()))??;

        Ok(Self {
            running,
            rms_bits,
            worker: Some(worker),
        })
    }

    /// Current microphone RMS level normalized to `0.0..=1.0`.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.rms_bits.load(Ordering::Relaxed))
    }
}

impl Drop for MicLevelProbe {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

/// Shared runtime controls for a live [`AudioEngine`].
#[derive(Clone)]
pub struct AudioSharedState {
    pub muted: Arc<AtomicBool>,
    pub aec: Arc<AtomicBool>,
    pub ns: Arc<AtomicBool>,
    pub agc: Arc<AtomicBool>,
    pub local_rms_bits: Arc<AtomicU32>,
    pub running: Arc<AtomicBool>,
}

impl AudioSharedState {
    pub fn from_config(cfg: &HuddleAudioConfig) -> Self {
        Self {
            muted: Arc::new(AtomicBool::new(cfg.muted)),
            aec: Arc::new(AtomicBool::new(cfg.aec)),
            ns: Arc::new(AtomicBool::new(cfg.ns)),
            agc: Arc::new(AtomicBool::new(cfg.agc)),
            local_rms_bits: Arc::new(AtomicU32::new(0)),
            running: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn local_rms(&self) -> f32 {
        if self.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            f32::from_bits(self.local_rms_bits.load(Ordering::Relaxed))
        }
    }
}

type CaptureSignal = Arc<(Mutex<VecDeque<f32>>, Condvar)>;

/// Live full-duplex audio engine (capture → sonora AEC3/NS/AGC → Opus TX; Opus RX → sonora ref → speaker).
///
/// Both `cpal::Stream`s live on the dedicated audio worker OS thread so `AudioEngine` stays `Send`
/// on macOS CoreAudio (where `cpal::Stream` is `!Send`).
/// Target playback jitter-buffer pre-fill before releasing audio after startup or underrun
/// (60 ms = 3 × 20 ms Opus frames). Eliminates 512-sample CoreAudio vs 960-sample Opus beat
/// underruns and absorbs normal internet UDP jitter cleanly.
const PLAYBACK_PRIME_SAMPLES: usize = FRAME_20MS * 3;
/// High-water mark above which excess queued playback audio is trimmed down cleanly (240 ms).
const PLAYBACK_MAX_SAMPLES: usize = FRAME_20MS * 12;
/// Target sample depth after an overflow trim (80 ms).
const PLAYBACK_TRIM_TARGET: usize = FRAME_20MS * 4;

/// Smooth, glitch-free playback ring buffer shared between the dedicated inbound Opus decoder
/// thread (`push`) and the real-time CoreAudio / `cpal` render callback (`pull_into`).
pub(crate) struct PlaybackBuffer {
    samples: VecDeque<f32>,
    primed: bool,
    last_sample: f32,
}

impl PlaybackBuffer {
    pub(crate) fn new() -> Self {
        Self {
            samples: VecDeque::with_capacity(PLAYBACK_MAX_SAMPLES),
            primed: false,
            last_sample: 0.0,
        }
    }

    pub(crate) fn push(&mut self, pcm: &[f32]) {
        if self.samples.len() + pcm.len() > PLAYBACK_MAX_SAMPLES {
            let keep = PLAYBACK_TRIM_TARGET.min(self.samples.len());
            let drop_n = self.samples.len().saturating_sub(keep);
            if drop_n > 0 {
                let prev_end = self.samples.front().copied().unwrap_or(self.last_sample);
                self.samples.drain(..drop_n);
                // Short 32-sample crossfade at the splice boundary avoids an audible pop.
                let fade = 32.min(self.samples.len());
                for i in 0..fade {
                    let t = (i + 1) as f32 / (fade + 1) as f32;
                    if let Some(s) = self.samples.get_mut(i) {
                        *s = prev_end * (1.0 - t) + *s * t;
                    }
                }
            }
        }
        self.samples.extend(pcm.iter().copied());
        if !self.primed && self.samples.len() >= PLAYBACK_PRIME_SAMPLES {
            self.primed = true;
        }
    }

    pub(crate) fn pull_into(&mut self, out: &mut [f32]) {
        if !self.primed {
            self.fade_to_silence(out);
            return;
        }
        let avail = self.samples.len().min(out.len());
        for dst in out.iter_mut().take(avail) {
            let s = self.samples.pop_front().unwrap_or(0.0).clamp(-1.0, 1.0);
            *dst = s;
            self.last_sample = s;
        }
        if avail < out.len() {
            // Genuine network underrun: ramp smoothly to zero and re-prime before resuming so
            // playback never sputters 1 frame at a time.
            self.primed = false;
            self.fade_to_silence(&mut out[avail..]);
        }
    }

    fn fade_to_silence(&mut self, out: &mut [f32]) {
        let mut s = self.last_sample;
        for dst in out.iter_mut() {
            s *= 0.85;
            if s.abs() < 1e-5 {
                s = 0.0;
            }
            *dst = s;
        }
        self.last_sample = s;
    }
}

/// Signed modular difference `a - b` in 16-bit RTP sequence space (positive means `a` is newer).
#[inline]
fn seq_diff(a: u16, b: u16) -> i32 {
    a.wrapping_sub(b) as i16 as i32
}

/// Sequence-ordered Opus RTP jitter buffer supporting out-of-order UDP delivery, Opus in-band FEC
/// recovery for single lost packets, and Opus PLC fallback for multi-packet loss.
pub(crate) struct AudioJitterBuffer {
    queue: BTreeMap<i64, Bytes>,
    /// Extended 64-bit sequence number of the next expected packet.
    next_seq: Option<i64>,
    /// Highest extended 64-bit sequence number observed so far (for wraparound unwrapping).
    base_seq: Option<i64>,
    missing_since: Option<Instant>,
    consecutive_plc: u8,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum JitterStep {
    /// Decode this normal Opus packet.
    Packet(Bytes),
    /// Missing packet whose immediate successor is already buffered: decode successor with
    /// Opus in-band FEC (`decode_fec = true`) to recover the lost frame cleanly.
    FecFromNext(Bytes),
    /// Missing packet with no immediate successor available: invoke Opus PLC.
    Plc,
}

impl AudioJitterBuffer {
    pub(crate) fn new() -> Self {
        Self {
            queue: BTreeMap::new(),
            next_seq: None,
            base_seq: None,
            missing_since: None,
            consecutive_plc: 0,
        }
    }

    fn unwrap_seq(&mut self, seq16: u16) -> i64 {
        match self.base_seq {
            None => {
                let s = seq16 as i64;
                self.base_seq = Some(s);
                s
            }
            Some(base) => {
                let diff = seq_diff(seq16, (base & 0xFFFF) as u16) as i64;
                let ext = base + diff;
                if ext > base {
                    self.base_seq = Some(ext);
                }
                ext
            }
        }
    }

    pub(crate) fn push(&mut self, seq16: u16, payload: Bytes, now: Instant) {
        let ext = self.unwrap_seq(seq16);
        let next = match self.next_seq {
            Some(n) => n,
            None => {
                self.next_seq = Some(ext);
                ext
            }
        };
        // Ignore stale packets already played out.
        if ext < next {
            return;
        }
        // Large jump (e.g. speaker resumed after DTX silence or SSRC switch): resync cleanly.
        if ext - next > 20 {
            self.queue.clear();
            self.next_seq = Some(ext);
            self.missing_since = None;
            self.consecutive_plc = 0;
        }
        if ext > next && self.missing_since.is_none() {
            self.missing_since = Some(now);
        }
        self.queue.insert(ext, payload);
        // Cap queue depth to prevent unbounded growth during stalls.
        while self.queue.len() > 24 {
            if let Some((&oldest, _)) = self.queue.iter().next() {
                self.queue.remove(&oldest);
                if self.next_seq.is_some_and(|n| oldest >= n) {
                    self.next_seq = Some(oldest + 1);
                }
            }
        }
    }

    /// Pop the next ready packet/FEC/PLC step if available at `now`.
    pub(crate) fn pop_ready(&mut self, now: Instant) -> Option<JitterStep> {
        let next = self.next_seq?;
        if let Some(pkt) = self.queue.remove(&next) {
            self.next_seq = Some(next + 1);
            self.missing_since = if self.queue.is_empty() || self.queue.contains_key(&(next + 1)) {
                None
            } else {
                Some(now)
            };
            self.consecutive_plc = 0;
            return Some(JitterStep::Packet(pkt));
        }

        // Next expected sequence number is missing — wait up to 35ms (or 2 newer packets) so
        // slightly reordered UDP packets can arrive before triggering FEC/PLC.
        let (&first_avail, _) = self.queue.iter().next()?;
        let wait_elapsed = self
            .missing_since
            .is_some_and(|t| now.saturating_duration_since(t) >= Duration::from_millis(35));
        let enough_newer = self.queue.len() >= 2 && first_avail - next <= 4;
        if !wait_elapsed && !enough_newer {
            return None;
        }

        // If gap is large (> 4 frames), jump directly to the next available packet (likely DTX).
        if first_avail - next > 4 || self.consecutive_plc >= 4 {
            self.next_seq = Some(first_avail);
            self.missing_since = None;
            self.consecutive_plc = 0;
            return self.pop_ready(now);
        }

        self.next_seq = Some(next + 1);
        self.consecutive_plc = self.consecutive_plc.saturating_add(1);
        if let Some(next_pkt) = self.queue.get(&(next + 1)).cloned() {
            Some(JitterStep::FecFromNext(next_pkt))
        } else {
            Some(JitterStep::Plc)
        }
    }
}

pub struct AudioEngine {
    shared: AudioSharedState,
    capture_signal: CaptureSignal,
    rx_wake: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    rx_worker: Option<std::thread::JoinHandle<mpsc::UnboundedReceiver<InboundAudioPacket>>>,
}

enum AudioIoBackend {
    /// OS-native full-duplex 48 kHz hardware-tuned AEC/NS/AGC (e.g. macOS CoreAudio
    /// `VoiceProcessingIO`) — skips software `sonora` completely!
    Hardware(Box<dyn HwAudioDuplexHandle>),
    /// Cross-platform portable fallback (`cpal` + pure-Rust `sonora` AEC3/NS/AGC2).
    Cpal {
        in_stream: Stream,
        out_stream: Stream,
        apm: Box<AudioProcessing>,
    },
}

fn open_hw_duplex(
    in_name: Option<&str>,
    out_name: Option<&str>,
    aec: bool,
    ns: bool,
    agc: bool,
    capture_signal: &CaptureSignal,
    playback_buf: &Arc<Mutex<PlaybackBuffer>>,
) -> Option<Box<dyn HwAudioDuplexHandle>> {
    let cap_sig = Arc::clone(capture_signal);
    let play_buf = Arc::clone(playback_buf);
    let on_capture = Arc::new(move |samples: &[f32]| {
        let (lock, cvar) = &*cap_sig;
        if let Ok(mut q) = lock.lock() {
            if q.len() > FRAME_20MS * 8 {
                let drop_n = q.len() - FRAME_20MS * 4;
                q.drain(..drop_n);
            }
            q.extend(samples.iter().copied());
            if q.len() >= FRAME_20MS {
                cvar.notify_one();
            }
        }
    });
    let on_render = Arc::new(move |out: &mut [f32]| {
        // Real-time CoreAudio callback: use `try_lock` so we never priority-invert on the
        // decoder thread; lock hold time on `push` is a few hundred nanoseconds.
        if let Ok(mut pb) = play_buf.try_lock() {
            pb.pull_into(out);
        } else {
            out.fill(0.0);
        }
    });
    match platform_backend().start_hw_audio_duplex(
        SAMPLE_RATE,
        in_name,
        out_name,
        aec || ns,
        agc,
        on_capture,
        on_render,
    )? {
        Ok(hw) => Some(hw),
        Err(err) => {
            tracing::warn!(%err, "hardware audio DSP unavailable; falling back to cpal + sonora");
            None
        }
    }
}

impl AudioEngine {
    pub fn start(
        cfg: &HuddleAudioConfig,
        shared: AudioSharedState,
        outbound_opus_tx: mpsc::UnboundedSender<Bytes>,
        mut inbound_opus_rx: mpsc::UnboundedReceiver<InboundAudioPacket>,
    ) -> Result<Self> {
        shared.running.store(true, Ordering::Relaxed);
        let capture_signal: CaptureSignal = Arc::new((
            Mutex::new(VecDeque::with_capacity(FRAME_20MS * 8)),
            Condvar::new(),
        ));
        let playback_buf: Arc<Mutex<PlaybackBuffer>> = Arc::new(Mutex::new(PlaybackBuffer::new()));
        // Optional render tap used only when software `cpal + sonora` fallback is active
        // (macOS CoreAudio `VoiceProcessingIO` taps its own render bus in hardware).
        let aec_render_tap: Arc<Mutex<VecDeque<f32>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(FRAME_20MS * 6)));
        let needs_sw_aec_ref = Arc::new(AtomicBool::new(false));
        let rx_wake = Arc::new(AtomicBool::new(true));

        let in_name = cfg.input_device.clone();
        let out_name = cfg.output_device.clone();
        let init_muted = shared.muted.load(Ordering::Relaxed);
        let init_aec = cfg.aec;
        let init_ns = cfg.ns;
        let init_agc = cfg.agc;
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);

        let worker_shared = shared.clone();
        let worker_capture = Arc::clone(&capture_signal);
        let worker_playback = Arc::clone(&playback_buf);
        let worker_aec_tap = Arc::clone(&aec_render_tap);
        let worker_needs_aec = Arc::clone(&needs_sw_aec_ref);
        let worker = std::thread::Builder::new()
            .name("slackless-huddle-audio".to_owned())
            .spawn(move || {
                let io_backend: Result<AudioIoBackend> = if let Some(hw) = open_hw_duplex(
                    in_name.as_deref(),
                    out_name.as_deref(),
                    !init_muted && init_aec,
                    !init_muted && init_ns,
                    !init_muted && init_agc,
                    &worker_capture,
                    &worker_playback,
                ) {
                    worker_needs_aec.store(false, Ordering::Release);
                    Ok(AudioIoBackend::Hardware(hw))
                } else {
                    (|| -> Result<AudioIoBackend> {
                        let in_stream =
                            open_input_stream(in_name.as_deref(), Arc::clone(&worker_capture))?;
                        let out_stream =
                            open_output_stream(out_name.as_deref(), Arc::clone(&worker_playback))?;
                        worker_needs_aec.store(true, Ordering::Release);
                        Ok(AudioIoBackend::Cpal {
                            in_stream,
                            out_stream,
                            apm: Box::new(build_sonora(init_aec, init_ns, init_agc)),
                        })
                    })()
                };

                match io_backend {
                    Ok(io) => {
                        let _ = ready_tx.send(Ok(()));
                        run_audio_dsp_loop(
                            worker_shared,
                            worker_capture,
                            worker_playback,
                            worker_aec_tap,
                            outbound_opus_tx,
                            in_name,
                            out_name,
                            io,
                        );
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                    }
                }
            })
            .map_err(|e| HuddleError::Audio(e.to_string()))?;

        if let Err(err) = ready_rx
            .recv()
            .map_err(|e| HuddleError::Audio(e.to_string()))
            .and_then(|r| r)
        {
            shared.running.store(false, Ordering::Relaxed);
            capture_signal.1.notify_all();
            let _ = worker.join();
            return Err(err);
        }

        // Dedicated inbound Opus jitter-buffer + decoder thread: completely decoupled from the
        // microphone capture loop so remote speech is decoded within microseconds of UDP arrival!
        let rx_shared = shared.clone();
        let rx_playback = Arc::clone(&playback_buf);
        let rx_aec_tap = Arc::clone(&aec_render_tap);
        let rx_needs_aec = Arc::clone(&needs_sw_aec_ref);
        let rx_worker = std::thread::Builder::new()
            .name("slackless-huddle-audio-rx".to_owned())
            .spawn(move || {
                run_inbound_audio_rx_loop(
                    rx_shared,
                    &mut inbound_opus_rx,
                    rx_playback,
                    rx_aec_tap,
                    rx_needs_aec,
                );
                inbound_opus_rx
            })
            .map_err(|e| HuddleError::Audio(e.to_string()))?;

        Ok(Self {
            shared,
            capture_signal,
            rx_wake,
            worker: Some(worker),
            rx_worker: Some(rx_worker),
        })
    }

    /// Stop streams & DSP threads, returning the inbound Opus receiver so a hot-swapped device
    /// restart keeps receiving remote audio seamlessly.
    pub fn stop(&mut self) -> Option<mpsc::UnboundedReceiver<InboundAudioPacket>> {
        self.shared.running.store(false, Ordering::Relaxed);
        self.rx_wake.store(false, Ordering::Relaxed);
        self.capture_signal.1.notify_all();
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
        self.rx_worker.take().and_then(|handle| handle.join().ok())
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn build_sonora(aec: bool, ns: bool, agc: bool) -> AudioProcessing {
    let mut cfg = SonoraConfig::default();
    if aec {
        cfg.echo_canceller = Some(EchoCanceller::default());
    }
    if ns {
        cfg.noise_suppression = Some(NoiseSuppression::default());
    }
    if agc {
        cfg.gain_controller2 = Some(GainController2::default());
    }
    AudioProcessing::builder()
        .config(cfg)
        .capture_config(SonoraStreamConfig::new(SAMPLE_RATE, 1))
        .render_config(SonoraStreamConfig::new(SAMPLE_RATE, 1))
        .build()
}

/// Decode a received Opus packet, or invoke Opus Packet Loss Concealment (PLC) when `pkt` is
/// empty (a sentinel emitted by the RTP reader when 1..=4 consecutive frames were dropped).
pub(crate) fn decode_opus_or_plc(
    decoder: &mut Decoder,
    pkt: &[u8],
    out: &mut [f32],
) -> std::result::Result<usize, opus::Error> {
    if pkt.is_empty() {
        let plc_len = FRAME_20MS.min(out.len());
        decoder.decode_float(&[], &mut out[..plc_len], false)
    } else {
        decoder.decode_float(pkt, out, false)
    }
}

fn current_default_input_name() -> Option<String> {
    cpal::default_host()
        .default_input_device()
        .and_then(|d| cpal_device_name(&d))
}

fn current_default_output_name() -> Option<String> {
    cpal::default_host()
        .default_output_device()
        .and_then(|d| cpal_device_name(&d))
}

fn run_inbound_audio_rx_loop(
    shared: AudioSharedState,
    inbound_opus_rx: &mut mpsc::UnboundedReceiver<InboundAudioPacket>,
    playback_buf: Arc<Mutex<PlaybackBuffer>>,
    aec_render_tap: Arc<Mutex<VecDeque<f32>>>,
    needs_sw_aec_ref: Arc<AtomicBool>,
) {
    let Ok(mut decoder) = Decoder::new(SAMPLE_RATE, Channels::Mono) else {
        return;
    };
    let mut jitter = AudioJitterBuffer::new();
    let mut normalizer = LoudnessNormalizer::speech();
    let mut decoded_pcm = vec![0.0f32; FRAME_20MS * 2];

    while shared.running.load(Ordering::Relaxed) {
        let mut received_any = false;
        while let Ok(pkt) = inbound_opus_rx.try_recv() {
            jitter.push(pkt.seq, pkt.payload, Instant::now());
            received_any = true;
        }

        let now = Instant::now();
        let mut decoded_any = false;
        while let Some(step) = jitter.pop_ready(now) {
            let decoded_res = match step {
                JitterStep::Packet(pkt) => decoder.decode_float(&pkt, &mut decoded_pcm, false),
                JitterStep::FecFromNext(next_pkt) => {
                    decoder.decode_float(&next_pkt, &mut decoded_pcm[..FRAME_20MS], true)
                }
                JitterStep::Plc => decode_opus_or_plc(&mut decoder, &[], &mut decoded_pcm),
            };
            if let Ok(samples) = decoded_res {
                let pcm = &mut decoded_pcm[..samples];
                // Generalized per-stream speech loudness normalization: lifts quiet mics and tames
                // hot mics to a consistent conversational level without boosting silence.
                normalizer.process_frame(pcm);
                if let Ok(mut pb) = playback_buf.lock() {
                    pb.push(pcm);
                }
                if needs_sw_aec_ref.load(Ordering::Relaxed)
                    && !shared.muted.load(Ordering::Relaxed)
                    && shared.aec.load(Ordering::Relaxed)
                    && let Ok(mut tap) = aec_render_tap.lock()
                {
                    if tap.len() > FRAME_20MS * 6 {
                        let drop_n = tap.len() - FRAME_20MS * 3;
                        tap.drain(..drop_n);
                    }
                    tap.extend(pcm.iter().copied());
                }
                decoded_any = true;
            }
        }

        if !received_any && !decoded_any {
            // Short 5ms poll sleep when idle: wakes immediately enough for 20ms RTP cadence
            // while still checking `shared.running` cleanly on shutdown/device-swap.
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_audio_dsp_loop(
    shared: AudioSharedState,
    capture_signal: CaptureSignal,
    playback_buf: Arc<Mutex<PlaybackBuffer>>,
    aec_render_tap: Arc<Mutex<VecDeque<f32>>>,
    outbound_opus_tx: mpsc::UnboundedSender<Bytes>,
    configured_in: Option<String>,
    configured_out: Option<String>,
    mut io: AudioIoBackend,
) {
    let Ok(mut encoder) = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip) else {
        return;
    };
    // Match Chime SDP `useinbandfec=1` with crisp 32 kbps VBR voice + DTX for low idle uplink.
    let _ = encoder.set_bitrate(Bitrate::Bits(32_000));
    let _ = encoder.set_vbr(true);
    let _ = encoder.set_inband_fec(true);
    let _ = encoder.set_packet_loss_perc(5);
    let _ = encoder.set_dtx(true);

    let mut cur_aec = shared.aec.load(Ordering::Relaxed);
    let mut cur_ns = shared.ns.load(Ordering::Relaxed);
    let mut cur_agc = shared.agc.load(Ordering::Relaxed);
    let mut cur_muted = shared.muted.load(Ordering::Relaxed);

    let mut opus_out = vec![0u8; 4000];
    // Chunked arena for outbound Opus packets: amortizes heap allocation to ~1 alloc/sec instead
    // of 50 Vec<u8> allocs/sec, and `cached_silence_opus.clone()` is a zero-alloc refcount bump.
    let mut opus_arena = BytesMut::with_capacity(8192);
    let silence_pcm = vec![0.0f32; FRAME_20MS];
    let cached_silence_opus: Bytes = encoder
        .encode_float(&silence_pcm, &mut opus_out)
        .map(|n| Bytes::copy_from_slice(&opus_out[..n]))
        .unwrap_or_else(|_| Bytes::from_static(&[0xF8, 0xFF, 0xFE]));
    let mut mic_frame = Vec::with_capacity(FRAME_20MS);
    let mut processed_capture = Vec::with_capacity(FRAME_20MS);
    let mut mic_normalizer = LoudnessNormalizer::speech();
    let mut aec_ref_chunk = [0.0f32; FRAME_10MS];

    // Event-driven OS audio device hot-plug monitor (CoreAudio listener on macOS; falls back to
    // 1.5s polling only on platforms without a native listener).
    let in_dirty = Arc::new(AtomicBool::new(false));
    let out_dirty = Arc::new(AtomicBool::new(false));
    let device_monitor = {
        let in_d = Arc::clone(&in_dirty);
        let out_d = Arc::clone(&out_dirty);
        let wake = Arc::clone(&capture_signal);
        platform_backend().start_audio_device_monitor(Arc::new(move |kind| {
            match kind {
                AudioDeviceChangeKind::Input => in_d.store(true, Ordering::Release),
                AudioDeviceChangeKind::Output => out_d.store(true, Ordering::Release),
            }
            wake.1.notify_one();
        }))
    };
    let use_polling_hotplug = device_monitor.is_none();

    let mut last_default_in = if configured_in.is_none() {
        current_default_input_name()
    } else {
        None
    };
    let mut last_default_out = if configured_out.is_none() {
        current_default_output_name()
    } else {
        None
    };
    let mut last_device_check = std::time::Instant::now();

    while shared.running.load(Ordering::Relaxed) {
        let check_in = in_dirty.swap(false, Ordering::AcqRel);
        let check_out = out_dirty.swap(false, Ordering::AcqRel);
        let poll_due =
            use_polling_hotplug && last_device_check.elapsed() >= Duration::from_millis(1500);
        if check_in || check_out || poll_due {
            if poll_due {
                last_device_check = std::time::Instant::now();
            }
            match &mut io {
                AudioIoBackend::Hardware(_) => {
                    // Restart hardware VoiceProcessingIO cleanly when the system default input or
                    // output changes (e.g. AirPods connect/disconnect).
                    let now_in = current_default_input_name();
                    let now_out = current_default_output_name();
                    if now_in != last_default_in || now_out != last_default_out {
                        last_default_in = now_in;
                        last_default_out = now_out;
                        if let Some(next_hw) = open_hw_duplex(
                            None,
                            None,
                            !cur_muted && cur_aec,
                            !cur_muted && cur_ns,
                            !cur_muted && cur_agc,
                            &capture_signal,
                            &playback_buf,
                        ) {
                            io = AudioIoBackend::Hardware(next_hw);
                        }
                    }
                }
                AudioIoBackend::Cpal {
                    in_stream,
                    out_stream,
                    ..
                } => {
                    if configured_in.is_none() && (check_in || poll_due) {
                        let now_in = current_default_input_name();
                        if now_in.is_some()
                            && now_in != last_default_in
                            && let Ok(next_stream) =
                                open_input_stream(None, Arc::clone(&capture_signal))
                        {
                            *in_stream = next_stream;
                            last_default_in = now_in;
                        }
                    }
                    if configured_out.is_none() && (check_out || poll_due) {
                        let now_out = current_default_output_name();
                        if now_out.is_some()
                            && now_out != last_default_out
                            && let Ok(next_stream) =
                                open_output_stream(None, Arc::clone(&playback_buf))
                        {
                            *out_stream = next_stream;
                            last_default_out = now_out;
                        }
                    }
                }
            }
        }

        // Reconfigure hardware DSP or Sonora live if AEC/NS/AGC or mute toggles changed.
        // Bypassing VoiceProcessingIO's AEC/NS/AGC while muted cuts idle muted audio CPU
        // significantly while keeping remote speaker playback untouched.
        let next_aec = shared.aec.load(Ordering::Relaxed);
        let next_ns = shared.ns.load(Ordering::Relaxed);
        let next_agc = shared.agc.load(Ordering::Relaxed);
        let next_muted = shared.muted.load(Ordering::Relaxed);
        let dsp_changed = (next_aec, next_ns, next_agc) != (cur_aec, cur_ns, cur_agc);
        let mute_changed = next_muted != cur_muted;
        if dsp_changed || mute_changed {
            (cur_aec, cur_ns, cur_agc, cur_muted) = (next_aec, next_ns, next_agc, next_muted);
            match &mut io {
                AudioIoBackend::Hardware(hw) => {
                    hw.set_processing(!cur_muted && (cur_aec || cur_ns), !cur_muted && cur_agc);
                }
                AudioIoBackend::Cpal { apm, .. } if dsp_changed => {
                    **apm = build_sonora(cur_aec, cur_ns, cur_agc);
                }
                AudioIoBackend::Cpal { .. } => {}
            }
        }

        // 1. Feed Sonora's AEC render reference only when running the software `cpal` fallback.
        // (Hardware `VoiceProcessingIO` taps its own render bus internally, and inbound Opus
        // decoding runs on the dedicated `slackless-huddle-audio-rx` thread without mic-loop lag.)
        if let AudioIoBackend::Cpal { apm, .. } = &mut io {
            if !cur_muted && cur_aec {
                if let Ok(mut tap) = aec_render_tap.lock() {
                    while tap.len() >= FRAME_10MS {
                        for dst in &mut aec_ref_chunk {
                            *dst = tap.pop_front().unwrap_or(0.0);
                        }
                        let mut out_ref = [0.0f32; FRAME_10MS];
                        let _ =
                            apm.process_render_f32(&[&aec_ref_chunk[..]], &mut [&mut out_ref[..]]);
                    }
                }
            } else if let Ok(mut tap) = aec_render_tap.lock() {
                tap.clear();
            }
        }

        // 2. Drain captured 20ms mic chunks -> (HW already processed or Sonora) -> Opus -> RTC TX.
        mic_frame.clear();
        {
            let (lock, cvar) = &*capture_signal;
            if let Ok(mut ring) = lock.lock() {
                if ring.len() < FRAME_20MS && shared.running.load(Ordering::Relaxed) {
                    if let Ok((guard, _)) = cvar.wait_timeout(ring, Duration::from_millis(18)) {
                        ring = guard;
                    } else {
                        continue;
                    }
                }
                if ring.len() >= FRAME_20MS {
                    mic_frame.extend(ring.drain(..FRAME_20MS));
                }
            }
        }

        if mic_frame.len() == FRAME_20MS {
            let muted = shared.muted.load(Ordering::Relaxed);
            if muted {
                shared
                    .local_rms_bits
                    .store(0.0f32.to_bits(), Ordering::Relaxed);
                let _ = outbound_opus_tx.send(cached_silence_opus.clone());
            } else {
                let pcm_to_encode: &mut [f32] = match &mut io {
                    AudioIoBackend::Hardware(_) => &mut mic_frame,
                    AudioIoBackend::Cpal { apm, .. } => {
                        processed_capture.clear();
                        for chunk in mic_frame.as_chunks::<FRAME_10MS>().0 {
                            let mut out_chunk = [0.0f32; FRAME_10MS];
                            if apm
                                .process_capture_f32(&[chunk.as_slice()], &mut [&mut out_chunk[..]])
                                .is_ok()
                            {
                                processed_capture.extend_from_slice(&out_chunk);
                            } else {
                                processed_capture.extend_from_slice(chunk);
                            }
                        }
                        &mut processed_capture
                    }
                };
                // Normalize local microphone volume when AGC is enabled so quiet Bluetooth/USB mics
                // and hot desk mics both transmit at consistent conversational loudness.
                if cur_agc {
                    mic_normalizer.process_frame(pcm_to_encode);
                }
                let rms = compute_rms_mono(pcm_to_encode);
                shared
                    .local_rms_bits
                    .store(rms.to_bits(), Ordering::Relaxed);
                if let Ok(n) = encoder.encode_float(pcm_to_encode, &mut opus_out) {
                    if opus_arena.capacity() < n {
                        opus_arena = BytesMut::with_capacity(8192.max(n));
                    }
                    opus_arena.extend_from_slice(&opus_out[..n]);
                    let _ = outbound_opus_tx.send(opus_arena.split().freeze());
                }
            }
        }
    }
    drop(device_monitor);
    drop(io);
}

fn select_input_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if let Some(target) = name.filter(|s| !s.is_empty())
        && let Ok(mut devices) = host.input_devices()
        && let Some(found) = devices.find(|d| cpal_device_name(d).as_deref() == Some(target))
    {
        return Ok(found);
    }
    host.default_input_device()
        .ok_or_else(|| HuddleError::Audio("no input microphone device found".to_owned()))
}

fn select_output_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if let Some(target) = name.filter(|s| !s.is_empty())
        && let Ok(mut devices) = host.output_devices()
        && let Some(found) = devices.find(|d| cpal_device_name(d).as_deref() == Some(target))
    {
        return Ok(found);
    }
    host.default_output_device()
        .ok_or_else(|| HuddleError::Audio("no output speaker device found".to_owned()))
}

fn open_input_stream(name: Option<&str>, signal: CaptureSignal) -> Result<Stream> {
    let device = select_input_device(name)?;
    let supported = device
        .default_input_config()
        .map_err(|e| HuddleError::Audio(e.to_string()))?;
    let src_rate = supported.sample_rate();
    let channels = supported.channels() as usize;
    let fmt = supported.sample_format();
    let config: StreamConfig = supported.into();

    let mut mono_scratch = Vec::with_capacity(FRAME_20MS * 2);
    let mut resampled_scratch = Vec::with_capacity(FRAME_20MS * 2);

    let stream = match fmt {
        SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _| {
                downmix_to_mono_f32_into(data, channels, &mut mono_scratch);
                let samples: &[f32] = if src_rate == SAMPLE_RATE {
                    &mono_scratch
                } else {
                    resample_linear_into(
                        &mono_scratch,
                        src_rate,
                        SAMPLE_RATE,
                        &mut resampled_scratch,
                    );
                    &resampled_scratch
                };
                let (lock, cvar) = &*signal;
                if let Ok(mut q) = lock.lock() {
                    if q.len() > FRAME_20MS * 8 {
                        let drop_n = q.len() - FRAME_20MS * 4;
                        q.drain(..drop_n);
                    }
                    q.extend(samples.iter().copied());
                    if q.len() >= FRAME_20MS {
                        cvar.notify_one();
                    }
                }
            },
            |_| {},
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            config,
            move |data: &[i16], _| {
                mono_scratch.clear();
                let ch_div = channels.max(1) as f32;
                for ch in data.chunks(channels.max(1)) {
                    let sum: f32 = ch.iter().map(|&s| s as f32 / 32768.0).sum();
                    mono_scratch.push(sum / ch_div);
                }
                let samples: &[f32] = if src_rate == SAMPLE_RATE {
                    &mono_scratch
                } else {
                    resample_linear_into(
                        &mono_scratch,
                        src_rate,
                        SAMPLE_RATE,
                        &mut resampled_scratch,
                    );
                    &resampled_scratch
                };
                let (lock, cvar) = &*signal;
                if let Ok(mut q) = lock.lock() {
                    if q.len() > FRAME_20MS * 8 {
                        let drop_n = q.len() - FRAME_20MS * 4;
                        q.drain(..drop_n);
                    }
                    q.extend(samples.iter().copied());
                    if q.len() >= FRAME_20MS {
                        cvar.notify_one();
                    }
                }
            },
            |_| {},
            None,
        ),
        other => {
            return Err(HuddleError::Audio(format!(
                "unsupported input sample format {other:?}"
            )));
        }
    }
    .map_err(|e| HuddleError::Audio(e.to_string()))?;

    stream
        .play()
        .map_err(|e| HuddleError::Audio(e.to_string()))?;
    Ok(stream)
}

fn open_output_stream(
    name: Option<&str>,
    playback_buf: Arc<Mutex<PlaybackBuffer>>,
) -> Result<Stream> {
    let device = select_output_device(name)?;
    let supported = device
        .default_output_config()
        .map_err(|e| HuddleError::Audio(e.to_string()))?;
    let dst_rate = supported.sample_rate();
    let channels = supported.channels() as usize;
    let config: StreamConfig = supported.into();

    let mut src_scratch = Vec::with_capacity(FRAME_20MS * 2);
    let mut resampled_scratch = Vec::with_capacity(FRAME_20MS * 2);

    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [f32], _| {
                let out_frames = data.len() / channels.max(1);
                let needed_48k = ((out_frames as u64 * SAMPLE_RATE as u64)
                    .div_ceil(dst_rate.max(1) as u64)) as usize;
                src_scratch.resize(needed_48k, 0.0);
                if let Ok(mut pb) = playback_buf.try_lock() {
                    pb.pull_into(&mut src_scratch[..needed_48k]);
                } else {
                    src_scratch[..needed_48k].fill(0.0);
                }
                let samples: &[f32] = if dst_rate == SAMPLE_RATE {
                    &src_scratch
                } else {
                    resample_linear_into(
                        &src_scratch,
                        SAMPLE_RATE,
                        dst_rate,
                        &mut resampled_scratch,
                    );
                    &resampled_scratch
                };
                for (i, frame) in data.chunks_mut(channels).enumerate() {
                    let s = samples.get(i).copied().unwrap_or(0.0).clamp(-1.0, 1.0);
                    for ch in frame.iter_mut() {
                        *ch = s;
                    }
                }
            },
            |_| {},
            None,
        )
        .map_err(|e| HuddleError::Audio(e.to_string()))?;

    stream
        .play()
        .map_err(|e| HuddleError::Audio(e.to_string()))?;
    Ok(stream)
}

fn downmix_to_mono_f32_into(data: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    if channels <= 1 {
        out.extend_from_slice(data);
        return;
    }
    let div = channels as f32;
    out.reserve(data.len() / channels);
    for ch in data.chunks(channels) {
        out.push(ch.iter().copied().sum::<f32>() / div);
    }
}

fn downmix_to_mono_f32(data: &[f32], channels: usize) -> Vec<f32> {
    let mut out = Vec::new();
    downmix_to_mono_f32_into(data, channels, &mut out);
    out
}

fn resample_linear_into(input: &[f32], src_rate: u32, dst_rate: u32, out: &mut Vec<f32>) {
    out.clear();
    if src_rate == dst_rate || input.is_empty() {
        out.extend_from_slice(input);
        return;
    }
    let ratio = src_rate as f64 / dst_rate as f64;
    let out_len = ((input.len() as f64) / ratio).round().max(1.0) as usize;
    out.reserve(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos.floor() as usize;
        let frac = (pos - idx as f64) as f32;
        let s0 = input.get(idx).copied().unwrap_or(0.0);
        let s1 = input.get(idx + 1).copied().unwrap_or(s0);
        out.push(s0 + (s1 - s0) * frac);
    }
}

/// Linear resampler used when a macOS device runs at 44.1 kHz instead of 48 kHz.
pub fn resample_linear(input: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    let mut out = Vec::new();
    resample_linear_into(input, src_rate, dst_rate, &mut out);
    out
}

pub fn compute_rms_mono(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt().clamp(0.0, 1.0)
}

fn compute_rms_interleaved(samples: &[f32], channels: usize) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mono = downmix_to_mono_f32(samples, channels);
    compute_rms_mono(&mono)
}

/// Official Slack huddle lounge / idle music player (`F-COLLAB-036`, `F-COLLAB-058`).
///
/// Plays Official Slack's cached lounge tracks (`waiting_muzak` Smooth Jazz by default, plus
/// `waiting`, `subway`, `whistle`, `focus`, `lofi`, `disco`, `zen`, `nature`, `piano`) via native
/// macOS `afplay` with gentle background volume and instant teardown on `stop()`/`Drop`. Falls
/// back to a soft synthesized lo-fi chord progression only when offline or on non-macOS hosts.
pub struct AmbientMusicPlayer {
    running: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl AmbientMusicPlayer {
    pub fn start(output_device: Option<String>, song_key: Option<&str>) -> Result<Self> {
        let running = Arc::new(AtomicBool::new(true));
        let run_flag = Arc::clone(&running);
        let filename = slack_lounge_music_filename(song_key).to_owned();
        let worker = std::thread::Builder::new()
            .name("slackless-huddle-music".to_owned())
            .spawn(move || {
                #[cfg(target_os = "macos")]
                {
                    if let Some(path) = ensure_cached_slack_audio(&filename) {
                        // Normalize every lounge track to an identical gentle background loudness
                        // (-30 dBFS 200Hz-weighted RMS = 0.032 linear). Slack's CDN MP3s span a
                        // 30 dB range (-46 dBFS piano_concerto to -16 dBFS lofihaus), so allow
                        // true linear gain from 0.08x (-22 dB) up to 14.0x (+23 dB) via
                        // AVAudioEngine + AVAudioUnitEQ!
                        let volume = normalized_file_volume(&path, 0.032, 0.08, 14.0, 1.0);
                        // Prefer CoreAudio device-routed looping NSSound (routes Smooth Jazz
                        // directly to the user's chosen huddle headset/speaker with 0 subprocesses).
                        if let Some(mut routed) = crate::macos_media::MacRoutedAudioLoop::start(
                            &path,
                            output_device.as_deref(),
                            volume,
                        ) {
                            while run_flag.load(Ordering::Relaxed) {
                                std::thread::sleep(Duration::from_millis(80));
                            }
                            routed.stop();
                            return;
                        }
                        let vol_arg = format!("{volume:.3}");
                        while run_flag.load(Ordering::Relaxed) {
                            let Ok(mut child) = std::process::Command::new("/usr/bin/afplay")
                                .args(["-v", &vol_arg])
                                .arg(&path)
                                .spawn()
                            else {
                                break;
                            };
                            while run_flag.load(Ordering::Relaxed) {
                                match child.try_wait() {
                                    Ok(Some(_)) => break,
                                    Ok(None) => std::thread::sleep(Duration::from_millis(80)),
                                    Err(_) => break,
                                }
                            }
                            if !run_flag.load(Ordering::Relaxed) {
                                let _ = child.kill();
                                let _ = child.wait();
                                return;
                            }
                        }
                        if !run_flag.load(Ordering::Relaxed) {
                            return;
                        }
                    }
                }
                let _ = &filename;
                // Offline / non-macOS fallback: warm lo-fi 4-chord progression.
                let Ok(device) = select_output_device(output_device.as_deref()) else {
                    return;
                };
                let Ok(supported) = device.default_output_config() else {
                    return;
                };
                let rate = supported.sample_rate() as f32;
                let channels = supported.channels() as usize;
                let config: StreamConfig = supported.into();
                const CHORDS: [[f32; 4]; 4] = [
                    [261.63, 329.63, 392.00, 493.88],
                    [220.00, 261.63, 329.63, 392.00],
                    [174.61, 220.00, 261.63, 329.63],
                    [196.00, 246.94, 293.66, 329.63],
                ];
                let mut sample_idx: u64 = 0;
                let chord_samples = (rate * 3.5) as u64;
                let stream = device.build_output_stream(
                    config,
                    move |out: &mut [f32], _| {
                        for frame in out.chunks_mut(channels) {
                            let chord_ix = ((sample_idx / chord_samples) % 4) as usize;
                            let pos_in_chord =
                                (sample_idx % chord_samples) as f32 / chord_samples as f32;
                            let env = (pos_in_chord * 8.0).min(1.0)
                                * ((1.0 - pos_in_chord) * 8.0).min(1.0);
                            let t = sample_idx as f32 / rate;
                            let mut s = 0.0f32;
                            for &hz in &CHORDS[chord_ix] {
                                s += (2.0 * std::f32::consts::PI * hz * t).sin();
                            }
                            let sample = (s * 0.018 * env).clamp(-0.12, 0.12);
                            for slot in frame {
                                *slot = sample;
                            }
                            sample_idx = sample_idx.wrapping_add(1);
                        }
                    },
                    |_| {},
                    None,
                );
                let Ok(stream) = stream else {
                    return;
                };
                if stream.play().is_err() {
                    return;
                }
                while run_flag.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(80));
                }
            })
            .map_err(|e| HuddleError::Audio(e.to_string()))?;
        Ok(Self {
            running,
            worker: Some(worker),
        })
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for AmbientMusicPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loudness_normalizer_lifts_quiet_tames_loud_and_ignores_silence() {
        let mut quiet_norm = LoudnessNormalizer::speech();
        let mut quiet_frame = vec![0.0f32; FRAME_20MS];
        // Feed 1.5s of quiet speech (~0.025 peak -> ~0.0177 RMS).
        for f in 0..75 {
            for (i, s) in quiet_frame.iter_mut().enumerate() {
                let t = (f * FRAME_20MS + i) as f32 / SAMPLE_RATE as f32;
                *s = (2.0 * std::f32::consts::PI * 300.0 * t).sin() * 0.025;
            }
            quiet_norm.process_frame(&mut quiet_frame);
        }
        assert!(
            quiet_norm.gain() > 2.2,
            "expected quiet voice to be boosted, got gain={}",
            quiet_norm.gain()
        );

        let mut loud_norm = LoudnessNormalizer::speech();
        let mut loud_frame = vec![0.0f32; FRAME_20MS];
        // Feed hot mic (~0.85 peak -> ~0.60 RMS): should tame quickly with zero hard clipping.
        for f in 0..25 {
            for (i, s) in loud_frame.iter_mut().enumerate() {
                let t = (f * FRAME_20MS + i) as f32 / SAMPLE_RATE as f32;
                *s = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.85;
            }
            loud_norm.process_frame(&mut loud_frame);
            assert!(
                loud_frame.iter().all(|s| s.abs() <= 0.985),
                "soft limiter must prevent clipping"
            );
        }
        assert!(
            loud_norm.gain() < 0.45,
            "expected hot voice to be attenuated, got gain={}",
            loud_norm.gain()
        );

        // Background noise below gate (-50 dBFS) must NOT be boosted.
        let mut noise_norm = LoudnessNormalizer::speech();
        let mut noise = vec![0.002f32; FRAME_20MS];
        for _ in 0..50 {
            noise.fill(0.002);
            noise_norm.process_frame(&mut noise);
        }
        assert!(
            (noise_norm.gain() - 1.0).abs() < 0.05,
            "noise below gate must remain near unity, got {}",
            noise_norm.gain()
        );

        // File RMS normalization scales louder tracks down and quieter tracks up cleanly.
        let loud_vol = normalized_gain_for_rms(0.30, 0.063, 0.06, 0.72);
        let quiet_vol = normalized_gain_for_rms(0.10, 0.063, 0.06, 0.72);
        assert!((loud_vol - 0.21).abs() < 0.01);
        assert!((quiet_vol - 0.63).abs() < 0.01);
    }

    #[test]
    fn synthetic_sonora_and_opus_round_trip_preserves_signal() {
        let mut apm = build_sonora(true, true, true);
        let mut enc = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip).unwrap();
        let mut dec = Decoder::new(SAMPLE_RATE, Channels::Mono).unwrap();

        let mut sine = Vec::with_capacity(FRAME_20MS);
        for i in 0..FRAME_20MS {
            let t = i as f32 / SAMPLE_RATE as f32;
            sine.push((2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.25);
        }
        let mut processed = Vec::with_capacity(FRAME_20MS);
        for chunk in sine.as_chunks::<FRAME_10MS>().0 {
            let mut out = [0.0f32; FRAME_10MS];
            apm.process_capture_f32(&[chunk.as_slice()], &mut [&mut out[..]])
                .unwrap();
            processed.extend_from_slice(&out);
        }
        let mut encoded = vec![0u8; 4000];
        let n = enc.encode_float(&processed, &mut encoded).unwrap();
        assert!(n > 0);
        let mut decoded = vec![0.0f32; FRAME_20MS];
        let samples = dec
            .decode_float(&encoded[..n], &mut decoded, false)
            .unwrap();
        assert_eq!(samples, FRAME_20MS);
        assert!(compute_rms_mono(&decoded) > 0.01);
    }

    #[test]
    fn opus_plc_conceals_lost_packet_without_silence_click() {
        let mut enc = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip).unwrap();
        let mut dec = Decoder::new(SAMPLE_RATE, Channels::Mono).unwrap();

        let sine: Vec<f32> = (0..FRAME_20MS)
            .map(|i| {
                (2.0 * std::f32::consts::PI * 330.0 * (i as f32 / SAMPLE_RATE as f32)).sin() * 0.3
            })
            .collect();
        let mut encoded = vec![0u8; 4000];
        let mut decoded = vec![0.0f32; FRAME_20MS * 2];

        // Prime the decoder with two normal frames so PLC has history to extrapolate.
        for _ in 0..2 {
            let n = enc.encode_float(&sine, &mut encoded).unwrap();
            let got = decode_opus_or_plc(&mut dec, &encoded[..n], &mut decoded).unwrap();
            assert_eq!(got, FRAME_20MS);
        }

        // Empty slice invokes Opus PLC for a missing 20ms RTP frame.
        let plc_samples = decode_opus_or_plc(&mut dec, &[], &mut decoded).unwrap();
        assert_eq!(plc_samples, FRAME_20MS);
        assert!(compute_rms_mono(&decoded[..plc_samples]) > 0.005);
    }

    #[test]
    fn linear_resampler_scales_length_accurately() {
        let input = vec![0.5f32; 441];
        let out = resample_linear(&input, 44_100, 48_000);
        assert_eq!(out.len(), 480);
        assert!((out[240] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn playback_buffer_prebuffers_and_fades_smoothly_on_underrun() {
        let mut pb = PlaybackBuffer::new();
        let mut out = vec![1.0f32; 512];

        // Below 60ms prime threshold: outputs silence so 512-sample CoreAudio pulls never sputter.
        pb.push(&vec![0.4f32; FRAME_20MS]);
        pb.pull_into(&mut out);
        assert!(out.iter().all(|&s| s == 0.0));

        // Pushing 2 more 20ms frames reaches 60ms -> primed and plays cleanly!
        pb.push(&vec![0.4f32; FRAME_20MS]);
        pb.push(&vec![0.4f32; FRAME_20MS]);
        pb.pull_into(&mut out);
        assert!(out.iter().all(|&s| (s - 0.4).abs() < 1e-5));

        // Draining past available samples fades smoothly toward zero (no hard step click) and unprimes.
        let mut big = vec![0.0f32; FRAME_20MS * 4];
        pb.pull_into(&mut big);
        let tail_start = PLAYBACK_PRIME_SAMPLES - 512;
        assert!(big[tail_start] > 0.2 && big[tail_start] < 0.4);
        assert!(big[tail_start + 64].abs() < 1e-4);
        assert!(!pb.primed);
    }

    #[test]
    fn jitter_buffer_reorders_out_of_order_packets_and_recovers_loss_with_fec() {
        let mut jb = AudioJitterBuffer::new();
        let t0 = Instant::now();

        // Out-of-order UDP delivery: 100, 102, 101 arrives 5ms later -> played strictly 100, 101, 102.
        jb.push(100, Bytes::from_static(b"p100"), t0);
        assert_eq!(
            jb.pop_ready(t0),
            Some(JitterStep::Packet(Bytes::from_static(b"p100")))
        );

        jb.push(
            102,
            Bytes::from_static(b"p102"),
            t0 + Duration::from_millis(20),
        );
        // Only 1 newer packet and <35ms waited -> holds briefly for reordered 101.
        assert_eq!(jb.pop_ready(t0 + Duration::from_millis(25)), None);

        jb.push(
            101,
            Bytes::from_static(b"p101"),
            t0 + Duration::from_millis(26),
        );
        assert_eq!(
            jb.pop_ready(t0 + Duration::from_millis(26)),
            Some(JitterStep::Packet(Bytes::from_static(b"p101")))
        );
        assert_eq!(
            jb.pop_ready(t0 + Duration::from_millis(26)),
            Some(JitterStep::Packet(Bytes::from_static(b"p102")))
        );

        // Genuine single-packet loss: 103 never arrives, 104 arrives -> after 35ms triggers Opus FEC from 104!
        jb.push(
            104,
            Bytes::from_static(b"p104"),
            t0 + Duration::from_millis(80),
        );
        assert_eq!(
            jb.pop_ready(t0 + Duration::from_millis(120)),
            Some(JitterStep::FecFromNext(Bytes::from_static(b"p104")))
        );
        assert_eq!(
            jb.pop_ready(t0 + Duration::from_millis(120)),
            Some(JitterStep::Packet(Bytes::from_static(b"p104")))
        );
    }

    #[test]
    fn official_slack_huddle_sounds_and_lounge_music_resolve_to_slack_cdn() {
        assert_eq!(
            HuddleChime::Join.slack_cdn_url(),
            "https://a.slack-edge.com/bv1-13/you_joined_call_v2-7029418.mp3"
        );
        assert_eq!(
            HuddleChime::Leave.slack_cdn_url(),
            "https://a.slack-edge.com/bv1-13/you_left_call_v2-1bc1290.mp3"
        );
        assert_eq!(
            HuddleChime::PeerJoin.slack_cdn_url(),
            "https://a.slack-edge.com/bv1-13/they_joined_call_v2-570b169.mp3"
        );
        assert_eq!(
            HuddleChime::PeerLeave.slack_cdn_url(),
            "https://a.slack-edge.com/bv1-13/they_left_call_v2-95d3260.mp3"
        );
        assert_eq!(
            HuddleChime::InviteRing.slack_cdn_url(),
            "https://a.slack-edge.com/bv1-13/outgoing_ring_v2-6b3697e.mp3"
        );
        assert_eq!(
            slack_lounge_music_filename(None),
            "waiting_muzak-9187b04.mp3"
        );
        assert_eq!(
            slack_lounge_music_filename(Some("jazz")),
            "waiting_muzak-9187b04.mp3"
        );
        assert_eq!(
            slack_lounge_music_filename(Some("lofi")),
            "huddles_focus_lofihaus_music_mixdown_stereo-07cf114.mp3"
        );
    }

    #[test]
    fn jitter_buffer_wraparound_stale_multi_loss_fast_trigger_dtx_resync_overflow_and_rearm() {
        let t0 = Instant::now();

        // 1. 16-bit seq wraparound (65534..=1) + out-of-order across wrap.
        let mut jb = AudioJitterBuffer::new();
        jb.push(65534, Bytes::from_static(b"a"), t0);
        jb.push(0, Bytes::from_static(b"c"), t0 + Duration::from_millis(5));
        jb.push(
            65535,
            Bytes::from_static(b"b"),
            t0 + Duration::from_millis(8),
        );
        jb.push(1, Bytes::from_static(b"d"), t0 + Duration::from_millis(12));
        for expected in [b"a", b"b", b"c", b"d"] {
            assert_eq!(
                jb.pop_ready(t0 + Duration::from_millis(12)),
                Some(JitterStep::Packet(Bytes::from_static(expected)))
            );
        }

        // 2. Stale packets (already played) are ignored; duplicate of queued seq doesn't corrupt.
        jb.push(
            65535,
            Bytes::from_static(b"stale"),
            t0 + Duration::from_millis(20),
        );
        assert_eq!(jb.pop_ready(t0 + Duration::from_millis(20)), None);
        jb.push(2, Bytes::from_static(b"v1"), t0 + Duration::from_millis(20));
        jb.push(2, Bytes::from_static(b"v2"), t0 + Duration::from_millis(21));
        assert_eq!(
            jb.pop_ready(t0 + Duration::from_millis(21)),
            Some(JitterStep::Packet(Bytes::from_static(b"v2")))
        );

        // 3. Multi-packet loss (3 & 4 lost, 5 arrives): after 35ms yields Plc then FecFromNext(5) then Packet(5).
        jb.push(5, Bytes::from_static(b"p5"), t0 + Duration::from_millis(40));
        assert_eq!(jb.pop_ready(t0 + Duration::from_millis(60)), None);
        assert_eq!(
            jb.pop_ready(t0 + Duration::from_millis(80)),
            Some(JitterStep::Plc)
        );
        assert_eq!(
            jb.pop_ready(t0 + Duration::from_millis(80)),
            Some(JitterStep::FecFromNext(Bytes::from_static(b"p5")))
        );
        assert_eq!(
            jb.pop_ready(t0 + Duration::from_millis(80)),
            Some(JitterStep::Packet(Bytes::from_static(b"p5")))
        );

        // 4. `enough_newer` fast-trigger: 2 newer packets within gap <= 4 trigger immediately without 35ms wait.
        jb.push(
            7,
            Bytes::from_static(b"p7"),
            t0 + Duration::from_millis(100),
        );
        jb.push(
            8,
            Bytes::from_static(b"p8"),
            t0 + Duration::from_millis(101),
        );
        assert_eq!(
            jb.pop_ready(t0 + Duration::from_millis(101)),
            Some(JitterStep::FecFromNext(Bytes::from_static(b"p7")))
        );
        assert_eq!(
            jb.pop_ready(t0 + Duration::from_millis(101)),
            Some(JitterStep::Packet(Bytes::from_static(b"p7")))
        );
        assert_eq!(
            jb.pop_ready(t0 + Duration::from_millis(101)),
            Some(JitterStep::Packet(Bytes::from_static(b"p8")))
        );

        // 5. DTX jump (gap > 4 and <= 20): waits 35ms then jumps straight to first available packet.
        jb.push(
            16,
            Bytes::from_static(b"p16"),
            t0 + Duration::from_millis(200),
        );
        assert_eq!(jb.pop_ready(t0 + Duration::from_millis(220)), None);
        assert_eq!(
            jb.pop_ready(t0 + Duration::from_millis(240)),
            Some(JitterStep::Packet(Bytes::from_static(b"p16")))
        );

        // 6. Large jump resync (> 20): clears queue and plays immediately.
        jb.push(
            18,
            Bytes::from_static(b"old"),
            t0 + Duration::from_millis(300),
        );
        jb.push(
            50,
            Bytes::from_static(b"resync"),
            t0 + Duration::from_millis(305),
        );
        assert_eq!(
            jb.pop_ready(t0 + Duration::from_millis(305)),
            Some(JitterStep::Packet(Bytes::from_static(b"resync")))
        );

        // 7. 24-packet overflow cap (exercised directly when queue exceeds 24 entries) + 21-packet resync boundary.
        let mut jb_cap = AudioJitterBuffer::new();
        for seq in 0u16..=20 {
            jb_cap.push(seq, Bytes::from(vec![seq as u8]), t0);
        }
        assert_eq!(jb_cap.queue.len(), 21);
        assert_eq!(jb_cap.next_seq, Some(0));
        // Pre-fill extra synthetic entries below next_seq + 20 to verify the >24 cap trims oldest.
        for extra in 21i64..26 {
            jb_cap.queue.insert(extra, Bytes::from(vec![extra as u8]));
        }
        jb_cap.push(20, Bytes::from(vec![20u8]), t0);
        assert_eq!(jb_cap.queue.len(), 24);
        assert_eq!(jb_cap.next_seq, Some(2));
        assert_eq!(
            jb_cap.pop_ready(t0),
            Some(JitterStep::Packet(Bytes::from(vec![2u8])))
        );

        // 8. `missing_since` re-arming when popping `next` exposes a new gap.
        let mut jb_rearm = AudioJitterBuffer::new();
        jb_rearm.push(10, Bytes::from_static(b"p10"), t0);
        jb_rearm.push(12, Bytes::from_static(b"p12"), t0);
        // Popping 10 at t0 + 100ms re-arms missing_since to t0 + 100ms for missing 11.
        assert_eq!(
            jb_rearm.pop_ready(t0 + Duration::from_millis(100)),
            Some(JitterStep::Packet(Bytes::from_static(b"p10")))
        );
        // At t0 + 120ms (only 20ms since re-arm), must still wait for 11.
        assert_eq!(jb_rearm.pop_ready(t0 + Duration::from_millis(120)), None);
        // At t0 + 140ms (40ms since re-arm), triggers FEC from 12.
        assert_eq!(
            jb_rearm.pop_ready(t0 + Duration::from_millis(140)),
            Some(JitterStep::FecFromNext(Bytes::from_static(b"p12")))
        );
    }

    #[test]
    fn playback_buffer_overflow_crossfade_exact_drain_clamping_reprime_and_snap() {
        let mut pb = PlaybackBuffer::new();

        // Exact drain stays primed!
        pb.push(&vec![0.5f32; PLAYBACK_PRIME_SAMPLES]);
        assert!(pb.primed);
        let mut exact = vec![0.0f32; PLAYBACK_PRIME_SAMPLES];
        pb.pull_into(&mut exact);
        assert!(pb.primed, "exact drain without shortage must stay primed");
        assert!(exact.iter().all(|&s| (s - 0.5).abs() < 1e-6));

        // Clamping out-of-range samples [-1.0, 1.0].
        pb.push(&[2.5, -3.0, 0.25, -0.75]);
        let mut clamped = vec![0.0f32; 4];
        pb.pull_into(&mut clamped);
        assert_eq!(clamped, vec![1.0, -1.0, 0.25, -0.75]);

        // Re-priming cycle: trigger underrun -> unprimed -> fade snaps to exact 0.0.
        let mut fade = vec![99.0f32; 128];
        pb.pull_into(&mut fade);
        assert!(!pb.primed);
        assert_eq!(
            *fade.last().unwrap(),
            0.0,
            "fade_to_silence must snap to 0.0"
        );
        pb.push(&vec![0.3f32; FRAME_20MS * 2]);
        assert!(!pb.primed);
        pb.push(&vec![0.3f32; FRAME_20MS]);
        assert!(pb.primed);

        // Overflow trim + 32-sample crossfade splice.
        let mut pb_ov = PlaybackBuffer::new();
        // Fill first portion with -0.6 and later portion with +0.6 up to PLAYBACK_MAX_SAMPLES.
        pb_ov.push(&vec![-0.6f32; PLAYBACK_MAX_SAMPLES - PLAYBACK_TRIM_TARGET]);
        pb_ov.push(&vec![0.6f32; PLAYBACK_TRIM_TARGET]);
        assert_eq!(pb_ov.samples.len(), PLAYBACK_MAX_SAMPLES);
        // Pushing 1 more frame exceeds PLAYBACK_MAX_SAMPLES -> trims to PLAYBACK_TRIM_TARGET + FRAME_20MS.
        pb_ov.push(&vec![0.6f32; FRAME_20MS]);
        assert_eq!(pb_ov.samples.len(), PLAYBACK_TRIM_TARGET + FRAME_20MS);
        let mut splice = vec![0.0f32; 40];
        pb_ov.pull_into(&mut splice);
        // First 32 samples ramp smoothly from -0.6 toward +0.6.
        assert!(splice[0] < -0.4);
        assert!(splice[15].abs() < 0.15);
        assert!((splice[31] - 0.6).abs() < 0.1);
        assert!((splice[35] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn dsp_helpers_and_shared_state_edge_cases() {
        // LoudnessNormalizer: empty frame no-op & silence drift toward unity.
        let mut norm = LoudnessNormalizer::speech();
        norm.process_frame(&mut []);
        assert!((norm.gain() - 1.0).abs() < 1e-6);
        let mut loud = vec![0.8f32; FRAME_20MS];
        for _ in 0..10 {
            loud.fill(0.8);
            norm.process_frame(&mut loud);
        }
        let attenuated = norm.gain();
        assert!(attenuated < 0.5);
        let mut silence = vec![0.0f32; FRAME_20MS];
        for _ in 0..100 {
            silence.fill(0.0);
            norm.process_frame(&mut silence);
        }
        assert!(
            norm.gain() > attenuated,
            "silence must drift gain back toward 1.0"
        );

        // soft_limit_sample: linear inside knee, bounded outside, antisymmetric.
        assert_eq!(soft_limit_sample(0.0), 0.0);
        assert_eq!(soft_limit_sample(0.80), 0.80);
        assert_eq!(soft_limit_sample(-0.80), -0.80);
        assert!(soft_limit_sample(10.0) <= 0.98 && soft_limit_sample(10.0) > 0.95);
        assert!((soft_limit_sample(-10.0) + soft_limit_sample(10.0)).abs() < 1e-6);

        // normalized_gain_for_rms edge cases (NaN, Inf, 0, negative, clamp bounds).
        assert!((normalized_gain_for_rms(f32::NAN, 0.1, 0.2, 5.0) - 0.2).abs() < 1e-6);
        assert!((normalized_gain_for_rms(f32::INFINITY, 0.5, 0.2, 5.0) - 0.5).abs() < 1e-6);
        assert!((normalized_gain_for_rms(f32::NEG_INFINITY, 0.5, 0.2, 5.0) - 0.5).abs() < 1e-6);
        assert!((normalized_gain_for_rms(0.0, 0.5, 0.2, 5.0) - 0.5).abs() < 1e-6);
        assert!((normalized_gain_for_rms(-1.0, 0.5, 0.2, 5.0) - 0.5).abs() < 1e-6);
        assert!((normalized_gain_for_rms(0.001, 0.5, 0.2, 5.0) - 5.0).abs() < 1e-6);
        assert!((normalized_gain_for_rms(10.0, 0.5, 0.2, 5.0) - 0.2).abs() < 1e-6);

        // downmix_to_mono_f32.
        assert!(downmix_to_mono_f32(&[], 2).is_empty());
        assert_eq!(downmix_to_mono_f32(&[0.2, -0.4], 1), vec![0.2, -0.4]);
        assert_eq!(
            downmix_to_mono_f32(&[0.2, 0.6, -0.4, 0.4], 2),
            vec![0.4, 0.0]
        );
        assert_eq!(downmix_to_mono_f32(&[1.0, 1.0, 0.0, 0.0], 4), vec![0.5]);

        // resample_linear edge cases.
        assert!(resample_linear(&[], 44_100, 48_000).is_empty());
        assert_eq!(resample_linear(&[0.1, 0.2], 48_000, 48_000), vec![0.1, 0.2]);
        let down = resample_linear(&vec![0.75f32; 480], 48_000, 16_000);
        assert_eq!(down.len(), 160);
        assert!((down[80] - 0.75).abs() < 1e-4);

        // compute_rms_mono.
        assert_eq!(compute_rms_mono(&[]), 0.0);
        assert!((compute_rms_mono(&[0.5; 10]) - 0.5).abs() < 1e-6);
        assert_eq!(compute_rms_mono(&[2.0; 10]), 1.0);

        // AudioSharedState::local_rms respects muted flag.
        let cfg = HuddleAudioConfig::default();
        let shared = AudioSharedState::from_config(&cfg);
        shared
            .local_rms_bits
            .store(0.42f32.to_bits(), Ordering::Relaxed);
        assert!((shared.local_rms() - 0.42).abs() < 1e-6);
        shared.muted.store(true, Ordering::Relaxed);
        assert_eq!(shared.local_rms(), 0.0);
    }

    #[test]
    fn inbound_audio_rx_loop_reorders_fec_plc_primes_playback_and_gates_aec_tap() {
        let mut enc = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip).unwrap();
        let _ = enc.set_bitrate(Bitrate::Bits(32_000));
        let _ = enc.set_inband_fec(true);
        let _ = enc.set_packet_loss_perc(20);

        let mut packets = Vec::new();
        let mut out_buf = vec![0u8; 4000];
        for frame_ix in 0..10usize {
            let pcm: Vec<f32> = (0..FRAME_20MS)
                .map(|i| {
                    let t = (frame_ix * FRAME_20MS + i) as f32 / SAMPLE_RATE as f32;
                    (2.0 * std::f32::consts::PI * 350.0 * t).sin() * 0.25
                })
                .collect();
            let n = enc.encode_float(&pcm, &mut out_buf).unwrap();
            packets.push(Bytes::copy_from_slice(&out_buf[..n]));
        }

        // Case 1: needs_sw_aec_ref = true, unmuted, aec = true -> populates both PlaybackBuffer and aec_render_tap (with overflow trim).
        let cfg = HuddleAudioConfig::default();
        let shared = AudioSharedState::from_config(&cfg);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let playback = Arc::new(Mutex::new(PlaybackBuffer::new()));
        let aec_tap = Arc::new(Mutex::new(VecDeque::<f32>::new()));
        let needs_aec = Arc::new(AtomicBool::new(true));

        let s_clone = shared.clone();
        let pb_clone = Arc::clone(&playback);
        let tap_clone = Arc::clone(&aec_tap);
        let needs_clone = Arc::clone(&needs_aec);
        let handle = std::thread::spawn(move || {
            run_inbound_audio_rx_loop(s_clone, &mut rx, pb_clone, tap_clone, needs_clone);
        });

        // Deliver out of order (100, 102, 101), drop 103 (FEC from 104), then 104..109.
        let send_order = [0usize, 2, 1, 4, 5, 6, 7, 8, 9];
        for idx in send_order {
            tx.send(InboundAudioPacket {
                seq: 100 + idx as u16,
                payload: packets[idx].clone(),
            })
            .unwrap();
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let primed = playback.lock().unwrap().primed;
            let tap_len = aec_tap.lock().unwrap().len();
            if primed && tap_len > 0 {
                // Wait briefly for remaining packets to decode through the jitter buffer.
                if playback.lock().unwrap().samples.len() >= FRAME_20MS * 8 {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        shared.running.store(false, Ordering::Relaxed);
        handle.join().unwrap();

        {
            let mut pb = playback.lock().unwrap();
            assert!(
                pb.primed,
                "playback buffer should be primed after decoding Opus packets"
            );
            let mut rendered = vec![0.0f32; FRAME_20MS * 3];
            pb.pull_into(&mut rendered);
            assert!(
                compute_rms_mono(&rendered) > 0.01,
                "decoded playback audio should contain non-silent tone"
            );
        }
        {
            let tap = aec_tap.lock().unwrap();
            assert!(
                !tap.is_empty(),
                "aec_render_tap should receive decoded samples"
            );
            // Overflow cap trims when > FRAME_20MS * 6 down to FRAME_20MS * 3 before extending.
            assert!(
                tap.len() <= FRAME_20MS * 7,
                "aec_render_tap must be bounded by overflow trim, got {}",
                tap.len()
            );
        }

        // Case 2: needs_sw_aec_ref = false -> aec_render_tap stays empty while playback still works.
        let shared2 = AudioSharedState::from_config(&cfg);
        let (tx2, mut rx2) = mpsc::unbounded_channel();
        let pb2 = Arc::new(Mutex::new(PlaybackBuffer::new()));
        let tap2 = Arc::new(Mutex::new(VecDeque::<f32>::new()));
        let needs2 = Arc::new(AtomicBool::new(false));
        let s2 = shared2.clone();
        let pb2_c = Arc::clone(&pb2);
        let tap2_c = Arc::clone(&tap2);
        let needs2_c = Arc::clone(&needs2);
        let h2 = std::thread::spawn(move || {
            run_inbound_audio_rx_loop(s2, &mut rx2, pb2_c, tap2_c, needs2_c);
        });
        for (i, pkt) in packets.iter().take(4).enumerate() {
            tx2.send(InboundAudioPacket {
                seq: i as u16,
                payload: pkt.clone(),
            })
            .unwrap();
        }
        let dl2 = Instant::now() + Duration::from_secs(2);
        while Instant::now() < dl2 {
            if pb2.lock().unwrap().primed {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        shared2.running.store(false, Ordering::Relaxed);
        h2.join().unwrap();
        assert!(pb2.lock().unwrap().primed);
        assert!(tap2.lock().unwrap().is_empty());
    }

    #[test]
    #[ignore = "benchmark; run with --release -- --ignored --nocapture"]
    fn bench_audio_jitter_and_opus_pipeline() {
        const FRAMES: usize = 500;
        let mut enc = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip).unwrap();
        let _ = enc.set_bitrate(Bitrate::Bits(32_000));
        let _ = enc.set_vbr(true);
        let _ = enc.set_inband_fec(true);
        let _ = enc.set_packet_loss_perc(5);

        let mut dec = Decoder::new(SAMPLE_RATE, Channels::Mono).unwrap();
        let mut jitter = AudioJitterBuffer::new();
        let mut normalizer = LoudnessNormalizer::speech();

        // Pre-generate 20ms speech-like multi-tone PCM frames so benchmark measures DSP + codec + jitter.
        let pcm_frames: Vec<Vec<f32>> = (0..FRAMES)
            .map(|f| {
                (0..FRAME_20MS)
                    .map(|i| {
                        let t = (f * FRAME_20MS + i) as f32 / SAMPLE_RATE as f32;
                        ((2.0 * std::f32::consts::PI * 260.0 * t).sin() * 0.18)
                            + ((2.0 * std::f32::consts::PI * 520.0 * t).sin() * 0.08)
                    })
                    .collect()
            })
            .collect();

        let mut enc_buf = vec![0u8; 4000];
        let mut dec_buf = vec![0.0f32; FRAME_20MS * 2];
        let mut decoded_frames = 0usize;
        let mut fec_count = 0usize;
        let mut plc_count = 0usize;
        let mut pending_swap: Option<(u16, Bytes)> = None;

        let t0 = Instant::now();
        let mut sim_now = t0;

        for (idx, pcm) in pcm_frames.iter().enumerate() {
            let n = enc.encode_float(pcm, &mut enc_buf).unwrap();
            let pkt = Bytes::copy_from_slice(&enc_buf[..n]);
            let seq = idx as u16;
            sim_now += Duration::from_millis(20);

            // Simulate 2% single-packet drop (exercises in-band Opus FEC) and 2% adjacent reorder.
            if idx > 2 && idx % 50 == 25 {
                // Drop packet `seq`; next packet will trigger FEC recovery after 35ms.
            } else if idx > 2 && idx % 50 == 10 && pending_swap.is_none() {
                pending_swap = Some((seq, pkt));
            } else {
                jitter.push(seq, pkt, sim_now);
                if let Some((delayed_seq, delayed_pkt)) = pending_swap.take() {
                    jitter.push(delayed_seq, delayed_pkt, sim_now + Duration::from_millis(4));
                }
            }

            let drain_now = sim_now + Duration::from_millis(40);
            while let Some(step) = jitter.pop_ready(drain_now) {
                let decoded_res = match step {
                    JitterStep::Packet(bytes) => dec.decode_float(&bytes, &mut dec_buf, false),
                    JitterStep::FecFromNext(next) => {
                        fec_count += 1;
                        dec.decode_float(&next, &mut dec_buf[..FRAME_20MS], true)
                    }
                    JitterStep::Plc => {
                        plc_count += 1;
                        decode_opus_or_plc(&mut dec, &[], &mut dec_buf)
                    }
                };
                if let Ok(samples) = decoded_res {
                    normalizer.process_frame(&mut dec_buf[..samples]);
                    decoded_frames += 1;
                }
            }
        }

        let elapsed = t0.elapsed();
        let total_ms = elapsed.as_secs_f64() * 1000.0;
        let fps = decoded_frames as f64 / elapsed.as_secs_f64().max(1e-9);
        let ms_per_frame = total_ms / decoded_frames.max(1) as f64;
        let realtime_x = (decoded_frames as f64 * 20.0) / total_ms.max(1e-6);
        println!(
            "bench_audio_jitter_and_opus_pipeline: {decoded_frames} frames (20ms @ 48kHz) in {total_ms:.2} ms | {fps:.1} frames/sec | {ms_per_frame:.3} ms/frame ({realtime_x:.1}x realtime, fec={fec_count}, plc={plc_count})"
        );
        assert!(
            decoded_frames >= FRAMES - 5,
            "expected near-complete decode with FEC, got {decoded_frames}/{FRAMES}"
        );
        assert!(fec_count > 0, "benchmark should exercise Opus FEC recovery");
    }
}
