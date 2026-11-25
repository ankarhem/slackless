//! `files` listing, `download`, and `transcribe` commands.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use slackless_core::api::FilesFilter;
use slackless_core::huddle::{TranscriptionProvider, resolve_local_file_provider};
use slackless_core::types::{File, FileId, TeamId};

use crate::archive::Archive;
use crate::output;
use crate::{Ctx, fail};

use super::session::{api_fail, api_for, resolve_channel};

static TRANSCRIBE_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

pub(crate) fn clamp_files_limit(limit: u32) -> u32 {
    limit.clamp(1, 200)
}

pub(crate) fn format_files_list(files: &[File]) -> (serde_json::Value, String) {
    let json_files: Vec<serde_json::Value> = files
        .iter()
        .map(|f| {
            serde_json::json!({
                "id": f.id.as_str(),
                "name": f.name,
                "title": f.title,
                "filetype": f.filetype,
                "size": f.size,
                "user": f.user.as_ref().map(|u| u.as_str()),
                "permalink": f.permalink,
            })
        })
        .collect();
    let mut human = String::new();
    for f in files {
        let name = f
            .title
            .as_deref()
            .or(f.name.as_deref())
            .unwrap_or(f.id.as_str());
        let kind = f.filetype.as_deref().unwrap_or("file");
        let size = f.size.map_or_else(|| "-".to_owned(), |s| s.to_string());
        human.push_str(&format!("{}\t{kind}\t{size}\t{name}\n", f.id));
    }
    let json = serde_json::json!({ "count": files.len(), "files": json_files });
    (json, human)
}

/// `files [--channel <ch>] [--types <csv>] [--limit N]`
pub async fn run_files(
    ctx: &Ctx,
    flag: Option<&str>,
    channel_arg: Option<&str>,
    types: Option<String>,
    limit: u32,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let channel = channel_arg.map(|ch| resolve_channel(&session.workspace, ch).0);
    let filter = FilesFilter {
        channel,
        types,
        ..FilesFilter::workspace()
    };
    let page = match session
        .api
        .files_list(&filter, clamp_files_limit(limit), 1)
        .await
    {
        Ok(page) => page,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let (json, human) = format_files_list(&page.items);
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}

/// Select the preferred authenticated download URL for a Slack file.
pub(crate) fn select_download_url(file: &File) -> Option<&str> {
    file.url_private_download
        .as_deref()
        .or(file.url_private.as_deref())
}

/// Sanitize a Slack file's display/file name into a single safe filename component.
pub(crate) fn sanitize_download_name(
    file_id: &str,
    name: Option<&str>,
    title: Option<&str>,
) -> String {
    let fallback_name = name.or(title).unwrap_or(file_id);
    Path::new(fallback_name)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("{file_id}.bin"))
}

/// Compute `(safe_name, destination_path)` for `download`.
pub(crate) fn resolve_download_dest(
    file_id: &str,
    file: &File,
    out: Option<&Path>,
) -> (String, PathBuf) {
    let safe_name = sanitize_download_name(file_id, file.name.as_deref(), file.title.as_deref());
    let dest = out
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&safe_name));
    (safe_name, dest)
}

/// `download <FILE_ID> [--out PATH]`
pub async fn run_download(
    ctx: &Ctx,
    flag: Option<&str>,
    file_id: &str,
    out: Option<&Path>,
) -> Result<ExitCode> {
    let session = match api_for(ctx, flag) {
        Ok(s) => s,
        Err(exit) => return Ok(exit),
    };
    let file = match session.api.files_info(file_id).await {
        Ok(f) => f,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let Some(url) = select_download_url(&file) else {
        return Ok(fail(
            ctx,
            "no_download_url",
            "file has no downloadable private URL",
        ));
    };
    let bytes = match session.api.download_file(url).await {
        Ok(b) => b,
        Err(err) => return Ok(api_fail(ctx, &err)),
    };
    let (safe_name, dest) = resolve_download_dest(file_id, &file, out);
    if let Err(err) = std::fs::write(&dest, &bytes) {
        return Ok(fail(
            ctx,
            "write_failed",
            &format!("writing {}: {err}", dest.display()),
        ));
    }
    output::emit(
        ctx.json,
        &serde_json::json!({
            "ok": true,
            "file_id": file_id,
            "name": safe_name,
            "bytes": bytes.len(),
            "path": dest.display().to_string(),
        }),
        &format!("downloaded {} ({} bytes)\n", dest.display(), bytes.len()),
    );
    Ok(ExitCode::SUCCESS)
}

/// Parse `--provider` for `slackless transcribe` into a preferred local [`TranscriptionProvider`].
pub(crate) fn parse_transcribe_provider(
    raw: Option<&str>,
) -> std::result::Result<TranscriptionProvider, String> {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") | Some("auto") | Some("parakeet") | Some("parakeet-v3") => {
            Ok(TranscriptionProvider::ParakeetV3)
        }
        Some("whisper") | Some("whisper-turbo") => Ok(TranscriptionProvider::WhisperTurbo),
        Some(other) => Err(format!(
            "unknown --provider '{other}' (expected parakeet or whisper)"
        )),
    }
}

/// Select the preferred authenticated media URL for transcribing a Slack video/audio file:
/// `extra["mp4"]` -> `url_private` -> `url_private_download`.
pub(crate) fn select_transcribe_media_url(file: &File) -> Option<&str> {
    file.extra
        .get("mp4")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            file.url_private
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            file.url_private_download
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
}

