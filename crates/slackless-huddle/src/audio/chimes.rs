use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use cpal::traits::{DeviceTrait as _, StreamTrait as _};

use super::devices::select_output_device;
use super::streams::build_output_stream_any_format;
use crate::backend::platform_backend;
use crate::{HuddleError, Result};

const SLACK_CDN_AUDIO_BASE: &str = "https://a.slack-edge.com/bv1-13";
static AUDIO_DOWNLOAD_SEQ: AtomicU64 = AtomicU64::new(0);

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
    #[cfg(test)]
    pub(crate) fn slack_cdn_url(self) -> String {
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

fn slack_sound_cache_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join("Library")
            .join("Caches")
            .join("Slackless")
            .join("sounds");
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
            return PathBuf::from(xdg).join("slackless").join("sounds");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".cache")
                .join("slackless")
                .join("sounds");
        }
    }
    std::env::temp_dir().join("slackless-sounds")
}

/// Ensure official Slack MP3 `filename` is cached on disk, downloading it once from Slack's
/// public CDN (`a.slack-edge.com/bv1-13/`) via `reqwest`. Returns `None` if offline or download
/// fails.
fn ensure_cached_slack_audio(filename: &str) -> Option<PathBuf> {
    let dir = slack_sound_cache_dir();
    let dest = dir.join(filename);
    if std::fs::metadata(&dest).is_ok_and(|m| m.len() > 256) {
        return Some(dest);
    }
    let _ = std::fs::create_dir_all(&dir);
    let url = format!("{SLACK_CDN_AUDIO_BASE}/{filename}");
    let seq = AUDIO_DOWNLOAD_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{filename}.{}.{seq}.tmp", std::process::id()));

    let bytes = crate::transcribe::block_on_http(async move {
        crate::signaling::install_default_crypto_provider();
        let client = reqwest::Client::builder()
            .user_agent("slackless/1.0")
            .timeout(Duration::from_secs(8))
            .build()
            .ok()?;
        let resp = client.get(&url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.bytes().await.ok()
    });

    if let Some(bytes) = bytes
        && bytes.len() > 256
        && std::fs::write(&tmp, &bytes).is_ok()
    {
        let _ = std::fs::rename(&tmp, &dest);
        if std::fs::metadata(&dest).is_ok_and(|m| m.len() > 256) {
            return Some(dest);
        }
    }
    let _ = std::fs::remove_file(&tmp);
    None
}

fn normalized_file_volume(
    path: &Path,
    target_rms: f32,
    min_gain: f32,
    max_gain: f32,
    fallback: f32,
) -> f32 {
    static FILE_RMS_CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<f32>>>> = OnceLock::new();
    let cache = FILE_RMS_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let cached = cache.lock().ok().and_then(|g| g.get(path).copied());
    let measured = match cached {
        Some(v) => v,
        None => {
            let rms = platform_backend().measure_audio_file_rms(path);
            if let Ok(mut g) = cache.lock() {
                g.insert(path.to_path_buf(), rms);
            }
            rms
        }
    };
    match measured {
        Some(rms) => {
            let vol = super::dsp::normalized_gain_for_rms(rms, target_rms, min_gain, max_gain);
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
/// Uses the cached official MP3 via the platform backend's native routed player (with loudness
/// normalization) and falls back to a synthesized chime.
pub fn play_huddle_chime(chime: HuddleChime, output_device: Option<&str>) -> Result<()> {
    let device_name = output_device.map(ToOwned::to_owned);
    std::thread::Builder::new()
        .name("slackless-huddle-chime".to_owned())
        .spawn(move || {
            let backend = platform_backend();
            if backend.supports_routed_audio_files()
                && let Some(path) = ensure_cached_slack_audio(chime.slack_asset_filename())
            {
                // Normalize chimes to ~ -22 dBFS weighted RMS (0.080 linear), supporting real
                // boost up to +20 dB (10.0x) via AVAudioEngine + AVAudioUnitEQ so quiet MP3s
                // match louder ones cleanly.
                let volume = normalized_file_volume(&path, 0.080, 0.15, 10.0, 1.0);
                if backend.play_routed_audio_oneshot(&path, device_name.as_deref(), volume) {
                    return;
                }
                #[cfg(target_os = "macos")]
                {
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
    let channels = (supported.channels() as usize).max(1);
    let duration_secs: f32 = match chime {
        HuddleChime::Join | HuddleChime::PeerJoin => 0.34,
        HuddleChime::Leave | HuddleChime::PeerLeave => 0.28,
        HuddleChime::InviteRing => 0.44,
    };
    let total_frames = (rate * duration_secs) as usize;
    let frame_ix = Arc::new(AtomicU32::new(0));
    let ix_cb = Arc::clone(&frame_ix);

    let Ok(stream) =
        build_output_stream_any_format(&device, supported, move |data: &mut [f32]| {
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
        })
    else {
        return;
    };
    if stream.play().is_ok() {
        std::thread::sleep(Duration::from_millis((duration_secs * 1000.0) as u64 + 80));
    }
    drop(stream);
}

/// Official Slack huddle lounge / idle music player (`F-COLLAB-036`, `F-COLLAB-058`).
///
/// Plays Official Slack's cached lounge tracks (`waiting_muzak` Smooth Jazz by default, plus
/// `waiting`, `subway`, `whistle`, `focus`, `lofi`, `disco`, `zen`, `nature`, `piano`) via the
/// platform backend's native routed audio loop with gentle background volume and instant teardown
/// on `stop()`/`Drop`. Falls back to a soft synthesized lo-fi chord progression when offline or on
/// hosts without native file routing.
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
                let backend = platform_backend();
                if backend.supports_routed_audio_files()
                    && let Some(path) = ensure_cached_slack_audio(&filename)
                {
                    // Normalize every lounge track to an identical gentle background loudness
                    // (-30 dBFS 200Hz-weighted RMS = 0.032 linear). Slack's CDN MP3s span a
                    // 30 dB range (-46 dBFS piano_concerto to -16 dBFS lofihaus), so allow
                    // true linear gain from 0.08x (-22 dB) up to 14.0x (+23 dB) via
                    // AVAudioEngine + AVAudioUnitEQ!
                    let volume = normalized_file_volume(&path, 0.032, 0.08, 14.0, 1.0);
                    if let Some(mut routed) =
                        backend.start_routed_audio_loop(&path, output_device.as_deref(), volume)
                    {
                        while run_flag.load(Ordering::Relaxed) {
                            std::thread::sleep(Duration::from_millis(80));
                        }
                        routed.stop();
                        return;
                    }
                    #[cfg(target_os = "macos")]
                    {
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
                // Offline / portable fallback: warm lo-fi 4-chord progression.
                let Ok(device) = select_output_device(output_device.as_deref()) else {
                    return;
                };
                let Ok(supported) = device.default_output_config() else {
                    return;
                };
                let rate = supported.sample_rate() as f32;
                let channels = (supported.channels() as usize).max(1);
                const CHORDS: [[f32; 4]; 4] = [
                    [261.63, 329.63, 392.00, 493.88],
                    [220.00, 261.63, 329.63, 392.00],
                    [174.61, 220.00, 261.63, 329.63],
                    [196.00, 246.94, 293.66, 329.63],
                ];
                let mut sample_idx: u64 = 0;
                let chord_samples = (rate * 3.5) as u64;
                let Ok(stream) =
                    build_output_stream_any_format(&device, supported, move |out: &mut [f32]| {
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
                    })
                else {
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
