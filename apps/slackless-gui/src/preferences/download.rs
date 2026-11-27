//! Model downloads started from Preferences: the download runs on its own thread and reports
//! over a channel the UI task awaits, so nothing polls.

use std::path::PathBuf;

use gpui_kit::gpui::Context;

use super::{Preferences, format_bytes};

/// One progress report of a model download, whoever downloads it; build it from the
/// downloader's own progress struct.
pub(super) struct ModelProgress {
    pub file_name: String,
    pub file_index: usize,
    pub file_count: usize,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
}

/// What the download thread hands the UI: a status line per report, then the outcome.
pub(super) enum ModelUpdate {
    Progress(String),
    Done(Result<PathBuf, String>),
}

/// Run `download` on a thread named `thread_name`, giving it a progress callback, and show
/// every report and the outcome through `on_update` on the UI thread as they arrive. `Err` when
/// the thread could not start.
pub(super) fn spawn_model_download(
    thread_name: &str,
    download: impl FnOnce(Box<dyn Fn(ModelProgress) + Send + Sync>) -> Result<PathBuf, String>
    + Send
    + 'static,
    cx: &mut Context<Preferences>,
    on_update: impl Fn(&mut Preferences, ModelUpdate, &mut Context<Preferences>) + 'static,
) -> Result<(), String> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::Builder::new()
        .name(thread_name.to_owned())
        .spawn(move || {
            let done_tx = tx.clone();
            let progress: Box<dyn Fn(ModelProgress) + Send + Sync> = Box::new(move |p| {
                let _ = tx.try_send(ModelUpdate::Progress(status_line(&p)));
            });
            let _ = done_tx.try_send(ModelUpdate::Done(download(progress)));
        })
        .map_err(|err| err.to_string())?;
    cx.spawn(async move |this, cx| {
        while let Ok(update) = rx.recv().await {
            let done = matches!(update, ModelUpdate::Done(_));
            let keep = this
                .update(cx, |prefs, cx| {
                    on_update(prefs, update, cx);
                    cx.notify();
                    !done
                })
                .unwrap_or(false);
            if !keep {
                break;
            }
        }
    })
    .detach();
    Ok(())
}

/// `[2/3] model.onnx (12.3 MB · 45%)`: the line both downloaders show.
fn status_line(p: &ModelProgress) -> String {
    let pct = p
        .total_bytes
        .filter(|&t| t > 0)
        .map(|t| format!(" · {}%", (p.downloaded_bytes * 100 / t).min(100)))
        .unwrap_or_default();
    format!(
        "[{}/{}] {} ({}{})",
        p.file_index,
        p.file_count,
        p.file_name,
        format_bytes(p.downloaded_bytes),
        pct
    )
}