fn clean_media_ext(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_start_matches('.');
    if !trimmed.is_empty()
        && trimmed.len() <= 8
        && trimmed.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        Some(trimmed.to_ascii_lowercase())
    } else {
        None
    }
}

/// Infer a safe lowercase file extension for a downloaded Slack media file so native decoders
/// (e.g. `AVAssetReader`) recognize its container format.
pub(crate) fn infer_media_extension(file: &File, selected_url: &str) -> String {
    if file
        .extra
        .get("mp4")
        .and_then(|v| v.as_str())
        .map(str::trim)
        == Some(selected_url)
    {
        return "mp4".to_owned();
    }
    if let Some(ext) = file
        .name
        .as_deref()
        .or(file.title.as_deref())
        .and_then(|n| Path::new(n).extension())
        .and_then(|e| e.to_str())
        .and_then(clean_media_ext)
    {
        return ext;
    }
    if let Some(ext) = file.filetype.as_deref().and_then(clean_media_ext) {
        return ext;
    }
    let url_path = selected_url
        .split(['?', '#'])
        .next()
        .unwrap_or(selected_url);
    if let Some(ext) = Path::new(url_path)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(clean_media_ext)
    {
        return ext;
    }
    "mp4".to_owned()
}

/// Format JSON and human output for `slackless transcribe`.
pub(crate) fn format_transcribe_result(
    file_id: &str,
    provider: TranscriptionProvider,
    indexed_messages: usize,
    text: &str,
) -> (serde_json::Value, String) {
    let provider_tag = provider.source_tag();
    let json = serde_json::json!({
        "ok": true,
        "file_id": file_id,
        "provider": provider_tag,
        "indexed_messages": indexed_messages,
        "text": text,
    });
    let human = format!(
        "transcribed {file_id} ({provider_tag}, indexed {indexed_messages} message(s)):\n{text}\n"
    );
    (json, human)
}

struct TempFileGuard(PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `transcribe <FILE_ID> [--file PATH] [--provider parakeet|whisper] [--workspace TEAM_ID]`
pub async fn run_transcribe(
    ctx: &Ctx,
    flag: Option<&str>,
    workspace: Option<&TeamId>,
    file_id: &str,
    local_file: Option<&Path>,
    provider_arg: Option<&str>,
) -> Result<ExitCode> {
    let file_id = file_id.trim();
    if file_id.is_empty() {
        return Ok(fail(ctx, "invalid_args", "file_id cannot be empty"));
    }
    let preferred = match parse_transcribe_provider(provider_arg) {
        Ok(p) => p,
        Err(msg) => return Ok(fail(ctx, "invalid_args", &msg)),
    };

    let (target_workspace, media_path, _temp_guard): (TeamId, PathBuf, Option<TempFileGuard>) =
        if let Some(local_path) = local_file {
            if !local_path.is_file() {
                return Ok(fail(
                    ctx,
                    "file_not_found",
                    &format!("local file not found: {}", local_path.display()),
                ));
            }
            let Some(ws) = workspace else {
                return Ok(fail(
                    ctx,
                    "no_workspace",
                    "no workspace known for transcribing; pass --workspace TEAM_ID (or run `slackless auth login` first)",
                ));
            };
            (ws.clone(), local_path.to_path_buf(), None)
        } else {
            let session = match api_for(ctx, flag) {
                Ok(s) => s,
                Err(exit) => return Ok(exit),
            };
            if resolve_local_file_provider(preferred).is_none() {
                return Ok(fail(
                    ctx,
                    "transcribe_failed",
                    "No local transcription model is downloaded yet. Download Parakeet v3 or Whisper Turbo in Preferences → Audio & Video.",
                ));
            }
            let file = match session.api.files_info(file_id).await {
                Ok(f) => f,
                Err(err) => return Ok(api_fail(ctx, &err)),
            };
            let Some(url) = select_transcribe_media_url(&file) else {
                return Ok(fail(
                    ctx,
                    "no_media_url",
                    "file has no downloadable video/audio URL",
                ));
            };
            let bytes = match session.api.download_file(url).await {
                Ok(b) => b,
                Err(err) => return Ok(api_fail(ctx, &err)),
            };
            let ext = infer_media_extension(&file, url);
            let safe_id: String = file_id
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            let seq = TRANSCRIBE_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let tmp_path = std::env::temp_dir().join(format!(
                "slackless-transcribe-{}-{}-{seq}.{ext}",
                std::process::id(),
                if safe_id.is_empty() { "file" } else { &safe_id },
            ));
            if let Err(err) = std::fs::write(&tmp_path, &bytes) {
                return Ok(fail(
                    ctx,
                    "write_failed",
                    &format!("writing temporary media file {}: {err}", tmp_path.display()),
                ));
            }
            (
                session.workspace,
                tmp_path.clone(),
                Some(TempFileGuard(tmp_path)),
            )
        };

    let mut archive = Archive::open_or_create_rw(&target_workspace)?;
    let fid = FileId::from(file_id);
    let (used_provider, text, indexed_messages) = match slackless_core::transcribe_and_index_file(
        archive.store_mut(),
        &target_workspace,
        &fid,
        &media_path,
        preferred,
    ) {
        Ok(res) => res,
        Err(err) => return Ok(fail(ctx, "transcribe_failed", &err)),
    };

    let (json, human) = format_transcribe_result(file_id, used_provider, indexed_messages, &text);
    output::emit(ctx.json, &json, &human);
    Ok(ExitCode::SUCCESS)
}
