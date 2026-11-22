//! Remote images (profile photos, bot icons, custom emoji, the workspace icon) render with
//! GPUI's `img(url)`, which fetches through the app's `HttpClient` and decodes and caches the
//! result. This is that client: requests run on the core's reqwest client and tokio runtime,
//! so the app keeps a single HTTP stack. Images that may animate (GIFs, animated custom emoji)
//! render as a [`Picture`].

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::future::Future;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Child, Command as ProcessCommand};
use std::time::{Duration, Instant};

use gpui_kit::gpui::http_client::http::HeaderValue;
use gpui_kit::gpui::http_client::{AsyncBody, HttpClient, Request, Response, Url};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, Corners, EntityId, Global, ImageSource, ImgResourceLoader, IntoElement,
    ObjectFit, Pixels, RenderImage, RenderOnce, Styled as _, StyledImage as _, Window, canvas, img,
    px,
};
use slackless_core::Fetcher;

pub struct CoreHttp(pub Fetcher);

impl HttpClient for CoreHttp {
    fn user_agent(&self) -> Option<&HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        request: Request<AsyncBody>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Response<AsyncBody>>> + Send>> {
        let bytes = self.0.get(request.uri().to_string());
        Box::pin(async move {
            let bytes = bytes.await.map_err(anyhow::Error::msg)?;
            Ok(Response::builder()
                .status(200)
                .body(AsyncBody::from(bytes))?)
        })
    }
}

/// Saves message files into the user's Downloads folder, as Slack's app does, through the
/// core's signed-in client (Slack's file hosts need the session cookie).
struct Downloader(Fetcher);

impl Global for Downloader {}

/// Live mode: let [`download`] fetch through `fetcher`.
pub fn enable_downloads(fetcher: Fetcher, cx: &mut App) {
    cx.set_global(Downloader(fetcher));
}

/// Fetch `url` through the core's client (live mode; `None` with nothing to fetch with).
pub fn fetch(
    url: String,
    cx: &App,
) -> Option<impl Future<Output = Result<Vec<u8>, String>> + Send + 'static + use<>> {
    Some(cx.try_global::<Downloader>()?.0.get(url))
}

/// Status of one file download initiated in this session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadStatus {
    InProgress,
    Completed(PathBuf),
    Failed(String),
}

/// One entry in the in-app Downloads list (`Cmd+Shift+J`).
#[derive(Clone, Debug)]
pub struct DownloadRecord {
    pub id: usize,
    pub name: String,
    pub url: String,
    pub bytes: Option<usize>,
    pub status: DownloadStatus,
}

#[derive(Default)]
struct DownloadRegistry {
    next_id: usize,
    items: Vec<DownloadRecord>,
    custom_dir: Option<PathBuf>,
    ask_each_time: bool,
}

impl Global for DownloadRegistry {}

/// Override the folder used by [`download`] (`None` reverts to `~/Downloads`).
pub fn set_download_dir(dir: Option<PathBuf>, cx: &mut App) {
    let reg = cx.default_global::<DownloadRegistry>();
    reg.custom_dir = dir;
}

/// Whether clicking Download prompts for a save location per file (`F-SHELL-190`).
pub fn set_ask_download_location(ask: bool, cx: &mut App) {
    let reg = cx.default_global::<DownloadRegistry>();
    reg.ask_each_time = ask;
}

/// Active download directory (custom preference if set, else `~/Downloads`).
pub fn effective_download_dir(cx: &App) -> Option<PathBuf> {
    if let Some(dir) = cx
        .try_global::<DownloadRegistry>()
        .and_then(|reg| reg.custom_dir.clone())
    {
        return Some(dir);
    }
    Some(PathBuf::from(std::env::var_os("HOME")?).join("Downloads"))
}

/// All downloads recorded in this session, newest first.
pub fn downloads(cx: &App) -> Vec<DownloadRecord> {
    cx.try_global::<DownloadRegistry>()
        .map(|reg| reg.items.iter().rev().cloned().collect())
        .unwrap_or_default()
}

