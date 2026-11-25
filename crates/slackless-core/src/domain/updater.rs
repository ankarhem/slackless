//! Shared GitHub Releases update checker, version comparator, checksum parser, and install-path
//! resolver used by both CLI (`slackless update`) and GUI (`Check for Updates`).

use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// GitHub repository hosting official Slackless releases.
pub const RELEASES_REPO: &str = "ankarhem/slackless";

/// GitHub REST endpoint for the latest official Slackless release.
pub const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/ankarhem/slackless/releases/latest";

const DEFAULT_USER_AGENT: &str = concat!("Slackless/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const METADATA_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Metadata for an available GitHub Release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub version: String,
    pub html_url: String,
    pub asset_name: String,
    pub download_url: String,
    pub sha256_url: Option<String>,
    /// Concise bullet highlights parsed from the GitHub Release body.
    pub notes: Vec<String>,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// Parse a `YYYY.MM.DD-xx`, `YYYY.MM.DD.xx`, or `v?major.minor.patch` tag into a comparable
/// 4-tuple `(a, b, c, seq)`. Non-numeric prerelease suffixes like `-rc1` or `-dev` return `None`
/// unless `seq` is a pure unsigned integer, preventing accidental downgrades/upgrades against
/// prerelease tags.
pub fn parse_version(raw: &str) -> Option<(u32, u32, u32, u32)> {
    let s = raw.trim().trim_start_matches('v');
    if s.is_empty() {
        return None;
    }
    let (base, seq) = match s.split_once('-') {
        Some((lhs, rhs)) => {
            let seq = rhs.parse::<u32>().ok()?;
            (lhs, Some(seq))
        }
        None => (s, None),
    };
    let nums: Vec<u32> = base
        .split('.')
        .map(|part| part.parse::<u32>().ok())
        .collect::<Option<Vec<_>>>()?;
    match (nums.as_slice(), seq) {
        (&[y, m, d], Some(n)) => Some((y, m, d, n)),
        (&[y, m, d, n], None) => Some((y, m, d, n)),
        (&[maj, min, patch], None) => Some((maj, min, patch, 0)),
        _ => None,
    }
}

/// Returns `true` if `latest` is strictly newer than `current`.
pub fn is_newer_version(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => {
            let l = latest.trim();
            let c = current.trim();
            !l.is_empty() && l != c
        }
    }
}

/// Returns `true` when `version` represents a local development build (`0.1.0` or `*-dev`).
pub fn is_dev_version(version: &str) -> bool {
    version == "0.1.0" || version.ends_with("-dev") || version.contains("-dev.")
}

/// Extract up to 8 concise bullet highlights from a GitHub Release markdown body, skipping
/// install/changelog boilerplate and stripping trailing `(`abc1234`)` commit hashes.
pub fn parse_release_highlights(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_install_section = false;
    for raw_line in body.lines() {
        let line = raw_line.trim();
        if line.starts_with('#') {
            let lower = line.to_ascii_lowercase();
            in_install_section = lower.contains("install") || lower.contains("update (macos");
            continue;
        }
        if in_install_section {
            continue;
        }
        let Some(item) = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .map(str::trim)
        else {
            continue;
        };
        if item.is_empty() {
            continue;
        }
        let cleaned = match item.rsplit_once(" (`") {
            Some((prefix, tail)) if tail.ends_with("`)") && tail.len() <= 16 => prefix.trim(),
            _ => item,
        };
        if !cleaned.is_empty() {
            out.push(cleaned.to_owned());
            if out.len() >= 8 {
                break;
            }
        }
    }
    out
}

