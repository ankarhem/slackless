//! Native in-app inline audio playback (`AVPlayer` via [`NativeVideoPlayer`]) and bounded LRU
//! cache eviction for `$TMPDIR/slackless-media/`.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use gpui_kit::gpui::{App, BackgroundExecutor, Global, Window};
use slackless_core::Fetcher;
use slackless_core::huddle::NativeVideoPlayer;
use slackless_core::maintenance::{
    DEFAULT_MEDIA_CACHE_MAX_DAYS, DEFAULT_MEDIA_CACHE_MAX_MB, MediaCachePolicy,
    prune_media_cache_policy,
};
use slackless_core::types::FileId;

use super::downloads::active_fetcher;
use super::video_player::{sanitize_secs, transcribe_media};

/// User-configurable byte cap for `$TMPDIR/slackless-media/` (`0` = unlimited), set from
/// Preferences via [`set_media_cache_policy`].
static MEDIA_CACHE_MAX_BYTES: AtomicU64 =
    AtomicU64::new(DEFAULT_MEDIA_CACHE_MAX_MB as u64 * 1024 * 1024);
/// User-configurable max file age in days (`0` = never expire).
static MEDIA_CACHE_MAX_AGE_DAYS: AtomicU32 = AtomicU32::new(DEFAULT_MEDIA_CACHE_MAX_DAYS);
/// Maximum number of cached media files kept in `$TMPDIR/slackless-media/`.
const MAX_MEDIA_CACHE_FILES: usize = 200;
/// Lightweight ~10 Hz progress refresh cadence while audio is actively playing.
const AUDIO_TICK_INTERVAL: Duration = Duration::from_millis(100);
/// Brief startup grace ticks while `AVPlayer` finishes preparing the audio item.
const AUDIO_STARTUP_GRACE_TICKS: u32 = 4;

/// Snapshot of an audio file's inline playback state.
#[derive(Clone, Debug)]
pub struct AudioSnapshot {
    pub loading: bool,
    pub playing: bool,
    pub paused: bool,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub rate: f32,
    pub muted: bool,
    #[allow(dead_code)]
    pub elapsed_ms: u64,
    pub duration_ms: Option<u64>,
}

struct ActiveAudio {
    generation: u64,
    tick_gen: u64,
    file_id: String,
    player: Option<NativeVideoPlayer>,
    fallback_duration_ms: Option<u64>,
    loading: bool,
}

#[derive(Default)]
struct MediaPlayer {
    next_generation: u64,
    audio: Option<ActiveAudio>,
}

impl MediaPlayer {
    fn next_gen(&mut self) -> u64 {
        self.next_generation = self.next_generation.wrapping_add(1);
        self.next_generation
    }

    fn stop_all(&mut self) {
        self.next_gen();
        self.audio = None;
    }

    fn snapshot(&self, file_id: &str) -> Option<AudioSnapshot> {
        let active = self.audio.as_ref()?;
        if active.file_id != file_id {
            return None;
        }
        let fallback_secs = active
            .fallback_duration_ms
            .map_or(0.0, |ms| ms as f64 / 1000.0);
        if active.loading {
            return Some(AudioSnapshot {
                loading: true,
                playing: false,
                paused: false,
                position_secs: 0.0,
                duration_secs: fallback_secs,
                rate: 1.0,
                muted: false,
                elapsed_ms: 0,
                duration_ms: active.fallback_duration_ms,
            });
        }
        let player = active.player.as_ref()?;
        let st = player.state();
        let position_secs = sanitize_secs(st.position_secs);
        let player_dur = sanitize_secs(st.duration_secs);
        let duration_secs = if player_dur > 0.0 {
            player_dur
        } else {
            fallback_secs
        };
        let duration_ms = if duration_secs > 0.0 {
            Some((duration_secs * 1000.0).round() as u64)
        } else {
            active.fallback_duration_ms
        };
        let elapsed_ms = (position_secs * 1000.0).round() as u64;
        let playing = st.playing && !st.ended;
        let paused = !playing && !st.ended;
        let rate = if st.rate.is_finite() && st.rate > 0.0 {
            st.rate
        } else {
            1.0
        };
        Some(AudioSnapshot {
            loading: false,
            playing,
            paused,
            position_secs,
            duration_secs,
            rate,
            muted: st.muted,
            elapsed_ms,
            duration_ms,
        })
    }

