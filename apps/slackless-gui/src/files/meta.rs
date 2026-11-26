//! Pure file attachment classification, sizing, formatting, transcript, and snippet metadata seams.

use gpui_kit::assets::IconName;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::gpui::{App, Hsla};
use slackless_core::types::{File, FileMode};

/// Largest rendered thumbnail box (px); images scale down to fit, keeping their aspect.
pub(super) const MAX_THUMB_W: f32 = 360.;
pub(super) const MAX_THUMB_H: f32 = 240.;
pub(super) const CARD_W: f32 = 340.;
/// Inline preview threshold when the user disabled "show large files" (2 MiB).
pub(super) const INLINE_MAX_BYTES: i64 = 2 * 1024 * 1024;

pub(super) const SNIPPET_COLLAPSED_LINES: usize = 5;
/// Maximum lines rendered inline when a snippet is expanded: high enough to show full files like
/// Slack does inline (e.g. 252-line configs) while keeping multi-thousand-line files responsive.
pub(super) const SNIPPET_MAX_EXPANDED_LINES: usize = 1_000;

/// Whether `file` is eligible for inline image/gallery rendering under `show_large`.
pub(super) fn inline_allowed(file: &File, show_large: bool) -> bool {
    show_large || file.size.unwrap_or(0) <= INLINE_MAX_BYTES
}

/// Where a file's original downloads from: Slack's download link, else its private url.
pub(crate) fn download_url(file: &File) -> Option<String> {
    file.url_private_download
        .clone()
        .or_else(|| file.url_private.clone())
}

/// Returns `false` for media container/format URLs that GPUI's `image` + `usvg` loader cannot
/// decode as a static or animated image (such as `.mp4` `thumb_video` clips or `.heic` originals),
/// preventing noisy `gpui::asset_cache` decode errors and allowing clean fallback to JPEG/PNG
/// thumbnails.
pub(crate) fn is_decodable_image_url(url: &str) -> bool {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    !matches!(
        path.rsplit_once('.').map(|(_, ext)| ext),
        Some(
            "mp4"
                | "mov"
                | "webm"
                | "mkv"
                | "avi"
                | "m4v"
                | "mp3"
                | "m4a"
                | "wav"
                | "aac"
                | "ogg"
                | "flac"
                | "opus"
                | "heic"
                | "heif"
                | "pdf"
        )
    )
}