/// Parse a GitHub Releases JSON payload into [`ReleaseInfo`]. Draft and prerelease payloads return
/// `Ok(None)` so neither CLI nor GUI accidentally installs an unpublished build.
pub fn parse_github_release(payload: &[u8]) -> Result<Option<ReleaseInfo>, String> {
    let rel: GhRelease =
        serde_json::from_slice(payload).map_err(|e| format!("invalid GitHub release JSON: {e}"))?;
    if rel.draft || rel.prerelease {
        return Ok(None);
    }
    let expected_name = format!("Slackless-{}-macos-arm64.app.tar.gz", rel.tag_name);
    let asset = rel
        .assets
        .iter()
        .find(|a| a.name == expected_name)
        .or_else(|| {
            rel.assets
                .iter()
                .find(|a| a.name.ends_with("-macos-arm64.app.tar.gz"))
        })
        .ok_or_else(|| format!("release {} has no macOS arm64 app archive", rel.tag_name))?;

    let sha256_url = rel
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS.txt")
        .map(|a| a.browser_download_url.clone());

    let notes = rel
        .body
        .as_deref()
        .map(parse_release_highlights)
        .unwrap_or_default();

    Ok(Some(ReleaseInfo {
        version: rel.tag_name,
        html_url: rel.html_url,
        asset_name: asset.name.clone(),
        download_url: asset.browser_download_url.clone(),
        sha256_url,
        notes,
    }))
}

fn encode_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Compute the lowercase hex SHA-256 digest of `bytes` in pure Rust.
pub fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    encode_lower_hex(&digest)
}

/// Compute the lowercase hex SHA-256 digest of the file at `path` in pure Rust, streaming in
/// 64 KiB chunks so large release archives do not require full-file buffering.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("failed to open {} for SHA-256: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("failed to read {} for SHA-256: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(encode_lower_hex(&hasher.finalize()))
}

fn build_updater_client(user_agent: &str, timeout: Duration) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(timeout)
        .user_agent(user_agent);
    if let Some(proxy_str) = crate::api::proxy_url()
        && let Ok(proxy) = reqwest::Proxy::all(&proxy_str)
    {
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|e| format!("failed to build updater HTTP client: {e}"))
}

async fn fetch_release_from_url(
    url: &str,
    user_agent: &str,
) -> Result<Option<ReleaseInfo>, String> {
    let client = build_updater_client(user_agent, METADATA_TIMEOUT)?;
    let resp = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("GitHub Releases check failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!(
            "GitHub Releases API returned HTTP {}",
            status.as_u16()
        ));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("failed reading GitHub Releases response: {e}"))?;
    parse_github_release(&bytes)
}

/// Query GitHub Releases for `repo` (e.g. `"ankarhem/slackless"`) over HTTPS using `reqwest`
/// (15s connect / 30s request timeout, `Accept: application/vnd.github+json`), returning
/// `Ok(None)` for drafts/prereleases.
pub async fn fetch_latest_release_for(
    repo: &str,
    user_agent: &str,
) -> Result<Option<ReleaseInfo>, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    fetch_release_from_url(&url, user_agent).await
}

fn block_on_updater<F: std::future::Future>(fut: F) -> Result<F::Output, String> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return Ok(tokio::task::block_in_place(|| handle.block_on(fut)));
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to initialize updater async runtime: {e}"))?;
    Ok(rt.block_on(fut))
}

/// Blocking wrapper around [`fetch_latest_release_for`] for non-Tokio callers (e.g. GPUI
/// background executor threads), running `reqwest` on a lightweight temporary Tokio runtime.
pub fn fetch_latest_release_for_blocking(
    repo: &str,
    user_agent: &str,
) -> Result<Option<ReleaseInfo>, String> {
    block_on_updater(fetch_latest_release_for(repo, user_agent))?
}

/// Query GitHub Releases over HTTPS (using `reqwest` with workspace proxy support and timeouts)
/// for the latest non-draft, non-prerelease official Slackless release.
pub async fn fetch_latest_release() -> Result<Option<ReleaseInfo>, String> {
    fetch_latest_release_for(RELEASES_REPO, DEFAULT_USER_AGENT).await
}