    fn cycle_rate_for(&mut self, file_id: &str) -> Option<f32> {
        let active = self.audio.as_mut().filter(|a| a.file_id == file_id)?;
        let player = active.player.as_ref()?;
        Some(player.cycle_rate())
    }

    fn toggle_mute_for(&mut self, file_id: &str) -> Option<bool> {
        let active = self.audio.as_mut().filter(|a| a.file_id == file_id)?;
        let player = active.player.as_ref()?;
        let next = !player.state().muted;
        player.set_muted(next);
        Some(next)
    }

    fn seek_for(&mut self, file_id: &str, secs: f64) -> Option<f64> {
        let active = self.audio.as_mut().filter(|a| a.file_id == file_id)?;
        let player = active.player.as_ref()?;
        let duration = sanitize_secs(player.state().duration_secs);
        let target = if duration > 0.0 {
            sanitize_secs(secs).min(duration)
        } else {
            sanitize_secs(secs)
        };
        player.seek(target);
        Some(target)
    }
}

impl Global for MediaPlayer {}

/// Current inline playback state for `file_id`, if it is loading, playing, or paused.
pub fn audio_state(file_id: &str, cx: &App) -> Option<AudioSnapshot> {
    cx.try_global::<MediaPlayer>()?.snapshot(file_id)
}

/// Pause or resume the currently active audio clip without restarting (`F-CORE-126`).
pub fn toggle_pause_audio(file_id: &str, window: &mut Window, cx: &mut App) {
    let kick = {
        let player = cx.default_global::<MediaPlayer>();
        let Some(active) = player.audio.as_mut().filter(|a| a.file_id == file_id) else {
            return;
        };
        let Some(native) = active.player.as_ref() else {
            return;
        };
        let was_playing = native.state().playing;
        native.toggle_play();
        active.tick_gen = active.tick_gen.wrapping_add(1);
        if !was_playing {
            Some((active.generation, active.tick_gen))
        } else {
            // Bumping `tick_gen` immediately terminates any active progress timer loop.
            None
        }
    };
    window.refresh();
    if let Some((generation, tick_gen)) = kick {
        spawn_audio_progress_loop(generation, tick_gen, window, cx);
    }
}

/// Seek the currently active audio clip matching `file_id` to `secs`.
pub fn seek_audio(file_id: &str, secs: f64, window: &mut Window, cx: &mut App) {
    if cx
        .default_global::<MediaPlayer>()
        .seek_for(file_id, secs)
        .is_some()
    {
        window.refresh();
    }
}

/// Cycle playback speed (`1x -> 1.5x -> 2x -> 1x`) on the currently active audio clip.
pub fn cycle_audio_rate(file_id: &str, window: &mut Window, cx: &mut App) -> Option<f32> {
    let rate = cx.default_global::<MediaPlayer>().cycle_rate_for(file_id)?;
    window.refresh();
    Some(rate)
}

/// Toggle mute/unmute on the currently active audio clip.
pub fn toggle_audio_mute(file_id: &str, window: &mut Window, cx: &mut App) {
    if cx
        .default_global::<MediaPlayer>()
        .toggle_mute_for(file_id)
        .is_some()
    {
        window.refresh();
    }
}

/// Stop any active audio playback immediately.
pub fn stop_audio(window: &mut Window, cx: &mut App) {
    cx.default_global::<MediaPlayer>().stop_all();
    window.refresh();
}

fn spawn_audio_progress_loop(generation: u64, tick_gen: u64, window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |cx| {
            run_audio_progress_loop(generation, tick_gen, cx).await;
        })
        .detach();
}