/// Static poster image URL for a video card, skipping non-image `thumb_video` `.mp4` URLs.
pub(crate) fn video_poster_url(file: &File) -> Option<String> {
    [
        file.thumb_video.as_deref(),
        file.thumb_480.as_deref(),
        file.thumb_360.as_deref(),
        file.thumb_720.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find(|url| is_decodable_image_url(url))
    .map(str::to_owned)
}

/// Direct playback URL for a Slack video file (`extra["mp4"]` -> `url_private` ->
/// `url_private_download` -> `extra["vjs_hls"]`).
pub(crate) fn video_playback_url(file: &File) -> Option<String> {
    file.extra
        .get("mp4")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| file.url_private.clone())
        .or_else(|| file.url_private_download.clone())
        .or_else(|| {
            file.extra
                .get("vjs_hls")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
}

/// A live image's thumbnail URL, sized for the inline preview on a 2x display; for a GIF its
/// animated rendition, which Slack plays inline.
pub(super) fn remote_thumbnail(file: &File) -> Option<String> {
    if !file
        .mimetype
        .as_deref()
        .is_some_and(|mime| mime.starts_with("image/"))
    {
        return None;
    }
    [
        file.thumb_480_gif.as_deref(),
        file.thumb_360_gif.as_deref(),
        file.thumb_480.as_deref(),
        file.thumb_360.as_deref(),
        file.thumb_720.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find(|url| is_decodable_image_url(url))
    .map(str::to_owned)
}

pub(super) fn is_audio(file: &File) -> bool {
    if file
        .mimetype
        .as_deref()
        .is_some_and(|m| m.starts_with("audio/"))
    {
        return true;
    }
    matches!(
        file.filetype.as_deref().unwrap_or_default(),
        "mp3" | "m4a" | "wav" | "aac" | "ogg" | "flac" | "opus"
    )
}

pub(crate) fn is_video(file: &File) -> bool {
    if file
        .mimetype
        .as_deref()
        .is_some_and(|m| m.starts_with("video/"))
    {
        return true;
    }
    matches!(
        file.filetype.as_deref().unwrap_or_default(),
        "mp4" | "mov" | "webm" | "mkv" | "avi" | "m4v"
    )
}

pub(crate) fn is_pdf(file: &File) -> bool {
    if file
        .mimetype
        .as_deref()
        .is_some_and(|m| m.eq_ignore_ascii_case("application/pdf"))
    {
        return true;
    }
    if file
        .filetype
        .as_deref()
        .is_some_and(|t| t.eq_ignore_ascii_case("pdf"))
    {
        return true;
    }
    file.name
        .as_deref()
        .is_some_and(|n| n.to_ascii_lowercase().ends_with(".pdf"))
}

/// Static preview thumbnail URL for a PDF file when Slack provides a rasterized PNG/JPG thumb.
pub(crate) fn pdf_thumb_url(file: &File) -> Option<String> {
    [
        file.thumb_pdf.as_deref(),
        file.thumb_480.as_deref(),
        file.thumb_360.as_deref(),
        file.thumb_720.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find(|url| is_decodable_image_url(url))
    .map(str::to_owned)
}

pub(super) fn is_snippet(file: &File) -> bool {
    let has_preview = file
        .preview
        .as_deref()
        .is_some_and(|p| !p.trim().is_empty());
    has_preview
        && (file.mode == Some(FileMode::Snippet)
            || (!is_audio(file)
                && !is_video(file)
                && remote_thumbnail(file).is_none()
                && !matches!(file.filetype.as_deref(), Some("quip" | "list" | "pdf"))))
}

pub(crate) fn media_duration_ms(file: &File) -> Option<u64> {
    file.extra
        .get("duration_ms")
        .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
        .filter(|&ms| ms > 0)
}

pub(super) use slackless_core::types::time::format_duration;

pub(crate) fn file_ext(file: &File, default: &str) -> String {
    if let Some(ft) = file.filetype.as_deref().filter(|s| !s.is_empty()) {
        return ft.to_owned();
    }
    file.name
        .as_deref()
        .and_then(|n| n.rsplit_once('.').map(|(_, ext)| ext.to_owned()))
        .unwrap_or_else(|| default.to_owned())
}

pub(super) fn file_name(file: &File) -> String {
    file.name
        .clone()
        .or_else(|| file.title.clone())
        .unwrap_or_else(|| file.id.to_string())
}

pub(crate) use slackless_core::domain::webvtt::{
    clip_vtt_url, inline_clip_transcript, parse_webvtt, parse_webvtt_cues,
};

/// Normalized waveform samples (`0.12..=1.0`) from Slack's `audio_wave_samples` metadata.
pub(super) fn audio_wave_bars(file: &File) -> Vec<f32> {
    let Some(arr) = file
        .extra
        .get("audio_wave_samples")
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    let raw: Vec<f32> = arr
        .iter()
        .filter_map(|v| v.as_f64().map(|n| n as f32))
        .collect();
    if raw.is_empty() {
        return Vec::new();
    }
    let max = raw.iter().copied().fold(1.0f32, f32::max);
    let target = 28usize.min(raw.len());
    let step = (raw.len() as f32 / target as f32).max(1.0);
    (0..target)
        .map(|i| {
            let ix = ((i as f32 * step) as usize).min(raw.len() - 1);
            (raw[ix] / max).clamp(0.12, 1.0)
        })
        .collect()
}

/// `(visible_count, omitted_lines_when_expanded)` for a snippet card.
pub(super) fn snippet_visible_and_omitted(total_loaded: usize, expanded: bool) -> (usize, usize) {
    if !expanded {
        return (total_loaded.min(SNIPPET_COLLAPSED_LINES), 0);
    }
    let visible = total_loaded.min(SNIPPET_MAX_EXPANDED_LINES);
    (visible, total_loaded.saturating_sub(visible))
}

pub(super) fn extra_usize(file: &File, key: &str) -> Option<usize> {
    let val = file.extra.get(key)?;
    val.as_u64()
        .map(|n| n as usize)
        .or_else(|| val.as_str().and_then(|s| s.parse::<usize>().ok()))
}

pub(super) fn text_lines(text: &str) -> Vec<String> {
    text.trim_end_matches(['\n', '\r'])
        .lines()
        .map(str::to_owned)
        .collect()
}

pub(super) fn snippet_lines(file: &File) -> Vec<String> {
    text_lines(file.preview.as_deref().unwrap_or_default())
}

/// Whether Slack truncated `file.preview` on the server (typically to 5 lines), so expanding or
/// copying needs to fetch the full file text from `url_private`.
pub(super) fn snippet_is_truncated(file: &File, preview_len: usize) -> bool {
    let flagged = file
        .extra
        .get("preview_is_truncated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let lines_more = extra_usize(file, "lines_more").unwrap_or(0);
    let server_total = extra_usize(file, "lines").unwrap_or(0);
    flagged || lines_more > 0 || server_total > preview_len
}

/// Extra lines beyond the collapsed 5-line preview: uses Slack's `lines` / `lines_more` metadata
/// when available so truncated previews report the real line count (`Show more (247 lines)`).
pub(super) fn snippet_hidden_lines(file: &File, loaded_len: usize, has_full_text: bool) -> usize {
    let from_loaded = loaded_len.saturating_sub(SNIPPET_COLLAPSED_LINES);
    if has_full_text {
        return from_loaded;
    }
    let from_total = extra_usize(file, "lines")
        .unwrap_or(0)
        .saturating_sub(SNIPPET_COLLAPSED_LINES);
    let from_more = extra_usize(file, "lines_more").unwrap_or(0);
    from_loaded.max(from_total).max(from_more)
}

/// Human-readable provider label for an external Slack file (`F-COLLAB-586`).
pub(super) fn external_provider_label(file: &File) -> String {
    match file
        .external_type
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "gdrive" => "Google Drive".to_owned(),
        "dropbox" => "Dropbox".to_owned(),
        "box" => "Box".to_owned(),
        "onedrive" | "sharepoint" => "OneDrive".to_owned(),
        "notion" => "Notion".to_owned(),
        "figma" => "Figma".to_owned(),
        "jira" => "Jira".to_owned(),
        "github" => "GitHub".to_owned(),
        other if !other.is_empty() => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => "External file".to_owned(),
            }
        }
        _ => "External file".to_owned(),
    }
}

/// Subtitle for an external file card (`Provider · Pretty Type`, omitting duplicate type labels).
pub(super) fn external_subtitle(file: &File) -> String {
    let provider = external_provider_label(file);
    let kind = file
        .pretty_type
        .clone()
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case(&provider));
    match kind {
        Some(k) => format!("{provider} \u{b7} {k}"),
        None => provider,
    }
}

/// Semantic tile accent bucket for [`file_icon`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum FileTileTone {
    Danger,
    Success,
    Warning,
    Link,
}