/// Stream `url` directly to `dest` via `reqwest` (15s connect / 300s request timeout), returning
/// the number of bytes written. Removes any partial file if an error occurs mid-transfer.
pub async fn download_to_file(url: &str, dest: &Path, user_agent: &str) -> Result<u64, String> {
    let client = build_updater_client(user_agent, DOWNLOAD_TIMEOUT)?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("download failed with HTTP {}", status.as_u16()));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let file = std::fs::File::create(dest)
        .map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
    let mut writer = BufWriter::new(file);
    let mut total = 0u64;
    while let Some(chunk) = resp.chunk().await.map_err(|e| {
        let _ = std::fs::remove_file(dest);
        format!("download stream interrupted: {e}")
    })? {
        writer.write_all(&chunk).map_err(|e| {
            let _ = std::fs::remove_file(dest);
            format!("failed writing {}: {e}", dest.display())
        })?;
        total = total.saturating_add(chunk.len() as u64);
    }
    writer.flush().map_err(|e| {
        let _ = std::fs::remove_file(dest);
        format!("failed flushing {}: {e}", dest.display())
    })?;
    Ok(total)
}

/// Download `release`'s archive and `SHA256SUMS.txt` into `staging_dir` via `reqwest`, verify the
/// SHA-256 checksum in pure Rust via [`sha256_file`] and [`find_sha256_entry`], and return the
/// verified tarball path. Refuses unverified updates if `sha256_url` is missing.
pub async fn download_and_verify_release(
    release: &ReleaseInfo,
    staging_dir: &Path,
    user_agent: &str,
) -> Result<PathBuf, String> {
    if !is_safe_asset_name(&release.asset_name) {
        return Err(format!(
            "invalid release asset name: {}",
            release.asset_name
        ));
    }
    let sums_url = release.sha256_url.as_deref().ok_or_else(|| {
        "release is missing SHA256SUMS.txt; refusing unverified update".to_owned()
    })?;

    std::fs::create_dir_all(staging_dir).map_err(|e| {
        format!(
            "cannot create staging directory {}: {e}",
            staging_dir.display()
        )
    })?;

    let tarball_path = staging_dir.join(&release.asset_name);
    download_to_file(&release.download_url, &tarball_path, user_agent).await?;

    let client = build_updater_client(user_agent, METADATA_TIMEOUT)?;
    let sums_resp = client
        .get(sums_url)
        .send()
        .await
        .map_err(|e| format!("failed to fetch SHA256SUMS.txt: {e}"))?;
    let sums_status = sums_resp.status();
    if !sums_status.is_success() {
        let _ = std::fs::remove_file(&tarball_path);
        return Err(format!(
            "failed to fetch SHA256SUMS.txt (HTTP {})",
            sums_status.as_u16()
        ));
    }
    let sums_text = sums_resp
        .text()
        .await
        .map_err(|e| format!("failed reading SHA256SUMS.txt: {e}"))?;
    let expected = find_sha256_entry(&sums_text, &release.asset_name).ok_or_else(|| {
        let _ = std::fs::remove_file(&tarball_path);
        format!(
            "SHA256SUMS.txt has no checksum entry for {}",
            release.asset_name
        )
    })?;
    let actual = sha256_file(&tarball_path)?;
    if actual != expected {
        let _ = std::fs::remove_file(&tarball_path);
        return Err(format!(
            "SHA-256 mismatch (expected {expected}, got {actual})"
        ));
    }
    Ok(tarball_path)
}

/// Blocking wrapper around [`download_and_verify_release`] for non-Tokio callers (e.g. GPUI
/// background executor threads), running `reqwest` + pure-Rust SHA-256 verification on a
/// lightweight temporary Tokio runtime.
pub fn download_and_verify_release_blocking(
    release: &ReleaseInfo,
    staging_dir: &Path,
    user_agent: &str,
) -> Result<PathBuf, String> {
    block_on_updater(download_and_verify_release(
        release,
        staging_dir,
        user_agent,
    ))?
}

/// Extract a `.tar.gz` archive into `dest_dir` via `tar -xzf`, returning clear stderr on failure.
pub fn extract_tar_gz(tarball: &Path, dest_dir: &Path) -> Result<(), String> {
    let tar_bin = if cfg!(target_os = "macos") {
        "/usr/bin/tar"
    } else {
        "tar"
    };
    let out = Command::new(tar_bin)
        .args(["-xzf"])
        .arg(tarball)
        .arg("-C")
        .arg(dest_dir)
        .output()
        .map_err(|e| format!("failed to run tar: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(if stderr.is_empty() {
            format!(
                "failed to extract update archive (tar exited {})",
                out.status
            )
        } else {
            format!("failed to extract update archive: {stderr}")
        });
    }
    Ok(())
}