async fn run_audio_progress_loop(
    generation: u64,
    tick_gen: u64,
    cx: &mut gpui_kit::gpui::AsyncWindowContext,
) {
    let mut startup_ticks = 0u32;
    let mut ever_playing = false;
    loop {
        cx.background_executor().timer(AUDIO_TICK_INTERVAL).await;
        let keep_going = cx
            .update(|window, cx| {
                let mgr = cx.default_global::<MediaPlayer>();
                let Some(active) = mgr.audio.as_ref() else {
                    return false;
                };
                if active.generation != generation || active.tick_gen != tick_gen {
                    return false;
                }
                let Some(player) = active.player.as_ref() else {
                    return false;
                };
                let st = player.state();
                if st.failed || st.ended {
                    mgr.audio = None;
                    window.refresh();
                    return false;
                }
                window.refresh();
                if st.playing {
                    ever_playing = true;
                    startup_ticks = 0;
                    true
                } else if !ever_playing && startup_ticks < AUDIO_STARTUP_GRACE_TICKS {
                    startup_ticks += 1;
                    true
                } else {
                    // Paused or stopped: exit immediately for 0% idle CPU.
                    false
                }
            })
            .unwrap_or(false);
        if !keep_going {
            break;
        }
    }
}

/// Apply the user's media cache retention policy to every subsequent cache write.
pub fn set_media_cache_policy(policy: MediaCachePolicy) {
    MEDIA_CACHE_MAX_BYTES.store(policy.max_bytes, Ordering::Relaxed);
    MEDIA_CACHE_MAX_AGE_DAYS.store(policy.max_age_days, Ordering::Relaxed);
}

/// The media cache retention policy currently in effect.
pub fn media_cache_policy() -> MediaCachePolicy {
    MediaCachePolicy {
        max_bytes: MEDIA_CACHE_MAX_BYTES.load(Ordering::Relaxed),
        max_age_days: MEDIA_CACHE_MAX_AGE_DAYS.load(Ordering::Relaxed),
    }
}

/// Enforce the active size/age policy plus the file-count cap on `dir`, never evicting `keep`.
fn enforce_media_cache_limits(dir: &Path, keep: Option<&Path>) {
    let policy = media_cache_policy();
    prune_media_cache_policy(dir, policy.max_bytes, policy.max_age_days, keep);
    prune_media_cache(dir, u64::MAX, MAX_MEDIA_CACHE_FILES, keep);
}

fn media_cache_dir() -> PathBuf {
    std::env::temp_dir().join("slackless-media")
}

fn media_cache_path(dir: &Path, file_id: &str, ext: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let safe_id: String = file_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    let safe_ext: String = ext.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let filename = if safe_ext.is_empty() {
        safe_id
    } else {
        format!("{safe_id}.{safe_ext}")
    };
    Ok(dir.join(filename))
}

/// Evict the oldest files in `dir` until the cache fits within `max_bytes` and `max_files`,
/// never evicting `keep` (the file just written or accessed).
fn prune_media_cache(dir: &Path, max_bytes: u64, max_files: usize, keep: Option<&Path>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
    let mut total_bytes: u64 = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let len = meta.len();
        total_bytes = total_bytes.saturating_add(len);
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        files.push((path, len, modified));
    }
    if files.len() <= max_files && total_bytes <= max_bytes {
        return;
    }
    // Oldest mtime first (LRU approximation; cache hits refresh mtime).
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut remaining_files = files.len();
    for (path, len, _) in files {
        if remaining_files <= max_files && total_bytes <= max_bytes {
            break;
        }
        if keep.is_some_and(|k| k == path.as_path()) {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {
                remaining_files = remaining_files.saturating_sub(1);
                total_bytes = total_bytes.saturating_sub(len);
            }
            Err(err) => {
                tracing::warn!("could not evict cached media {}: {err}", path.display());
            }
        }
    }
}