/// Pure file type -> `(IconName, FileTileTone)` classification.
pub(super) fn file_icon_kind(file: &File) -> (IconName, FileTileTone) {
    match file.filetype.as_deref().unwrap_or_default() {
        "pdf" => (IconName::FileText, FileTileTone::Danger),
        "xlsx" | "xls" | "csv" | "gsheet" => (IconName::FileSpreadsheet, FileTileTone::Success),
        "zip" | "gz" | "tar" => (IconName::FileArchive, FileTileTone::Warning),
        "png" | "jpg" | "jpeg" | "gif" | "webp" => (IconName::FileImage, FileTileTone::Link),
        "mp4" | "mov" | "webm" | "mkv" | "avi" | "m4v" => (IconName::Video, FileTileTone::Link),
        "mp3" | "m4a" | "wav" | "aac" | "ogg" | "flac" | "opus" => {
            (IconName::Mic, FileTileTone::Link)
        }
        // Slack's canvases are `quip` files; its lists are `list` files.
        "quip" => (IconName::StickyNote, FileTileTone::Link),
        "list" => (IconName::Table2, FileTileTone::Link),
        _ => (IconName::File, FileTileTone::Link),
    }
}

/// Type icon and tile color: PDFs red, spreadsheets green, images, canvases, lists and the
/// rest blue.
pub(crate) fn file_icon(file: &File, cx: &App) -> (IconName, Hsla) {
    let theme = cx.theme();
    let (icon, tone) = file_icon_kind(file);
    let color = match tone {
        FileTileTone::Danger => theme.danger,
        FileTileTone::Success => theme.success,
        FileTileTone::Warning => theme.warning,
        FileTileTone::Link => theme.link,
    };
    (icon, color)
}

