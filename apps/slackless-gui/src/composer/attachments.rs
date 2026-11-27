//! File, clipboard image, and `+` upload attachment staging for [`Composer`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::gpui::{Context, Image, ImageFormat, PathPromptOptions};

use super::{Composer, MAX_UPLOAD_BYTES, PendingAttachment};

/// Double-click threshold on the `+` button for direct upload (`"Tip: double click to upload"`).
pub(super) const PLUS_DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(380);

/// Map a file path's extension to an inline preview [`ImageFormat`].
pub(super) fn image_format_for_ext(path: &Path) -> Option<ImageFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "webp" => Some(ImageFormat::Webp),
        "gif" => Some(ImageFormat::Gif),
        "bmp" => Some(ImageFormat::Bmp),
        "tif" | "tiff" => Some(ImageFormat::Tiff),
        _ => None,
    }
}

/// File extension used when naming a pasted clipboard image (`image-N.<ext>`).
pub(super) fn clipboard_image_ext(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
        ImageFormat::Ico => "ico",
        ImageFormat::Pnm => "pnm",
    }
}

/// Format the user-facing warning when one or more files exceed `MAX_UPLOAD_BYTES`.
pub(super) fn format_upload_rejection(rejected: &[String]) -> Option<String> {
    if rejected.is_empty() {
        return None;
    }
    Some(format!(
        "Skipped {} (exceeds Slack's 1 GB per-file upload limit).",
        rejected.join(", ")
    ))
}

/// Read on-disk `paths` enforcing `max_bytes`, returning `(staged, rejected_filenames)`.
/// Directories, unreadable files, and empty files are silently skipped.
pub(super) fn load_attachment_paths(
    paths: Vec<PathBuf>,
    max_bytes: u64,
) -> (Vec<PendingAttachment>, Vec<String>) {
    let mut out = Vec::new();
    let mut too_big = Vec::new();
    for path in paths {
        if !path.is_file() {
            continue;
        }
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_owned();
        if let Ok(meta) = std::fs::metadata(&path)
            && meta.len() > max_bytes
        {
            too_big.push(filename);
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if bytes.is_empty() {
            continue;
        }
        let preview = image_format_for_ext(&path)
            .map(|format| Arc::new(Image::from_bytes(format, bytes.clone())));
        out.push(PendingAttachment::new(filename, bytes, preview));
    }
    (out, too_big)
}

impl Composer {
    /// Track `+` dropdown open state (`+` -> `×`) and trigger direct file upload on double-click,
    /// matching Slack's `"Tip: double click to upload"`.
    pub(super) fn on_plus_open_change(&mut self, open: bool, cx: &mut Context<Self>) {
        self.plus_open = open;
        let now = Instant::now();
        if !open
            && self
                .plus_last_click
                .is_some_and(|prev| now.duration_since(prev) <= PLUS_DOUBLE_CLICK_WINDOW)
        {
            self.plus_last_click = None;
            self.pick_files(cx);
        } else if open {
            self.plus_last_click = Some(now);
        }
        cx.notify();
    }

    /// Open the OS file picker and stage every chosen file as a pending attachment.
    pub(super) fn pick_files(&mut self, cx: &mut Context<Self>) {
        if self.edit_mode {
            return;
        }
        let draft_gen = self.draft_gen;
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach files".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let _ = this.update(cx, |composer, cx| {
                if composer.draft_gen != draft_gen || composer.edit_mode {
                    return;
                }
                composer.attach_paths(paths, cx);
            });
        })
        .detach();
    }

    /// Read on-disk files off the UI thread and stage them in `self.attachments`.
    pub(crate) fn attach_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.edit_mode || paths.is_empty() {
            return;
        }
        let draft_gen = self.draft_gen;
        self.upload_warning = None;
        cx.spawn(async move |this, cx| {
            let (loaded, rejected) = cx
                .background_executor()
                .spawn(async move { load_attachment_paths(paths, MAX_UPLOAD_BYTES) })
                .await;
            if loaded.is_empty() && rejected.is_empty() {
                return;
            }
            let _ = this.update(cx, |composer, cx| {
                if composer.draft_gen != draft_gen || composer.edit_mode {
                    return;
                }
                composer.attachments.extend(loaded);
                if let Some(warning) = format_upload_rejection(&rejected) {
                    composer.upload_warning = Some(warning);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Stage an image pasted from the clipboard (`Cmd+V`).
    pub(super) fn attach_clipboard_image(&mut self, image: Image, cx: &mut Context<Self>) {
        if self.edit_mode || image.bytes.is_empty() {
            return;
        }
        let ext = clipboard_image_ext(image.format);
        let n = self.next_paste_id;
        self.next_paste_id += 1;
        let bytes = image.bytes.clone();
        self.attachments.push(PendingAttachment::new(
            format!("image-{n}.{ext}"),
            bytes,
            Some(Arc::new(image)),
        ));
        cx.notify();
    }

    /// Stage `files` again after a canceled send put them back.
    pub(crate) fn restore_attachments(
        &mut self,
        files: Vec<PendingAttachment>,
        cx: &mut Context<Self>,
    ) {
        self.attachments = files;
        cx.notify();
    }

    pub(super) fn remove_attachment(&mut self, id: u64, cx: &mut Context<Self>) {
        let before = self.attachments.len();
        self.attachments.retain(|att| att.id != id);
        if self.attachments.len() != before {
            cx.notify();
        }
    }
}