/// Remove completed and failed entries from the in-app Downloads list.
pub fn clear_finished_downloads(cx: &mut App) {
    let reg = cx.default_global::<DownloadRegistry>();
    reg.items
        .retain(|item| matches!(item.status, DownloadStatus::InProgress));
}

/// Reveal `path` in Finder (macOS) or its parent folder.
pub fn reveal_in_finder(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = ProcessCommand::new("open").arg("-R").arg(path).spawn();
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(parent) = path.parent() {
            let _ = ProcessCommand::new("xdg-open").arg(parent).spawn();
        }
    }
}

/// Open `path` with the OS default application.
pub fn open_local_file(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = ProcessCommand::new("open").arg(path).spawn();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = ProcessCommand::new("xdg-open").arg(path).spawn();
    }
}

/// Download `url` into ~/Downloads as `name` (`name (1).png` when that is taken), track it in
/// the in-app Downloads list, and notify when it finishes.
pub fn download(url: String, name: String, window: &mut Window, cx: &mut App) {
    if cx
        .try_global::<DownloadRegistry>()
        .is_some_and(|reg| reg.ask_each_time)
    {
        download_as(url, name, window, cx);
        return;
    }
    let Some(fetcher) = cx.try_global::<Downloader>().map(|d| d.0.clone()) else {
        crate::actions::notify(
            "Download",
            "Downloads need a signed-in workspace.",
            window,
            cx,
        );
        return;
    };
    let id = {
        let reg = cx.default_global::<DownloadRegistry>();
        reg.next_id += 1;
        let id = reg.next_id;
        reg.items.push(DownloadRecord {
            id,
            name: name.clone(),
            url: url.clone(),
            bytes: None,
            status: DownloadStatus::InProgress,
        });
        id
    };
    window.refresh();
    let dir = effective_download_dir(cx);
    let bytes = fetcher.get(url);
    window
        .spawn(cx, async move |cx| {
            let saved = match bytes.await {
                Ok(bytes) => {
                    let len = bytes.len();
                    cx.background_executor()
                        .spawn(async move { save_download(dir, &name, &bytes).map(|p| (p, len)) })
                        .await
                }
                Err(e) => Err(e),
            };
            cx.update(|window, cx| {
                let reg = cx.default_global::<DownloadRegistry>();
                match saved {
                    Ok((path, len)) => {
                        if let Some(item) = reg.items.iter_mut().find(|i| i.id == id) {
                            item.bytes = Some(len);
                            item.status = DownloadStatus::Completed(path.clone());
                        }
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        let folder = path
                            .parent()
                            .and_then(|p| p.file_name())
                            .map(|f| f.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "Downloads".to_owned());
                        let message = format!("{name} saved to {folder}.");
                        crate::actions::notify("Downloaded", message, window, cx);
                    }
                    Err(e) => {
                        if let Some(item) = reg.items.iter_mut().find(|i| i.id == id) {
                            item.status = DownloadStatus::Failed(e.clone());
                        }
                        crate::actions::notify("Download failed", e, window, cx);
                    }
                }
                window.refresh();
            })
            .ok();
        })
        .detach();
}

