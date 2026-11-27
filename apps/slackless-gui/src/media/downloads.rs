//! Authenticated file downloads, session download history (`Cmd+Shift+J`), and OS file/folder
//! reveal helpers.

use std::fs::OpenOptions;
use std::future::Future;
use std::io::ErrorKind;
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
pub(crate) fn active_fetcher(cx: &App) -> Option<Fetcher> {
    cx.try_global::<Downloader>().map(|d| d.0.clone())
}

/// Fetch `url` through the core's client (live mode; `None` with nothing to fetch with).
pub fn fetch(
    url: String,
    cx: &App,
) -> Option<impl Future<Output = Result<Vec<u8>, String>> + Send + 'static + use<>> {
    let body = cx.try_global::<Downloader>()?.0.get(url);
    Some(async move { body.await.map(Vec::from) })
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
    pub bytes: Option<u64>,
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

    fn complete(&mut self, id: usize, path: PathBuf, len: u64) {
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
    /// The path to download into: a new numbered file in the folder, created empty so no other
    /// download takes the name, or the path picked in the Save dialog.
    fn reserve(&self) -> Result<PathBuf, String> {
        match self {
            DownloadDestination::DefaultFolder { dir, name } => reserve_download(dir.clone(), name),
            DownloadDestination::ExplicitPath(dest) => Ok(dest.clone()),
        }
    }

    /// Quarantines a finished download, or gives back the name a failed one reserved (a failed
    /// download never touches the path itself).
    fn finish(
        &self,
        path: PathBuf,
        written: Result<u64, String>,
    ) -> Result<(PathBuf, u64), String> {
        match written {
            Ok(len) => {
                #[cfg(target_os = "macos")]
                quarantine(&path);
                Ok((path, len))
            }
            Err(err) => {
                if matches!(self, DownloadDestination::DefaultFolder { .. })
                    && let Err(remove_err) = std::fs::remove_file(&path)
                {
                    tracing::warn!(
                        path = %path.display(),
                        error = %remove_err,
                        "could not remove a failed download's empty file"
                    );
                }
                Err(err)
            }
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

/// Shared download execution pipeline used by [`download`], [`download_as`] and
/// `save_download`: from the background executor, `write` fills the file reserved for
/// `destination` and resolves to its length; then the listed download `id` is updated in
/// `DownloadRegistry`, the user notified, and the window refreshed.
async fn execute_download<F>(
    write: impl FnOnce(PathBuf) -> F + Send + 'static,
    id: Option<usize>,
    destination: DownloadDestination,
    cx: &mut AsyncWindowContext,
) where
    F: Future<Output = Result<u64, String>> + Send + 'static,
{
    let fail_title = destination.failure_title();
    let saved = cx
        .background_executor()
        .spawn(async move {
            let res = match destination.reserve() {
                Ok(path) => {
                    let written = write(path.clone()).await;
                    destination.finish(path, written)
                }
                Err(e) => Err(e),
            };
            let success_msg = res
                .as_ref()
                .ok()
                .map(|(path, _)| destination.success_notification(path));
            (res, success_msg)
        })
        .await;
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
            let write = move |path| fetcher.download_to_path(url, path);
            execute_download(write, Some(id), destination, cx).await;
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
            let write = move |path| fetcher.download_to_path(url, path);
            execute_download(write, id, DownloadDestination::ExplicitPath(dest), cx).await;
        })
        .detach();
}

/// Save `bytes` the app made itself (the canvas PDF) into ~/Downloads as `name` (`name (1).pdf`
/// when that is taken), with the toast a finished download shows. It stays out of the in-app
/// Downloads list, whose Retry fetches a URL again.
#[cfg(target_os = "macos")]
pub fn save_download(name: String, bytes: Vec<u8>, window: &mut Window, cx: &mut App) {
    let dir = effective_download_dir(cx);
    let destination = DownloadDestination::DefaultFolder { dir, name };
    let write = move |path: PathBuf| async move {
        std::fs::write(&path, &bytes)
            .map(|()| bytes.len() as u64)
            .map_err(|e| e.to_string())
    };
    window
        .spawn(cx, async move |cx| {
            execute_download(write, None, destination, cx).await;
        })
        .detach();
}

/// The extended attribute macOS keeps a file's download quarantine in.
#[cfg(target_os = "macos")]
const QUARANTINE_XATTR: &std::ffi::CStr = c"com.apple.quarantine";

/// Mark the saved `path` as downloaded from the internet, as a browser does: Gatekeeper then
/// checks an app or installer from Slack (or one unzipped from it) before it first runs. A
/// failure is logged; the file stays saved.
#[cfg(target_os = "macos")]
fn quarantine(path: &Path) {
    if let Err(err) = set_quarantine(path) {
        tracing::warn!(path = %path.display(), error = %err, "could not quarantine a download");
    }
}

/// Write `com.apple.quarantine` as `<flags>;<hex unix time>;<agent>;<event UUID>`, with no
/// event UUID. The flags are the ones non-sandboxed downloaders write (Firefox, Chrome,
/// AirDrop): `QTN_FLAG_DOWNLOAD` (`0x0001`) and `0x0080`. Sandboxed ones (Safari, Mail) also
/// get `QTN_FLAG_SANDBOX` (`0x0002`), making `0083`.
#[cfg(target_os = "macos")]
fn set_quarantine(path: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt as _;
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let value = format!("0081;{now:x};Slackless;");
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: `path` and `QUARANTINE_XATTR` are NUL-terminated and `value` is readable for
    // `value.len()` bytes; all three outlive the call, which only reads them.
    let status = unsafe {
        libc::setxattr(
            path.as_ptr(),
            QUARANTINE_XATTR.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Creates an empty `dir`/`name` (or ~/Downloads/`name`) to download into, numbering the name
/// like a browser when it is taken, and returns its path.
fn reserve_download(dir: Option<PathBuf>, name: &str) -> Result<PathBuf, String> {
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
            Ok(_) => return Ok(path),
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
    fn download_names_are_sanitized_numbered_and_freed_on_failure() {
        let dir = std::env::temp_dir().join(format!("slackless-dl-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let p1 = reserve_download(Some(dir.clone()), "../../secret/report.pdf").unwrap();
        assert_eq!(p1, dir.join("report.pdf"));
        assert!(p1.is_file(), "the name is held while the download runs");

        let p2 = reserve_download(Some(dir.clone()), "report.pdf").unwrap();
        assert_eq!(p2, dir.join("report (1).pdf"));

        let p3 = reserve_download(Some(dir.clone()), "report.pdf").unwrap();
        assert_eq!(p3, dir.join("report (2).pdf"));

        let no_ext = reserve_download(Some(dir.clone()), "README").unwrap();
        assert_eq!(no_ext, dir.join("README"));
        let no_ext_2 = reserve_download(Some(dir.clone()), "README").unwrap();
        assert_eq!(no_ext_2, dir.join("README (1)"));

        // A failed download gives its name back instead of leaving an empty file.
        let failing = DownloadDestination::DefaultFolder {
            dir: Some(dir.clone()),
            name: "report.pdf".to_owned(),
        };
        let reserved = failing.reserve().unwrap();
        assert_eq!(reserved, dir.join("report (3).pdf"));
        assert!(
            failing
                .finish(reserved.clone(), Err("connection reset".to_owned()))
                .is_err()
        );
        assert!(!reserved.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn saved_downloads_are_quarantined_like_browser_downloads() {
        use std::os::unix::ffi::OsStrExt as _;
        use std::time::{SystemTime, UNIX_EPOCH};

        let now = || {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
        };
        let quarantine_of = |path: &Path| {
            let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            let mut value = [0u8; 256];
            // SAFETY: `path` and the name are NUL-terminated; `value` is writable for its length.
            let len = unsafe {
                libc::getxattr(
                    path.as_ptr(),
                    QUARANTINE_XATTR.as_ptr(),
                    value.as_mut_ptr().cast(),
                    value.len(),
                    0,
                    0,
                )
            };
            let len = usize::try_from(len).expect("the saved file carries com.apple.quarantine");
            String::from_utf8(value[..len].to_vec()).unwrap()
        };
        let dir = std::env::temp_dir().join(format!(
            "slackless-dl-quarantine-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let start = now();
        let saved: Vec<PathBuf> = [
            DownloadDestination::DefaultFolder {
                dir: Some(dir.clone()),
                name: "Installer.zip".to_owned(),
            },
            DownloadDestination::ExplicitPath(dir.join("Chosen.zip")),
        ]
        .iter()
        .map(|destination| {
            let path = destination.reserve().unwrap();
            // Stands in for the streamed body.
            std::fs::write(&path, b"PK").unwrap();
            destination.finish(path, Ok(2)).unwrap().0
        })
        .collect();
        let end = now();

        for path in saved {
            let value = quarantine_of(&path);
            let fields: Vec<&str> = value.split(';').collect();
            let flags = u32::from_str_radix(fields[0], 16).unwrap();
            assert_ne!(flags & 0x0001, 0, "QTN_FLAG_DOWNLOAD in {value}");
            let stamped = u64::from_str_radix(fields[1], 16).unwrap();
            assert!((start..=end).contains(&stamped), "download time in {value}");
            assert_eq!(fields[2], "Slackless", "{value}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