/// Create or replace a symlink at `link` pointing to `app_bundle/Contents/MacOS/slackless`.
#[cfg(unix)]
pub fn install_cli_symlink_at(app_bundle: &Path, link: &Path) -> Result<PathBuf, String> {
    let bundled = app_bundle.join("Contents/MacOS/slackless");
    if !bundled.is_file() {
        return Err(format!(
            "{} does not contain a bundled CLI executable",
            app_bundle.display()
        ));
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    if std::fs::symlink_metadata(link).is_ok() {
        std::fs::remove_file(link)
            .map_err(|e| format!("cannot replace {}: {e}", link.display()))?;
    }
    std::os::unix::fs::symlink(&bundled, link).map_err(|e| {
        format!(
            "cannot link {} -> {}: {e}",
            link.display(),
            bundled.display()
        )
    })?;
    Ok(link.to_path_buf())
}

#[cfg(not(unix))]
pub fn install_cli_symlink_at(_app_bundle: &Path, _link: &Path) -> Result<PathBuf, String> {
    Err("CLI symlink installation is currently supported on macOS/Unix only".to_owned())
}

/// Install or refresh the `slackless` CLI symlink in the best writable `$PATH` directory
/// (`/usr/local/bin/slackless` when writable, else `~/.local/bin/slackless`), pointing to
/// `app_bundle/Contents/MacOS/slackless`. Returns the created symlink path on success.
pub fn install_cli_symlink(app_bundle: &Path) -> Option<PathBuf> {
    let link = resolve_cli_link_path()?;
    install_cli_symlink_at(app_bundle, &link).ok()
}

/// Returns `true` when `name` is a single safe filename component (no path separators or `..`).
pub fn is_safe_asset_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && Path::new(name).file_name() == Some(std::ffi::OsStr::new(name))
}

/// Extract the lowercase hex SHA-256 hash matching `asset_name` from a `SHA256SUMS.txt` body.
pub fn find_sha256_entry(sums_text: &str, asset_name: &str) -> Option<String> {
    sums_text.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        if name == asset_name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            Some(hash.to_ascii_lowercase())
        } else {
            None
        }
    })
}

/// Returns `true` if `dir` exists and the current process can create a file inside it.
pub fn dir_is_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(format!(".slackless-probe-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Locate the `.app` bundle containing the currently running executable (macOS only).
pub fn current_app_bundle() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let exe = std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .ok()?;
        for ancestor in exe.ancestors() {
            if ancestor
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("app"))
            {
                return Some(ancestor.to_path_buf());
            }
        }
        None
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Resolve the target `Slackless.app` installation path consistently across CLI and GUI:
/// 1. The `.app` bundle containing the currently running binary, if bundled.
/// 2. Existing `/Applications/Slackless.app`, else existing `~/Applications/Slackless.app`.
/// 3. `/Applications/Slackless.app` if writable, else `~/Applications/Slackless.app`.
pub fn resolve_target_app_bundle() -> Option<PathBuf> {
    if let Some(bundle) = current_app_bundle() {
        return Some(bundle);
    }
    let sys_app = PathBuf::from("/Applications/Slackless.app");
    let user_app =
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Applications/Slackless.app"));
    if sys_app.is_dir() {
        return Some(sys_app);
    }
    if let Some(ref ua) = user_app
        && ua.is_dir()
    {
        return Some(ua.clone());
    }
    if dir_is_writable(Path::new("/Applications")) {
        Some(sys_app)
    } else {
        user_app
    }
}

/// Candidate directories for the `slackless` CLI symlink (`/usr/local/bin`, then `~/.local/bin`).
pub fn cli_candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/bin"));
    }
    dirs
}