/// Prompt for a destination path ("Save as…"), then download `url` directly to that path.
pub fn download_as(url: String, name: String, window: &mut Window, cx: &mut App) {
    let Some(fetcher) = cx.try_global::<Downloader>().map(|d| d.0.clone()) else {
        crate::actions::notify(
            "Save as",
            "Downloads need a signed-in workspace.",
            window,
            cx,
        );
        return;
    };
    let start_dir = effective_download_dir(cx).unwrap_or_else(|| PathBuf::from("."));
    let rx = cx.prompt_for_new_path(&start_dir, Some(&name));
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(dest))) = rx.await else {
                return;
            };
            let id = cx
                .update(|window, cx| {
                    let reg = cx.default_global::<DownloadRegistry>();
                    reg.next_id += 1;
                    let id = reg.next_id;
                    let disp = dest
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| name.clone());
                    reg.items.push(DownloadRecord {
                        id,
                        name: disp,
                        url: url.clone(),
                        bytes: None,
                        status: DownloadStatus::InProgress,
                    });
                    window.refresh();
                    id
                })
                .ok();
            let saved = match fetcher.get(url).await {
                Ok(bytes) => {
                    let len = bytes.len();
                    let dest_write = dest.clone();
                    cx.background_executor()
                        .spawn(async move {
                            std::fs::write(&dest_write, &bytes)
                                .map(|()| (dest_write, len))
                                .map_err(|e| e.to_string())
                        })
                        .await
                }
                Err(e) => Err(e),
            };
            cx.update(|window, cx| {
                let reg = cx.default_global::<DownloadRegistry>();
                match saved {
                    Ok((path, len)) => {
                        if let Some(id) = id {
                            if let Some(item) = reg.items.iter_mut().find(|i| i.id == id) {
                                item.bytes = Some(len);
                                item.status = DownloadStatus::Completed(path.clone());
                            }
                        }
                        let file = path.file_name().unwrap_or_default().to_string_lossy();
                        crate::actions::notify("Saved", file.into_owned(), window, cx);
                    }
                    Err(e) => {
                        if let Some(id) = id {
                            if let Some(item) = reg.items.iter_mut().find(|i| i.id == id) {
                                item.status = DownloadStatus::Failed(e.clone());
                            }
                        }
                        crate::actions::notify("Save failed", e, window, cx);
                    }
                }
                window.refresh();
            })
            .ok();
        })
        .detach();
}

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
    loading_videos: HashSet<String>,
}

