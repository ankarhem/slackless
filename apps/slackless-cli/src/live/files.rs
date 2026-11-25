//! `files` listing and `download` live commands.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use slackless_core::api::FilesFilter;
use slackless_core::types::File;

use crate::output;
use crate::{Ctx, fail};

use super::session::{api_fail, api_for, resolve_channel};

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
    let (api, workspace) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let channel = channel_arg.map(|ch| resolve_channel(&workspace, ch).0);
    let filter = FilesFilter {
        channel,
        types,
        ..FilesFilter::workspace()
    };
    let page = match api.files_list(&filter, clamp_files_limit(limit), 1).await {
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
    let (api, _) = match api_for(ctx, flag) {
        Ok(pair) => pair,
        Err(exit) => return Ok(exit),
    };
    let file = match api.files_info(file_id).await {
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
    let bytes = match api.download_file(url).await {
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
