//! Shared GitHub Releases update checker, version comparator, checksum parser, and install-path
//! resolver used by both CLI (`slackless update`) and GUI (`Check for Updates`).

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::api::AssetClient;

/// GitHub repository hosting official Slackless releases.
pub const RELEASES_REPO: &str = "ankarhem/slackless";

/// GitHub REST endpoint for the latest official Slackless release.
pub const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/ankarhem/slackless/releases/latest";

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

/// Query GitHub Releases over HTTPS (using [`AssetClient`] / `reqwest`, respecting workspace proxy
/// settings and timeouts) for the latest non-draft, non-prerelease release.
pub async fn fetch_latest_release() -> Result<Option<ReleaseInfo>, String> {
    let client = AssetClient::new();
    let bytes = client
        .get(LATEST_RELEASE_API)
        .await
        .map_err(|e| format!("GitHub Releases check failed: {e}"))?;
    parse_github_release(&bytes)
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
}