/// Fetch `url` into the bounded `$TMPDIR/slackless-media/` cache if absent, refreshing mtime on
/// cache hit and enforcing LRU size/count limits after writes.
pub(crate) async fn fetch_cached_media(
    fetcher: Fetcher,
    file_id: String,
    url: String,
    ext: String,
    bg: BackgroundExecutor,
) -> Result<PathBuf, String> {
    let dir = media_cache_dir();
    let check_dir = dir.clone();
    let (path, hit) = bg
        .spawn(async move {
            let path = media_cache_path(&check_dir, &file_id, &ext)?;
            let exists = path.is_file();
            if exists {
                let _ = OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .and_then(|f| f.set_modified(SystemTime::now()));
            }
            Ok::<(PathBuf, bool), String>((path, exists))
        })
        .await?;
    if hit {
        return Ok(path);
    }
    let bytes = fetcher.get(url).await?;
    bg.spawn(async move {
        write_cache_file(&path, bytes.as_slice()).map_err(|e| e.to_string())?;
        enforce_media_cache_limits(&dir, Some(&path));
        Ok::<PathBuf, String>(path)
    })
    .await
}

/// Copy `data` into a unique `.part` sibling of `path`, then rename it into place, so a crash or
/// a concurrent fetch of the same file never leaves a truncated file at the cache key.
fn write_cache_file(path: &Path, mut data: impl std::io::Read) -> std::io::Result<()> {
    static PART_SEQ: AtomicU64 = AtomicU64::new(0);
    let mut part = path.as_os_str().to_owned();
    part.push(format!(
        ".{}-{}.part",
        std::process::id(),
        PART_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let part = PathBuf::from(part);
    let copied = {
        let mut file = std::fs::File::create(&part)?;
        std::io::copy(&mut data, &mut file)
    };
    let written = copied.and_then(|_| std::fs::rename(&part, path));
    if written.is_err()
        && let Err(err) = std::fs::remove_file(&part)
    {
        tracing::warn!("could not remove partial media {}: {err}", part.display());
    }
    written
}

/// Toggle inline playback of a Slack-hosted audio clip or file. Downloads authenticated bytes
/// into `$TMPDIR/slackless-media/` once and plays natively in-app via [`NativeVideoPlayer`].
pub fn toggle_audio(
    file_id: String,
    url: String,
    ext: String,
    duration_ms: Option<u64>,
    window: &mut Window,
    cx: &mut App,
) {
    let existing = cx
        .try_global::<MediaPlayer>()
        .and_then(|p| p.audio.as_ref())
        .filter(|active| active.file_id == file_id)
        .map(|active| (active.loading, active.player.is_some()));
    match existing {
        Some((_, true)) => {
            toggle_pause_audio(&file_id, window, cx);
            return;
        }
        Some((true, false)) => return,
        _ => {}
    }
    let Some(fetcher) = active_fetcher(cx) else {
        crate::actions::notify(
            "Audio playback",
            "Playback needs a signed-in workspace.",
            window,
            cx,
        );
        return;
    };
    let mgr = cx.default_global::<MediaPlayer>();
    let generation = mgr.next_gen();
    mgr.audio = Some(ActiveAudio {
        generation,
        tick_gen: 0,
        file_id: file_id.clone(),
        player: None,
        fallback_duration_ms: duration_ms,
        loading: true,
    });
    window.refresh();

    let cache_id = file_id.clone();
    let fetch_url = url.clone();
    let fetch_ext = ext.clone();
    window
        .spawn(cx, async move |cx| {
            let bg = cx.background_executor().clone();
            let path =
                fetch_cached_media(fetcher, cache_id, fetch_url, fetch_ext, bg.clone()).await;
            let open_res = match path {
                Ok(path) => bg
                    .spawn(async move { NativeVideoPlayer::open(&path) })
                    .await
                    .map_err(|e| e.to_string()),
                Err(err) => Err(err),
            };
            let tick = cx
                .update(|window, cx| {
                    let mgr = cx.default_global::<MediaPlayer>();
                    let matches = mgr
                        .audio
                        .as_ref()
                        .is_some_and(|a| a.generation == generation);
                    if !matches {
                        drop(open_res);
                        return None;
                    }
                    match open_res {
                        Ok(player) => {
                            player.play();
                            let mut tick_gen = 0;
                            if let Some(active) = mgr.audio.as_mut() {
                                active.loading = false;
                                active.player = Some(player);
                                active.tick_gen = active.tick_gen.wrapping_add(1);
                                tick_gen = active.tick_gen;
                            }
                            transcribe_media(
                                FileId::from(file_id.clone()),
                                url,
                                ext,
                                true,
                                window,
                                cx,
                            );
                            window.refresh();
                            Some(tick_gen)
                        }
                        Err(e) => {
                            mgr.audio = None;
                            crate::actions::notify("Audio playback failed", e, window, cx);
                            window.refresh();
                            None
                        }
                    }
                })
                .ok()
                .flatten();
            if let Some(tick_gen) = tick {
                run_audio_progress_loop(generation, tick_gen, cx).await;
            }
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_media_cache_evicts_oldest_files_by_count_and_bytes() {
        let dir =
            std::env::temp_dir().join(format!("slackless-media-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp cache dir");

        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let write_with_mtime = |name: &str, bytes: &[u8], offset_secs: u64| {
            let path = dir.join(name);
            std::fs::write(&path, bytes).expect("write test cache file");
            let f = OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open test cache file");
            f.set_modified(base + Duration::from_secs(offset_secs))
                .expect("set mtime");
            path
        };

        let f1 = write_with_mtime("old1.m4a", &[0u8; 40], 10);
        let f2 = write_with_mtime("old2.m4a", &[0u8; 40], 20);
        let f3 = write_with_mtime("mid.m4a", &[0u8; 40], 30);
        let f4 = write_with_mtime("newest.m4a", &[0u8; 40], 40);

        // Byte cap (90 bytes): evicts old1 and old2, keeping mid + newest (80 bytes).
        prune_media_cache(&dir, 90, 10, Some(&f4));
        assert!(!f1.exists());
        assert!(!f2.exists());
        assert!(f3.exists());
        assert!(f4.exists());

        // File count cap (1 file): evicts mid, keeping newest.
        prune_media_cache(&dir, 1024, 1, Some(&f4));
        assert!(!f3.exists());
        assert!(f4.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_writes_expose_only_complete_files() {
        struct ConnectionReset;
        impl std::io::Read for ConnectionReset {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("connection reset"))
            }
        }
        let dir =
            std::env::temp_dir().join(format!("slackless-media-write-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("F1.mp4");
        let entries = || std::fs::read_dir(&dir).unwrap().count();

        // The bytes stop mid-file (connection reset, app quit): nothing may sit at the cache key.
        let partial = std::io::Read::chain(&b"first half"[..], ConnectionReset);
        assert!(write_cache_file(&path, partial).is_err());
        assert!(
            !path.exists(),
            "a partial file at the cache key is served as a cache hit forever"
        );
        assert_eq!(entries(), 0, "the partial download is removed");

        write_cache_file(&path, &b"whole clip"[..]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"whole clip");
        assert_eq!(entries(), 1, "no .part file is left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn media_cache_policy_round_trips_and_limits_apply_to_writes() {
        let original = media_cache_policy();
        let dir = std::env::temp_dir().join(format!(
            "slackless-media-policy-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        let aged = |name: &str, age_days: u64| {
            let path = dir.join(name);
            std::fs::write(&path, [0u8; 64]).unwrap();
            OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .set_modified(now - Duration::from_secs(age_days * 86_400))
                .unwrap();
            path
        };
        let stale = aged("stale.mp4", 10);
        let recent = aged("recent.mp4", 1);
        let written = aged("written.mp4", 20);

        set_media_cache_policy(MediaCachePolicy {
            max_bytes: 0,
            max_age_days: 7,
        });
        assert_eq!(media_cache_policy().max_age_days, 7);
        enforce_media_cache_limits(&dir, Some(&written));
        assert!(!stale.exists(), "files past max age are evicted");
        assert!(recent.exists());
        assert!(written.exists(), "the file just written is never evicted");

        set_media_cache_policy(original);
        assert_eq!(media_cache_policy(), original);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn media_player_loading_snapshot_and_stop_lifecycle() {
        let mut mgr = MediaPlayer::default();
        assert!(mgr.snapshot("F_AUD1").is_none());

        let g1 = mgr.next_gen();
        mgr.audio = Some(ActiveAudio {
            generation: g1,
            tick_gen: 0,
            file_id: "F_AUD1".into(),
            player: None,
            fallback_duration_ms: Some(12_500),
            loading: true,
        });
        assert!(mgr.snapshot("F_OTHER").is_none());
        let snap = mgr.snapshot("F_AUD1").expect("loading snapshot");
        assert!(snap.loading);
        assert!(!snap.playing);
        assert!(!snap.paused);
        assert_eq!(snap.rate, 1.0);
        assert!(!snap.muted);
        assert_eq!(snap.duration_ms, Some(12_500));
        assert!((snap.duration_secs - 12.5).abs() < 1e-6);

        mgr.stop_all();
        assert!(mgr.snapshot("F_AUD1").is_none());
        assert!(mgr.next_generation > g1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_audio_player_speed_cycle_mute_and_seek() {
        // Synthesize a 1-second 16 kHz mono PCM WAV file for AVPlayer audio-only playback.
        let sample_rate = 16_000u32;
        let num_samples = sample_rate as usize;
        let data_bytes = (num_samples * 2) as u32;
        let mut wav = Vec::with_capacity(44 + data_bytes as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_bytes.to_le_bytes());
        for i in 0..num_samples {
            let t = i as f32 / sample_rate as f32;
            let sample = ((t * 440.0 * std::f32::consts::TAU).sin() * 4000.0) as i16;
            wav.extend_from_slice(&sample.to_le_bytes());
        }

        let path = std::env::temp_dir().join(format!(
            "slackless-audio-player-test-{}.wav",
            std::process::id()
        ));
        std::fs::write(&path, &wav).expect("write synthetic wav");

        let player = NativeVideoPlayer::open(&path).expect("open audio-only wav");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while player.state().duration_secs <= 0.0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }

        let mut mgr = MediaPlayer::default();
        let g = mgr.next_gen();
        mgr.audio = Some(ActiveAudio {
            generation: g,
            tick_gen: 0,
            file_id: "F_WAV".into(),
            player: Some(player),
            fallback_duration_ms: None,
            loading: false,
        });

        let snap0 = mgr.snapshot("F_WAV").expect("initial snapshot");
        assert!(!snap0.loading);
        assert_eq!(snap0.rate, 1.0);
        assert!(!snap0.muted);

        // Speed cycling: 1.0 -> 1.5 -> 2.0 -> 1.0
        assert_eq!(mgr.cycle_rate_for("F_WAV"), Some(1.5));
        assert_eq!(mgr.snapshot("F_WAV").unwrap().rate, 1.5);
        assert_eq!(mgr.cycle_rate_for("F_WAV"), Some(2.0));
        assert_eq!(mgr.snapshot("F_WAV").unwrap().rate, 2.0);
        assert_eq!(mgr.cycle_rate_for("F_WAV"), Some(1.0));
        assert_eq!(mgr.snapshot("F_WAV").unwrap().rate, 1.0);

        // Mute toggle
        assert_eq!(mgr.toggle_mute_for("F_WAV"), Some(true));
        assert!(mgr.snapshot("F_WAV").unwrap().muted);
        assert_eq!(mgr.toggle_mute_for("F_WAV"), Some(false));
        assert!(!mgr.snapshot("F_WAV").unwrap().muted);

        // Seek clamped within duration
        let sought = mgr.seek_for("F_WAV", 0.5).expect("seek succeeded");
        assert!(sought <= 1.1);

        mgr.stop_all();
        assert!(mgr.snapshot("F_WAV").is_none());
        let _ = std::fs::remove_file(&path);
    }
}