/// Best writable symlink path for the `slackless` CLI (`/usr/local/bin/slackless` when writable,
/// else `~/.local/bin/slackless`). Never falls back to world-writable `/tmp`.
pub fn resolve_cli_link_path() -> Option<PathBuf> {
    let dirs = cli_candidate_dirs();
    for dir in &dirs {
        if dir_is_writable(dir) {
            return Some(dir.join("slackless"));
        }
    }
    dirs.into_iter().last().map(|d| d.join("slackless"))
}

/// Returns `true` if `dir` is present in `$PATH` (or on macOS `/etc/paths`).
pub fn dir_in_path(dir: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        if dir == Path::new("/usr/local/bin") {
            return true;
        }
        if let Ok(etc_paths) = std::fs::read_to_string("/etc/paths")
            && etc_paths.lines().any(|line| Path::new(line.trim()) == dir)
        {
            return true;
        }
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|p| p == dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_calver_four_tuple_and_semver_versions() {
        assert_eq!(parse_version("2026.11.24-03"), Some((2026, 11, 24, 3)));
        assert_eq!(parse_version("2026.11.24.03"), Some((2026, 11, 24, 3)));
        assert_eq!(parse_version("v0.1.0"), Some((0, 1, 0, 0)));
        assert_eq!(parse_version("  v1.2.3  "), Some((1, 2, 3, 0)));
        assert_eq!(parse_version("1.2.3-rc1"), None);
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("nightly"), None);

        assert!(is_newer_version("2026.11.24-03", "2026.11.24-02"));
        assert!(is_newer_version("2026.11.24-10", "2026.11.24-09"));
        assert!(is_newer_version("2026.11.24-01", "0.1.0"));
        assert!(!is_newer_version("2026.11.24-02", "2026.11.24-02"));
        assert!(!is_newer_version("2026.11.24-01", "2026.11.24-02"));
        assert!(!is_newer_version("", "0.1.0"));
    }

    #[test]
    fn validates_asset_names_and_sha256_entries() {
        assert!(is_safe_asset_name(
            "Slackless-2026.11.24-03-macos-arm64.app.tar.gz"
        ));
        assert!(!is_safe_asset_name(""));
        assert!(!is_safe_asset_name("."));
        assert!(!is_safe_asset_name(".."));
        assert!(!is_safe_asset_name("../escape.tar.gz"));
        assert!(!is_safe_asset_name("dir/file.tar.gz"));
        assert!(!is_safe_asset_name("dir\\file.tar.gz"));

        let hash_upper = "0123456789ABCDEF".repeat(4);
        let hash_lower = hash_upper.to_ascii_lowercase();
        let sums = format!(
            "{} *Slackless-2026.11.24-03-macos-arm64.app.tar.gz\n",
            hash_upper
        );
        assert_eq!(
            find_sha256_entry(&sums, "Slackless-2026.11.24-03-macos-arm64.app.tar.gz"),
            Some(hash_lower)
        );
        assert_eq!(find_sha256_entry(&sums, "missing.tar.gz"), None);
    }

    #[test]
    fn parse_github_release_skips_drafts_and_prereleases_and_extracts_highlights() {
        let draft = json!({
            "tag_name": "2026.11.25-01",
            "html_url": "https://example.com",
            "draft": true,
            "prerelease": false,
            "assets": []
        })
        .to_string();
        assert_eq!(parse_github_release(draft.as_bytes()), Ok(None));

        let pre = json!({
            "tag_name": "2026.11.25-01",
            "html_url": "https://example.com",
            "draft": false,
            "prerelease": true,
            "assets": []
        })
        .to_string();
        assert_eq!(parse_github_release(pre.as_bytes()), Ok(None));

        let good = json!({
            "tag_name": "2026.11.25-02",
            "html_url": "https://github.com/ankarhem/slackless/releases/tag/2026.11.25-02",
            "body": "## Changes\n- Faster search (`1234abc`)\n## Install / Update (macOS)\n- Run curl",
            "draft": false,
            "prerelease": false,
            "assets": [
                {
                    "name": "Slackless-2026.11.25-02-macos-arm64.app.tar.gz",
                    "browser_download_url": "https://example.com/app.tar.gz"
                },
                {
                    "name": "SHA256SUMS.txt",
                    "browser_download_url": "https://example.com/SHA256SUMS.txt"
                }
            ]
        })
        .to_string();
        let rel = parse_github_release(good.as_bytes())
            .expect("valid release")
            .expect("published release");
        assert_eq!(rel.version, "2026.11.25-02");
        assert_eq!(rel.notes, vec!["Faster search"]);
        assert_eq!(
            rel.sha256_url.as_deref(),
            Some("https://example.com/SHA256SUMS.txt")
        );
    }

    #[test]
    fn dir_is_writable_checks_directory_permissions() {
        let dir = std::env::temp_dir().join(format!(
            "slackless-core-updater-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        assert!(dir_is_writable(&dir));
        assert!(!dir_is_writable(&dir.join("nonexistent")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha256_bytes_and_file_match_known_vectors_and_find_entry() {
        // NIST / RFC empty and "abc" SHA-256 test vectors.
        assert_eq!(
            sha256_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        let dir =
            std::env::temp_dir().join(format!("slackless-core-sha256-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");

        // Multi-chunk (>64 KiB) payload to exercise streaming read loop.
        let payload: Vec<u8> = (0u8..=255).cycle().take(150_000).collect();
        let file_path = dir.join("Slackless-2026.11.25-01-macos-arm64.app.tar.gz");
        std::fs::write(&file_path, &payload).expect("write payload");

        let from_bytes = sha256_bytes(&payload);
        let from_file = sha256_file(&file_path).expect("sha256_file");
        assert_eq!(from_file, from_bytes);
        assert_eq!(from_file.len(), 64);

        let sums = format!(
            "{}  other.tar.gz\n{} *Slackless-2026.11.25-01-macos-arm64.app.tar.gz\n",
            "0".repeat(64),
            from_file.to_ascii_uppercase()
        );
        assert_eq!(
            find_sha256_entry(&sums, "Slackless-2026.11.25-01-macos-arm64.app.tar.gz"),
            Some(from_file)
        );
        assert!(sha256_file(&dir.join("does-not-exist")).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn install_cli_symlink_at_creates_and_replaces_link() {
        let dir = std::env::temp_dir().join(format!(
            "slackless-core-symlink-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let app_v1 = dir.join("v1/Slackless.app");
        let app_v2 = dir.join("v2/Slackless.app");
        let cli_v1 = app_v1.join("Contents/MacOS/slackless");
        let cli_v2 = app_v2.join("Contents/MacOS/slackless");
        std::fs::create_dir_all(cli_v1.parent().expect("parent")).expect("mkdir v1");
        std::fs::create_dir_all(cli_v2.parent().expect("parent")).expect("mkdir v2");
        std::fs::write(&cli_v1, b"v1").expect("write v1");
        std::fs::write(&cli_v2, b"v2").expect("write v2");

        let link = dir.join("bin/slackless");
        let created = install_cli_symlink_at(&app_v1, &link).expect("create symlink");
        assert_eq!(created, link);
        assert_eq!(std::fs::read_link(&link).expect("read link v1"), cli_v1);

        let updated = install_cli_symlink_at(&app_v2, &link).expect("replace symlink");
        assert_eq!(updated, link);
        assert_eq!(std::fs::read_link(&link).expect("read link v2"), cli_v2);

        // Missing bundled CLI fails cleanly.
        assert!(install_cli_symlink_at(&dir.join("missing.app"), &link).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn download_and_verify_release_streams_and_validates_sha256() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        let asset_name = "Slackless-2026.11.25-05-macos-arm64.app.tar.gz";
        let payload = b"fake-tar-gz-payload-bytes-for-updater-test".to_vec();
        let good_hash = sha256_bytes(&payload);
        let good_sums = format!("{good_hash}  {asset_name}\n").into_bytes();
        let bad_sums = format!("{}  {asset_name}\n", "f".repeat(64)).into_bytes();

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local_addr");
        let payload_srv = payload.clone();
        let server_thread = std::thread::spawn(move || {
            for _ in 0..4 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                let mut req_line = String::new();
                let _ = reader.read_line(&mut req_line);
                let path = req_line.split_whitespace().nth(1).unwrap_or("/");
                let body: &[u8] = match path {
                    "/asset.tar.gz" => &payload_srv,
                    "/SHA256SUMS.txt" => &good_sums,
                    "/BADSUMS.txt" => &bad_sums,
                    _ => b"",
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
                let _ = stream.flush();
            }
        });

        let stage = std::env::temp_dir().join(format!(
            "slackless-core-dl-verify-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&stage);

        let release = ReleaseInfo {
            version: "2026.11.25-05".to_owned(),
            html_url: "https://example.com".to_owned(),
            asset_name: asset_name.to_owned(),
            download_url: format!("http://{addr}/asset.tar.gz"),
            sha256_url: Some(format!("http://{addr}/SHA256SUMS.txt")),
            notes: Vec::new(),
        };

        let verified = download_and_verify_release(&release, &stage, "Slackless-Test/1.0")
            .await
            .expect("verified download");
        assert_eq!(verified, stage.join(asset_name));
        assert_eq!(std::fs::read(&verified).expect("read archive"), payload);

        // Mismatched checksum fails and cleans up the unverified tarball.
        let mut bad_release = release.clone();
        bad_release.sha256_url = Some(format!("http://{addr}/BADSUMS.txt"));
        let err = download_and_verify_release(&bad_release, &stage, "Slackless-Test/1.0")
            .await
            .expect_err("checksum mismatch must fail");
        assert!(err.contains("SHA-256 mismatch"), "unexpected error: {err}");
        assert!(!stage.join(asset_name).exists());

        // Missing sha256_url refuses unverified update.
        let mut unsigned = release;
        unsigned.sha256_url = None;
        assert!(
            download_and_verify_release(&unsigned, &stage, "Slackless-Test/1.0")
                .await
                .is_err()
        );

        let _ = server_thread.join();
        let _ = std::fs::remove_dir_all(&stage);
    }

    #[test]
    fn download_and_verify_release_blocking_works_outside_tokio() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        let asset_name = "Slackless-2026.11.25-06-macos-arm64.app.tar.gz";
        let payload = b"blocking-wrapper-tar-gz-payload".to_vec();
        let good_hash = sha256_bytes(&payload);
        let good_sums = format!("{good_hash}  {asset_name}\n").into_bytes();

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local_addr");
        let payload_srv = payload.clone();
        let server_thread = std::thread::spawn(move || {
            for _ in 0..2 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                let mut req_line = String::new();
                let _ = reader.read_line(&mut req_line);
                let path = req_line.split_whitespace().nth(1).unwrap_or("/");
                let body: &[u8] = match path {
                    "/asset.tar.gz" => &payload_srv,
                    "/SHA256SUMS.txt" => &good_sums,
                    _ => b"",
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
                let _ = stream.flush();
            }
        });

        let stage = std::env::temp_dir().join(format!(
            "slackless-core-dl-blocking-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&stage);
        let release = ReleaseInfo {
            version: "2026.11.25-06".to_owned(),
            html_url: "https://example.com".to_owned(),
            asset_name: asset_name.to_owned(),
            download_url: format!("http://{addr}/asset.tar.gz"),
            sha256_url: Some(format!("http://{addr}/SHA256SUMS.txt")),
            notes: Vec::new(),
        };

        // Run from a plain OS thread with no Tokio runtime context.
        let stage_c = stage.clone();
        let verified = std::thread::spawn(move || {
            download_and_verify_release_blocking(&release, &stage_c, "Slackless-Test/1.0")
        })
        .join()
        .expect("join worker thread")
        .expect("verified blocking download");

        assert_eq!(verified, stage.join(asset_name));
        assert_eq!(std::fs::read(&verified).expect("read archive"), payload);

        let _ = server_thread.join();
        let _ = std::fs::remove_dir_all(&stage);
    }
}