fn extra_i64(file: &File, key: &str) -> Option<i64> {
    let val = file.extra.get(key)?;
    val.as_i64()
        .or_else(|| val.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| val.as_f64().filter(|f| f.is_finite()).map(|f| f as i64))
        .or_else(|| val.as_str().and_then(|s| s.parse::<i64>().ok()))
}

fn positive_dims(w: Option<i64>, h: Option<i64>) -> Option<(i64, i64)> {
    match (w, h) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
        _ => None,
    }
}

/// Best available `(width, height)` metadata for a video file, falling back from `original_w/h`
/// (honoring 90°/270° `image_exif_rotation`) through `thumb_video_w/h` and Slack's thumbnail
/// derivative dimensions.
pub(crate) fn video_dimensions(file: &File) -> Option<(i64, i64)> {
    let original = positive_dims(file.original_w, file.original_h).map(|(w, h)| {
        let swapped = matches!(extra_i64(file, "image_exif_rotation"), Some(5..=8));
        if swapped { (h, w) } else { (w, h) }
    });
    original
        .or_else(|| {
            positive_dims(
                extra_i64(file, "thumb_video_w"),
                extra_i64(file, "thumb_video_h"),
            )
        })
        .or_else(|| {
            positive_dims(
                extra_i64(file, "thumb_1024_w"),
                extra_i64(file, "thumb_1024_h"),
            )
        })
        .or_else(|| {
            positive_dims(
                extra_i64(file, "thumb_960_w"),
                extra_i64(file, "thumb_960_h"),
            )
        })
        .or_else(|| {
            positive_dims(
                extra_i64(file, "thumb_800_w"),
                extra_i64(file, "thumb_800_h"),
            )
        })
        .or_else(|| {
            positive_dims(
                extra_i64(file, "thumb_720_w"),
                extra_i64(file, "thumb_720_h"),
            )
        })
        .or_else(|| positive_dims(file.thumb_480_w, file.thumb_480_h))
        .or_else(|| positive_dims(file.thumb_360_w, file.thumb_360_h))
}

/// Compute the rendered inline video card `(w, h)` from live player dimensions, Slack file
/// metadata dimensions, and/or decoded poster image dimensions while preserving aspect ratio.
pub(crate) fn resolve_video_preview_size(
    meta_dims: Option<(i64, i64)>,
    poster_dims: Option<(i64, i64)>,
    snap_dims: Option<(u32, u32)>,
) -> (f32, f32) {
    if let Some((sw, sh)) = snap_dims.filter(|&(w, h)| w > 0 && h > 0) {
        return fit(Some(i64::from(sw)), Some(i64::from(sh)));
    }
    let chosen = match (meta_dims, poster_dims) {
        (Some((mw, mh)), Some((pw, ph))) if (mw > mh) != (pw > ph) => Some((pw, ph)),
        (Some(m), _) => Some(m),
        (None, Some(p)) => Some(p),
        (None, None) => None,
    };
    match chosen {
        Some((w, h)) => fit(Some(w), Some(h)),
        None => fit(None, None),
    }
}

/// Scale `(w, h)` into the thumbnail box, keeping the aspect ratio; smaller images keep
/// their size.
pub(crate) fn fit(width: Option<i64>, height: Option<i64>) -> (f32, f32) {
    let (w, h) = match (width, height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w as f32, h as f32),
        _ => (MAX_THUMB_W, MAX_THUMB_H),
    };
    let scale = (MAX_THUMB_W / w).min(MAX_THUMB_H / h).min(1.);
    (w * scale, h * scale)
}

/// Bytes formatted in binary IEC units (`0 B`, `512 B`, `1.0 KiB`, `2.3 MiB`).
pub(crate) fn human_size(bytes: i64) -> String {
    slackless_core::types::time::format_bytes(bytes.max(0) as u64)
}
