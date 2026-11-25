//! Inline audio playback and bounded LRU cache eviction for `$TMPDIR/slackless-media/`.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand};
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::gpui::{App, BackgroundExecutor, Global, Window};
use slackless_core::Fetcher;

use super::downloads::active_fetcher;

/// Maximum cumulative bytes kept in `$TMPDIR/slackless-media/` before evicting oldest files.
const MAX_MEDIA_CACHE_BYTES: u64 = 128 * 1024 * 1024;
/// Maximum number of cached media files kept in `$TMPDIR/slackless-media/`.
const MAX_MEDIA_CACHE_FILES: usize = 200;

/// Snapshot of an audio file's inline playback state.
#[derive(Clone, Debug)]
pub struct AudioSnapshot {
    pub loading: bool,
    pub playing: bool,
    pub paused: bool,
    pub elapsed_ms: u64,
    pub duration_ms: Option<u64>,
}

struct ActiveAudio {
    file_id: String,
    child: Option<Child>,
    started: Option<Instant>,
    paused_at: Option<Instant>,
    accumulated_ms: u64,
    duration_ms: Option<u64>,
    loading: bool,
}

#[derive(Default)]
struct MediaPlayer {
    audio: Option<ActiveAudio>,
}

impl Drop for MediaPlayer {
    fn drop(&mut self) {
        if let Some(active) = self.audio.as_mut()
            && let Some(child) = active.child.as_mut()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Global for MediaPlayer {}

/// Current inline playback state for `file_id`, if it is loading or playing.
pub fn audio_state(file_id: &str, cx: &App) -> Option<AudioSnapshot> {
    let active = cx.try_global::<MediaPlayer>()?.audio.as_ref()?;
    if active.file_id != file_id {
        return None;
    }
    let elapsed_ms = active.accumulated_ms
        + if active.paused_at.is_some() {
            0
        } else {
            active
                .started
                .map(|s| s.elapsed().as_millis() as u64)
                .unwrap_or(0)
        };
    Some(AudioSnapshot {
        loading: active.loading,
        playing: active.child.is_some() && active.paused_at.is_none(),
        paused: active.child.is_some() && active.paused_at.is_some(),
        elapsed_ms,
        duration_ms: active.duration_ms,
    })
}

#[cfg(unix)]
fn send_child_signal(child: &Child, sig: libc::c_int) -> std::io::Result<()> {
    let pid = libc::pid_t::try_from(child.id())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "pid out of range"))?;
    if pid <= 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to signal non-positive pid",
        ));
    }
    // SAFETY: `pid` is a validated positive PID belonging to our own spawned audio child process,
    // and `sig` is a standard POSIX signal constant from `libc` (`SIGSTOP` / `SIGCONT`).
    let rc = unsafe { libc::kill(pid, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Pause or resume the currently active audio clip without restarting (`F-CORE-126`).
pub fn toggle_pause_audio(file_id: &str, window: &mut Window, cx: &mut App) {
    let player = cx.default_global::<MediaPlayer>();
    let Some(active) = player.audio.as_mut() else {
        return;
    };
    if active.file_id != file_id {
        return;
    }
    let Some(child) = active.child.as_ref() else {
        return;
    };
    #[cfg(unix)]
    {
        if active.paused_at.is_some() {
            if let Err(err) = send_child_signal(child, libc::SIGCONT) {
                tracing::warn!("failed to resume audio child process: {err}");
                return;
            }
            active.paused_at = None;
            active.started = Some(Instant::now());
        } else {
            if let Err(err) = send_child_signal(child, libc::SIGSTOP) {
                tracing::warn!("failed to pause audio child process: {err}");
                return;
            }
            if let Some(started) = active.started.take() {
                active.accumulated_ms = active
                    .accumulated_ms
                    .saturating_add(started.elapsed().as_millis() as u64);
            }
            active.paused_at = Some(Instant::now());
        }
        window.refresh();
    }
    #[cfg(not(unix))]
    {
        let _ = (child, window);
    }
}

/// Stop any active audio playback immediately.
pub fn stop_audio(window: &mut Window, cx: &mut App) {
    stop_active_audio(cx.default_global::<MediaPlayer>());
    window.refresh();
}

fn stop_active_audio(player: &mut MediaPlayer) {
    if let Some(mut active) = player.audio.take()
        && let Some(mut child) = active.child.take()
    {
        let _ = child.kill();
        let _ = child.wait();
    }
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
    let write_path = path.clone();
    bg.spawn(async move {
        std::fs::write(&write_path, &bytes).map_err(|e| e.to_string())?;
        prune_media_cache(
            &dir,
            MAX_MEDIA_CACHE_BYTES,
            MAX_MEDIA_CACHE_FILES,
            Some(&write_path),
        );
        Ok::<PathBuf, String>(write_path)
    })
    .await
}

/// Toggle inline playback of a Slack-hosted audio clip or file. Downloads authenticated bytes
/// into `$TMPDIR/slackless-media/` once and plays via macOS `afplay` with live progress ticks.
pub fn toggle_audio(
    file_id: String,
    url: String,
    ext: String,
    duration_ms: Option<u64>,
    window: &mut Window,
    cx: &mut App,
) {
    let already_active = cx
        .try_global::<MediaPlayer>()
        .and_then(|p| p.audio.as_ref())
        .is_some_and(|active| active.file_id == file_id && active.child.is_some());
    if already_active {
        toggle_pause_audio(&file_id, window, cx);
        return;
    }
    stop_active_audio(cx.default_global::<MediaPlayer>());
    let Some(fetcher) = active_fetcher(cx) else {
        crate::actions::notify(
            "Audio playback",
            "Playback needs a signed-in workspace.",
            window,
            cx,
        );
        return;
    };
    cx.default_global::<MediaPlayer>().audio = Some(ActiveAudio {
        file_id: file_id.clone(),
        child: None,
        started: None,
        paused_at: None,
        accumulated_ms: 0,
        duration_ms,
        loading: true,
    });
    window.refresh();
    let cache_id = file_id.clone();
    window
        .spawn(cx, async move |cx| {
            let bg = cx.background_executor().clone();
            let path = fetch_cached_media(fetcher, cache_id, url, ext, bg).await;
            let launched = cx
                .update(|window, cx| {
                    let player = cx.default_global::<MediaPlayer>();
                    if player
                        .audio
                        .as_ref()
                        .is_none_or(|active| active.file_id != file_id)
                    {
                        return false;
                    }
                    match path {
                        Ok(path) => {
                            let spawn_res = {
                                #[cfg(target_os = "macos")]
                                {
                                    ProcessCommand::new("/usr/bin/afplay").arg(&path).spawn()
                                }
                                #[cfg(not(target_os = "macos"))]
                                {
                                    ProcessCommand::new("ffplay")
                                        .args(["-nodisp", "-autoexit"])
                                        .arg(&path)
                                        .spawn()
                                }
                            };
                            match spawn_res {
                                Ok(child) => {
                                    if let Some(active) = player.audio.as_mut() {
                                        active.loading = false;
                                        active.child = Some(child);
                                        active.started = Some(Instant::now());
                                    }
                                    window.refresh();
                                    true
                                }
                                Err(e) => {
                                    player.audio = None;
                                    crate::actions::notify(
                                        "Audio playback failed",
                                        e.to_string(),
                                        window,
                                        cx,
                                    );
                                    window.refresh();
                                    false
                                }
                            }
                        }
                        Err(e) => {
                            player.audio = None;
                            crate::actions::notify("Audio playback failed", e, window, cx);
                            window.refresh();
                            false
                        }
                    }
                })
                .unwrap_or(false);
            if !launched {
                return;
            }
            // Poll playback progress at 4 Hz while audio plays and clean up once finished.
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let keep_going = cx
                    .update(|window, cx| {
                        let player = cx.default_global::<MediaPlayer>();
                        let Some(active) = player.audio.as_mut() else {
                            return false;
                        };
                        if active.file_id != file_id {
                            return false;
                        }
                        let done = active
                            .child
                            .as_mut()
                            .is_none_or(|c| matches!(c.try_wait(), Ok(Some(_)) | Err(_)));
                        if done {
                            player.audio = None;
                            window.refresh();
                            false
                        } else {
                            window.refresh();
                            true
                        }
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
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
}
