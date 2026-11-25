//! Authenticated file downloads, session download history (`Cmd+Shift+J`), and OS file/folder
//! reveal helpers.

use std::fs::OpenOptions;
use std::future::Future;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use gpui_kit::gpui::{App, AsyncWindowContext, Global, Window};
use slackless_core::Fetcher;

/// Saves message files into the user's Downloads folder, as Slack's app does, through the
/// core's signed-in client (Slack's file hosts need the session cookie).
pub(super) struct Downloader(pub(super) Fetcher);

impl Global for Downloader {}

/// Live mode: let [`download`] fetch through `fetcher`.
pub fn enable_downloads(fetcher: Fetcher, cx: &mut App) {
    cx.set_global(Downloader(fetcher));
}

/// Clone the active authenticated [`Fetcher`] if signed in.
pub(super) fn active_fetcher(cx: &App) -> Option<Fetcher> {
    cx.try_global::<Downloader>().map(|d| d.0.clone())
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

impl DownloadRegistry {
    fn register(&mut self, name: String, url: String) -> usize {
        self.next_id += 1;
        let id = self.next_id;
        self.items.push(DownloadRecord {
            id,
            name,
            url,
            bytes: None,
            status: DownloadStatus::InProgress,
        });
        id
    }

    fn complete(&mut self, id: usize, path: PathBuf, len: usize) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.bytes = Some(len);
            item.status = DownloadStatus::Completed(path);
        }
    }

    fn fail(&mut self, id: usize, err: String) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.status = DownloadStatus::Failed(err);
        }
    }
}

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

fn spawn_detached(mut cmd: ProcessCommand) {
    if let Ok(mut child) = cmd.spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Reveal `path` in Finder (macOS) or its parent folder.
pub fn reveal_in_finder(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = ProcessCommand::new("open");
        cmd.arg("-R").arg("--").arg(path);
        spawn_detached(cmd);
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(parent) = path.parent() {
            let mut cmd = ProcessCommand::new("xdg-open");
            cmd.arg(parent);
            spawn_detached(cmd);
        }
    }
}

/// Open `path` with the OS default application.
pub fn open_local_file(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = ProcessCommand::new("open");
        cmd.arg("--").arg(path);
        spawn_detached(cmd);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut cmd = ProcessCommand::new("xdg-open");
        cmd.arg(path);
        spawn_detached(cmd);
    }
}

enum DownloadDestination {
    /// Save into `dir` (or `~/Downloads`) with collision-avoiding numbering.
    DefaultFolder { dir: Option<PathBuf>, name: String },
    /// Write directly to the user-chosen path from a Save As dialog.
    ExplicitPath(PathBuf),
}

impl DownloadDestination {
    fn write(&self, bytes: &[u8]) -> Result<(PathBuf, usize), String> {
        let len = bytes.len();
        match self {
            DownloadDestination::DefaultFolder { dir, name } => {
                save_download(dir.clone(), name, bytes).map(|p| (p, len))
            }
            DownloadDestination::ExplicitPath(dest) => std::fs::write(dest, bytes)
                .map(|()| (dest.clone(), len))
                .map_err(|e| e.to_string()),
        }
    }

    fn success_notification(&self, path: &Path) -> (&'static str, String) {
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        match self {
            DownloadDestination::DefaultFolder { .. } => {
                let folder = path
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Downloads".to_owned());
                ("Downloaded", format!("{file} saved to {folder}."))
            }
            DownloadDestination::ExplicitPath(_) => ("Saved", file.into_owned()),
        }
    }

    fn failure_title(&self) -> &'static str {
        match self {
            DownloadDestination::DefaultFolder { .. } => "Download failed",
            DownloadDestination::ExplicitPath(_) => "Save failed",
        }
    }
}

/// Shared download execution pipeline used by both [`download`] and [`download_as`]:
/// fetches bytes via `fetcher`, writes them to `destination` on the background executor, updates
/// `DownloadRegistry`, notifies the user, and refreshes the window.
async fn execute_download(
    fetcher: Fetcher,
    url: String,
    id: Option<usize>,
    destination: DownloadDestination,
    cx: &mut AsyncWindowContext,
) {
    let fail_title = destination.failure_title();
    let saved = match fetcher.get(url).await {
        Ok(bytes) => {
            cx.background_executor()
                .spawn(async move {
                    let res = destination.write(&bytes);
                    let success_msg = res
                        .as_ref()
                        .ok()
                        .map(|(path, _)| destination.success_notification(path));
                    (res, success_msg)
                })
                .await
        }
        Err(e) => (Err(e), None),
    };
    cx.update(|window, cx| {
        let reg = cx.default_global::<DownloadRegistry>();
        match saved {
            (Ok((path, len)), Some((title, message))) => {
                if let Some(id) = id {
                    reg.complete(id, path, len);
                }
                crate::actions::notify(title, message, window, cx);
            }
            (Ok((path, len)), None) => {
                let file = path.file_name().unwrap_or_default().to_string_lossy();
                let message = file.into_owned();
                if let Some(id) = id {
                    reg.complete(id, path, len);
                }
                crate::actions::notify("Saved", message, window, cx);
            }
            (Err(e), _) => {
                if let Some(id) = id {
                    reg.fail(id, e.clone());
                }
                crate::actions::notify(fail_title, e, window, cx);
            }
        }
        window.refresh();
    })
    .ok();
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
    let Some(fetcher) = active_fetcher(cx) else {
        crate::actions::notify(
            "Download",
            "Downloads need a signed-in workspace.",
            window,
            cx,
        );
        return;
    };
    let id = cx
        .default_global::<DownloadRegistry>()
        .register(name.clone(), url.clone());
    window.refresh();
    let dir = effective_download_dir(cx);
    let destination = DownloadDestination::DefaultFolder { dir, name };
    window
        .spawn(cx, async move |cx| {
            execute_download(fetcher, url, Some(id), destination, cx).await;
        })
        .detach();
}

/// Prompt for a destination path ("Save as…"), then download `url` directly to that path.
pub fn download_as(url: String, name: String, window: &mut Window, cx: &mut App) {
    let Some(fetcher) = active_fetcher(cx) else {
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
                    let disp = dest
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| name.clone());
                    let id = cx
                        .default_global::<DownloadRegistry>()
                        .register(disp, url.clone());
                    window.refresh();
                    id
                })
                .ok();
            execute_download(
                fetcher,
                url,
                id,
                DownloadDestination::ExplicitPath(dest),
                cx,
            )
            .await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_download_sanitizes_traversal_and_numbers_collisions() {
        let dir = std::env::temp_dir().join(format!("slackless-dl-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let p1 = save_download(Some(dir.clone()), "../../secret/report.pdf", b"v1").unwrap();
        assert_eq!(p1, dir.join("report.pdf"));
        assert_eq!(std::fs::read(&p1).unwrap(), b"v1");

        let p2 = save_download(Some(dir.clone()), "report.pdf", b"v2").unwrap();
        assert_eq!(p2, dir.join("report (1).pdf"));
        assert_eq!(std::fs::read(&p2).unwrap(), b"v2");

        let p3 = save_download(Some(dir.clone()), "report.pdf", b"v3").unwrap();
        assert_eq!(p3, dir.join("report (2).pdf"));

        let no_ext = save_download(Some(dir.clone()), "README", b"hi").unwrap();
        assert_eq!(no_ext, dir.join("README"));
        let no_ext_2 = save_download(Some(dir.clone()), "README", b"hi2").unwrap();
        assert_eq!(no_ext_2, dir.join("README (1)"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
