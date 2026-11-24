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

/// Enumerate all host microphones and speakers.
pub fn list_audio_devices() -> AudioDevices {
    let host = cpal::default_host();
    let default_in = host.default_input_device().and_then(|d| d.name().ok());
    let default_out = host.default_output_device().and_then(|d| d.name().ok());

    let inputs = host
        .input_devices()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|d| d.name().ok())
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
        .filter_map(|d| d.name().ok())
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
    let rate = supported.sample_rate().0 as f32;
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
        &config,
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
                let rate = supported.sample_rate().0 as f32;
                let channels = supported.channels() as usize;
                let total_frames = (rate * 0.45) as usize;
                let frame_ix = Arc::new(AtomicU32::new(0));
                let ix_cb = Arc::clone(&frame_ix);

                let config: StreamConfig = supported.into();
                let stream = device
                    .build_output_stream(
                        &config,
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
                            &config,
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
        .and_then(|d| d.name().ok())
}

fn current_default_output_name() -> Option<String> {
    cpal::default_host()
        .default_output_device()
        .and_then(|d| d.name().ok())
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
                {
                    if let Ok(mut tap) = aec_render_tap.lock() {
                        if tap.len() > FRAME_20MS * 6 {
                            let drop_n = tap.len() - FRAME_20MS * 3;
                            tap.drain(..drop_n);
                        }
                        tap.extend(pcm.iter().copied());
                    }
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
                        if now_in.is_some() && now_in != last_default_in {
                            if let Ok(next_stream) =
                                open_input_stream(None, Arc::clone(&capture_signal))
                            {
                                *in_stream = next_stream;
                                last_default_in = now_in;
                            }
                        }
                    }
                    if configured_out.is_none() && (check_out || poll_due) {
                        let now_out = current_default_output_name();
                        if now_out.is_some() && now_out != last_default_out {
                            if let Ok(next_stream) =
                                open_output_stream(None, Arc::clone(&playback_buf))
                            {
                                *out_stream = next_stream;
                                last_default_out = now_out;
                            }
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
    if let Some(target) = name.filter(|s| !s.is_empty()) {
        if let Ok(mut devices) = host.input_devices() {
            if let Some(found) = devices.find(|d| d.name().ok().as_deref() == Some(target)) {
                return Ok(found);
            }
        }
    }
    host.default_input_device()
        .ok_or_else(|| HuddleError::Audio("no input microphone device found".to_owned()))
}

fn select_output_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if let Some(target) = name.filter(|s| !s.is_empty()) {
        if let Ok(mut devices) = host.output_devices() {
            if let Some(found) = devices.find(|d| d.name().ok().as_deref() == Some(target)) {
                return Ok(found);
            }
        }
    }
    host.default_output_device()
        .ok_or_else(|| HuddleError::Audio("no output speaker device found".to_owned()))
}

fn open_input_stream(name: Option<&str>, signal: CaptureSignal) -> Result<Stream> {
    let device = select_input_device(name)?;
    let supported = device
        .default_input_config()
        .map_err(|e| HuddleError::Audio(e.to_string()))?;
    let src_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let fmt = supported.sample_format();
    let config: StreamConfig = supported.into();

    let mut mono_scratch = Vec::with_capacity(FRAME_20MS * 2);
    let mut resampled_scratch = Vec::with_capacity(FRAME_20MS * 2);

    let stream = match fmt {
        SampleFormat::F32 => device.build_input_stream(
            &config,
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
            &config,
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
    let dst_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let config: StreamConfig = supported.into();

    let mut src_scratch = Vec::with_capacity(FRAME_20MS * 2);
    let mut resampled_scratch = Vec::with_capacity(FRAME_20MS * 2);

    let stream = device
        .build_output_stream(
            &config,
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
                let rate = supported.sample_rate().0 as f32;
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
                    &config,
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
}