impl Drop for MediaPlayer {
    fn drop(&mut self) {
        if let Some(active) = self.audio.as_mut() {
            if let Some(child) = active.child.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
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
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        const SIGSTOP: i32 = 17;
        const SIGCONT: i32 = 19;
        let pid = child.id() as i32;
        if active.paused_at.is_some() {
            // SAFETY: sending SIGCONT to our own spawned audio child process.
            unsafe { kill(pid, SIGCONT) };
            active.paused_at = None;
            active.started = Some(Instant::now());
        } else {
            if let Some(started) = active.started.take() {
                active.accumulated_ms = active
                    .accumulated_ms
                    .saturating_add(started.elapsed().as_millis() as u64);
            }
            active.paused_at = Some(Instant::now());
            // SAFETY: sending SIGSTOP to our own spawned audio child process.
            unsafe { kill(pid, SIGSTOP) };
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

/// Whether `file_id` is currently buffering for video playback.
pub fn video_loading(file_id: &str, cx: &App) -> bool {
    cx.try_global::<MediaPlayer>()
        .is_some_and(|p| p.loading_videos.contains(file_id))
}

fn stop_active_audio(player: &mut MediaPlayer) {
    if let Some(mut active) = player.audio.take() {
        if let Some(mut child) = active.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn media_cache_path(file_id: &str, ext: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("slackless-media");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
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
    let Some(fetcher) = cx.try_global::<Downloader>().map(|d| d.0.clone()) else {
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
            let cached = cx
                .background_executor()
                .spawn(async move {
                    let path = media_cache_path(&cache_id, &ext)?;
                    Ok::<(PathBuf, bool), String>((path.clone(), path.is_file()))
                })
                .await;
            let path = match cached {
                Ok((path, true)) => Ok(path),
                Ok((path, false)) => match fetcher.get(url).await {
                    Ok(bytes) => {
                        let write_path = path.clone();
                        cx.background_executor()
                            .spawn(async move {
                                std::fs::write(&write_path, &bytes).map_err(|e| e.to_string())?;
                                Ok::<PathBuf, String>(write_path)
                            })
                            .await
                    }
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            };
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

/// Fetch a Slack-hosted video clip (authenticated, cached in `$TMPDIR/slackless-media/`) and
/// play it with native hardware-accelerated QuickLook / QuickTime playback.
pub fn play_video(file_id: String, url: String, ext: String, window: &mut Window, cx: &mut App) {
    let Some(fetcher) = cx.try_global::<Downloader>().map(|d| d.0.clone()) else {
        crate::actions::notify(
            "Video playback",
            "Playback needs a signed-in workspace.",
            window,
            cx,
        );
        return;
    };
    let player = cx.default_global::<MediaPlayer>();
    if !player.loading_videos.insert(file_id.clone()) {
        return;
    }
    window.refresh();
    let cache_id = file_id.clone();
    window
        .spawn(cx, async move |cx| {
            let cached = cx
                .background_executor()
                .spawn(async move {
                    let path = media_cache_path(&cache_id, &ext)?;
                    Ok::<(PathBuf, bool), String>((path.clone(), path.is_file()))
                })
                .await;
            let path = match cached {
                Ok((path, true)) => Ok(path),
                Ok((path, false)) => match fetcher.get(url).await {
                    Ok(bytes) => {
                        let write_path = path.clone();
                        cx.background_executor()
                            .spawn(async move {
                                std::fs::write(&write_path, &bytes).map_err(|e| e.to_string())?;
                                Ok::<PathBuf, String>(write_path)
                            })
                            .await
                    }
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            };
            cx.update(|window, cx| {
                cx.default_global::<MediaPlayer>()
                    .loading_videos
                    .remove(&file_id);
                match path {
                    Ok(path) => open_local_file(&path),
                    Err(e) => crate::actions::notify("Video playback failed", e, window, cx),
                }
                window.refresh();
            })
            .ok();
        })
        .detach();
}

/// Writes `bytes` to `dir`/`name` (or ~/Downloads/`name`), numbering the name like a browser when it is taken.
fn save_download(dir: Option<PathBuf>, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let dir = match dir {
        Some(dir) => dir,
        None => {
            let home = std::env::var_os("HOME").ok_or("no home folder")?;
            PathBuf::from(home).join("Downloads")
        }
    };
    let _ = std::fs::create_dir_all(&dir);
    // A name from Slack never reaches outside the folder.
    let name = Path::new(name)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_owned());
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_owned(), format!(".{ext}")),
        _ => (name.clone(), String::new()),
    };
    let mut path = dir.join(&name);
    for n in 1.. {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(bytes).map_err(|e| e.to_string())?;
                return Ok(path);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                path = dir.join(format!("{stem} ({n}){ext}"));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    unreachable!("the numbering never runs out")
}

/// An image filling its parent; an animated one (a GIF, an animated custom emoji) plays at
/// its own frame delays. GPUI's animated `img` asks for a frame every vsync, redrawing the
/// window about 120 times a second while one shows (68% CPU for one GIF, against 0.6% idle).
/// A `Picture` paints the frame itself from the same cached decode, picks it from a clock
/// every animation shares, and asks for a redraw only when its frame changes, of only the
/// view it is painted in. All animations share one [`TICK`] grid, so however many play, they
/// cost at most 30 redraws a second. A picture that is not painted (scrolled away, or in a
/// covered or minimized window, which macOS does not draw) asks for nothing until it is.
#[derive(IntoElement)]
pub struct Picture {
    source: ImageSource,
    fit: ObjectFit,
    radius: Pixels,
    fallback: Option<Box<dyn Fn() -> AnyElement>>,
}

/// Animation redraws land on this grid: frame changes due within one tick share a redraw.
const TICK: Duration = Duration::from_nanos(1_000_000_000 / 30);

/// The clock animated [`Picture`]s pick their frame from, and the next redraw: when it lands
/// and the views it notifies.
struct Clock {
    epoch: Instant,
    next: Option<(Instant, Vec<EntityId>)>,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            next: None,
        }
    }
}

impl Global for Clock {}

impl Picture {
    pub fn new(source: impl Into<ImageSource>) -> Self {
        Self {
            source: source.into(),
            fit: ObjectFit::Contain,
            radius: px(0.),
            fallback: None,
        }
    }

    pub fn fit(mut self, fit: ObjectFit) -> Self {
        self.fit = fit;
        self
    }

    pub fn radius(mut self, radius: Pixels) -> Self {
        self.radius = radius;
        self
    }

    /// What shows when the image fails to load.
    pub fn fallback(mut self, fallback: impl Fn() -> AnyElement + 'static) -> Self {
        self.fallback = Some(Box::new(fallback));
        self
    }
}

impl RenderOnce for Picture {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let loaded = match &self.source {
            ImageSource::Resource(resource) => window
                .use_asset::<ImgResourceLoader>(resource, cx)
                .and_then(Result::ok),
            ImageSource::Render(data) => Some(data.clone()),
            _ => None,
        };
        // Still images, and any image while it loads or fails, stay GPUI's own img.
        let Some(data) = loaded.filter(|data| data.frame_count() > 1) else {
            return img(self.source)
                .size_full()
                .object_fit(self.fit)
                .rounded(self.radius)
                .when_some(self.fallback, |image, fallback| {
                    image.with_fallback(fallback)
                })
                .into_any_element();
        };
        let (fit, radius) = (self.fit, self.radius);
        canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                // Time snapped to the tick grid, so every view drawn within one tick shows
                // the same frame; with Reduce Motion on, the first frame stays.
                let epoch = cx.default_global::<Clock>().epoch;
                let ticks = epoch.elapsed().as_nanos() / TICK.as_nanos();
                let at = Duration::from_nanos((ticks * TICK.as_nanos()) as u64);
                let (frame, shows_for) = frame_at(&data, at);
                let display = crate::preferences::display_prefs(cx);
                let frozen = !display.animate_emoji || display.reduce_motion || cx.reduce_motion();
                let frame = if frozen { 0 } else { frame };
                let image_bounds = fit.get_bounds(bounds, data.size(frame));
                if let Err(e) = window.paint_image(
                    bounds,
                    image_bounds,
                    Corners::all(radius),
                    data,
                    frame,
                    false,
                ) {
                    eprintln!("slackless: could not paint an animated image ({e})");
                }
                // Only a picture that shows asks for its next frame.
                if !frozen && window.content_mask().bounds.intersects(&bounds) {
                    schedule(epoch + at + shows_for, window.current_view(), cx);
                }
            },
        )
        .size_full()
        .into_any_element()
    }
}

/// The frame showing `elapsed` into `data`'s loop, and how much longer it shows.
fn frame_at(data: &RenderImage, elapsed: Duration) -> (usize, Duration) {
    // Browsers play a delay of 10ms or less as 100ms; so does Slack.
    let delay = |frame| match Duration::from(data.delay(frame)) {
        delay if delay <= Duration::from_millis(10) => Duration::from_millis(100),
        delay => delay,
    };
    let frames = data.frame_count();
    let total: Duration = (0..frames).map(delay).sum();
    let mut left = Duration::from_nanos((elapsed.as_nanos() % total.as_nanos()) as u64);
    for frame in 0..frames {
        let shows = delay(frame);
        if left < shows {
            return (frame, shows - left);
        }
        left -= shows;
    }
    (0, delay(0))
}

/// Redraw `view` at `due`, rounded up to the [`TICK`] grid. A redraw already scheduled no
/// later takes the view along; a later one moves up, keeping the views it had.
fn schedule(due: Instant, view: EntityId, cx: &mut App) {
    let clock = cx.default_global::<Clock>();
    let ticks = due
        .duration_since(clock.epoch)
        .as_nanos()
        .div_ceil(TICK.as_nanos());
    let at = clock.epoch + Duration::from_nanos((ticks * TICK.as_nanos()) as u64);
    let views = match &mut clock.next {
        Some((next, views)) if *next <= at => {
            if !views.contains(&view) {
                views.push(view);
            }
            return;
        }
        next => {
            let mut views = next.take().map(|(_, views)| views).unwrap_or_default();
            if !views.contains(&view) {
                views.push(view);
            }
            views
        }
    };
    clock.next = Some((at, views));
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(at.saturating_duration_since(Instant::now()))
            .await;
        cx.update(|cx| {
            let clock = cx.global_mut::<Clock>();
            // A redraw moved up meanwhile took this one's views along.
            if clock.next.as_ref().is_some_and(|(next, _)| *next == at) {
                let views = clock
                    .next
                    .take()
                    .map(|(_, views)| views)
                    .unwrap_or_default();
                for view in views {
                    cx.notify(view);
                }
            }
        });
    })
    .detach();
}
