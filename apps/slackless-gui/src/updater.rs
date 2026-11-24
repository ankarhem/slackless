//! In-app auto-updater (`F-SHELL-163`) backed by GitHub Releases (`ankarhem/slackless`).
//!
//! Checks `https://api.github.com/repos/ankarhem/slackless/releases/latest` for newer
//! `YYYY.MM.DD-xx` releases, verifies `SHA256SUMS.txt`, atomically swaps the running `.app`
//! bundle in place without setting `com.apple.quarantine`, and relaunches Slackless.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::blockkit::serde_json;

/// GitHub repository hosting official Slackless releases.
pub const RELEASES_REPO: &str = "ankarhem/slackless";

/// Release version embedded at build time (`SLACKLESS_VERSION` in CI, falling back to Cargo.toml).
pub const CURRENT_VERSION: &str = match option_env!("SLACKLESS_VERSION") {
    Some(v) if !v.is_empty() => v,
    _ => env!("CARGO_PKG_VERSION"),
};

/// Metadata for an available GitHub Release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub version: String,
    pub html_url: String,
    pub asset_name: String,
    pub download_url: String,
    pub sha256_url: Option<String>,
}

/// UI state for the top update banner and `Slackless -> Check for Updates…`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    Available(ReleaseInfo),
    Downloading(String),
    UpToDate,
    Error(String),
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
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

/// Parse a `YYYY.MM.DD-xx`, `YYYY.MM.DD.xx`, or `major.minor.patch` version into a comparable tuple.
pub fn parse_version(raw: &str) -> Option<(u32, u32, u32, u32)> {
    let s = raw.trim().trim_start_matches('v');
    if s.is_empty() {
        return None;
    }
    // Split `YYYY.MM.DD-xx` first if `-` is present.
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
        _ => latest.trim() != current.trim() && !latest.trim().is_empty(),
    }
}

/// Locate the `.app` bundle containing the currently running executable (macOS only).
/// Returns `None` when running unbundled (e.g. `cargo run`).
pub fn current_app_bundle() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let exe = std::env::current_exe().ok()?;
        let macos_dir = exe.parent()?;
        if macos_dir.file_name()?.to_str()? != "MacOS" {
            return None;
        }
        let contents_dir = macos_dir.parent()?;
        if contents_dir.file_name()?.to_str()? != "Contents" {
            return None;
        }
        let app_dir = contents_dir.parent()?;
        if app_dir.extension()?.to_str()? != "app" {
            return None;
        }
        Some(app_dir.to_path_buf())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Installation status of the bundled `slackless` CLI symlink (`~/.local/bin/slackless`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliInstallStatus {
    /// `~/.local/bin/slackless` is symlinked directly to this bundle's `Contents/MacOS/slackless`.
    Installed(PathBuf),
    /// `~/.local/bin/slackless` exists, but is a stale copy or points to a different bundle path.
    OutOfSync(PathBuf),
    /// Bundled CLI is present in `Slackless.app`, ready to symlink into `~/.local/bin/slackless`.
    NotInstalled,
    /// Running unbundled (e.g. `cargo run`) or without `Contents/MacOS/slackless`.
    Unavailable,
}

/// Candidate directories for installing the `slackless` CLI symlink, ordered by macOS default
/// `$PATH` availability without requiring `sudo`:
/// 1. `/usr/local/bin` (in macOS `/etc/paths` by default; used by VS Code, Zed, Cursor, Docker)
/// 2. `~/.local/bin` (user-writable fallback when `/usr/local/bin` is not writable)
fn cli_candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    dirs.push(home.join(".local/bin"));
    dirs
}

fn dir_is_writable(dir: &Path) -> bool {
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

/// Best writable symlink path for the `slackless` CLI (`/usr/local/bin/slackless` when writable,
/// else `~/.local/bin/slackless`).
pub fn cli_link_path() -> PathBuf {
    let dirs = cli_candidate_dirs();
    for dir in &dirs {
        if dir_is_writable(dir) {
            return dir.join("slackless");
        }
    }
    dirs.into_iter()
        .last()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("slackless")
}

/// Locate the matching `slackless` CLI executable embedded inside the running `Slackless.app`.
pub fn bundled_cli_path() -> Option<PathBuf> {
    let cli = current_app_bundle()?.join("Contents/MacOS/slackless");
    cli.is_file().then_some(cli)
}

/// Returns `true` if `dir` is present in `$PATH` (or on macOS `/etc/paths`, which LaunchServices
/// omits from a GUI app's own `PATH` even though every interactive shell includes it).
pub fn dir_in_path(dir: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        if dir == Path::new("/usr/local/bin") {
            return true;
        }
        if let Ok(etc_paths) = std::fs::read_to_string("/etc/paths") {
            if etc_paths.lines().any(|line| Path::new(line.trim()) == dir) {
                return true;
            }
        }
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|p| p == dir)
}

/// Inspect whether `slackless` CLI is installed (checking `/usr/local/bin` and `~/.local/bin`)
/// and matches the running `Slackless.app`.
pub fn cli_install_status() -> CliInstallStatus {
    let Some(bundled) = bundled_cli_path() else {
        return CliInstallStatus::Unavailable;
    };
    let mut out_of_sync = None;
    for dir in cli_candidate_dirs() {
        let link = dir.join("slackless");
        match std::fs::symlink_metadata(&link) {
            Err(_) => continue,
            Ok(meta) if meta.file_type().is_symlink() => {
                if std::fs::read_link(&link).ok().as_deref() == Some(bundled.as_path()) {
                    return CliInstallStatus::Installed(link);
                }
                if out_of_sync.is_none() {
                    out_of_sync = Some(link);
                }
            }
            Ok(_) => {
                if out_of_sync.is_none() {
                    out_of_sync = Some(link);
                }
            }
        }
    }
    match out_of_sync {
        Some(path) => CliInstallStatus::OutOfSync(path),
        None => CliInstallStatus::NotInstalled,
    }
}

/// Create or refresh the `slackless` CLI symlink in the best writable `$PATH` directory
/// (`/usr/local/bin/slackless` or `~/.local/bin/slackless`), pointing directly to the bundled CLI
/// inside `Slackless.app`.
#[cfg(unix)]
pub fn install_cli_symlink() -> Result<PathBuf, String> {
    let bundled = bundled_cli_path()
        .ok_or_else(|| "Slackless.app does not contain a bundled CLI executable".to_owned())?;
    let link = cli_link_path();
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    if std::fs::symlink_metadata(&link).is_ok() {
        std::fs::remove_file(&link)
            .map_err(|e| format!("cannot replace {}: {e}", link.display()))?;
    }
    std::os::unix::fs::symlink(&bundled, &link).map_err(|e| {
        format!(
            "cannot link {} -> {}: {e}",
            link.display(),
            bundled.display()
        )
    })?;
    Ok(link)
}

#[cfg(not(unix))]
pub fn install_cli_symlink() -> Result<PathBuf, String> {
    Err("CLI symlink installation is currently supported on macOS/Unix only".to_owned())
}

/// Remove any installed `slackless` CLI symlink across standard bin directories.
pub fn uninstall_cli_symlink() -> Result<(), String> {
    for dir in cli_candidate_dirs() {
        let link = dir.join("slackless");
        if std::fs::symlink_metadata(&link).is_ok() {
            std::fs::remove_file(&link)
                .map_err(|e| format!("cannot remove {}: {e}", link.display()))?;
        }
    }
    Ok(())
}

/// Returns `true` when running a local development build (`0.1.0` or `*-dev`) rather than a
/// stamped CalVer release (`YYYY.MM.DD-xx`). Startup auto-update checks are skipped on dev builds
/// so local development bundles never nag or overwrite themselves with older releases.
pub fn is_dev_build() -> bool {
    let v = installed_version();
    v == "0.1.0" || v.ends_with("-dev") || v.contains("-dev.")
}

/// Return the installed app's version string (prefers `SLACKLESS_VERSION` compiled into the binary;
/// falls back to reading `CFBundleShortVersionString` from `Info.plist` when running bundled).
pub fn installed_version() -> String {
    if CURRENT_VERSION != "0.1.0" {
        return CURRENT_VERSION.to_owned();
    }
    #[cfg(target_os = "macos")]
    if let Some(app) = current_app_bundle() {
        let plist = app.join("Contents/Info.plist");
        if let Ok(out) = Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Print :CFBundleShortVersionString"])
            .arg(&plist)
            .output()
        {
            if out.status.success() {
                let v = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                if !v.is_empty() {
                    return v;
                }
            }
        }
    }
    CURRENT_VERSION.to_owned()
}

/// Query GitHub Releases for the latest macOS arm64 release.
/// Returns `Ok(Some(info))` if a newer version is available, `Ok(None)` if already up to date.
pub fn check_for_update() -> Result<Option<ReleaseInfo>, String> {
    let url = format!("https://api.github.com/repos/{RELEASES_REPO}/releases/latest");
    let ua = format!("Slackless/{CURRENT_VERSION}");
    let out = Command::new("/usr/bin/curl")
        .args([
            "-fsSL",
            "--max-time",
            "10",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            &format!("User-Agent: {ua}"),
            &url,
        ])
        .output()
        .map_err(|e| format!("failed to run curl: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(if stderr.is_empty() {
            format!("GitHub API request failed (status {})", out.status)
        } else {
            stderr
        });
    }

    let rel: GhRelease = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("invalid GitHub release JSON: {e}"))?;
    if rel.draft || rel.prerelease {
        return Ok(None);
    }

    let current = installed_version();
    if !is_newer_version(&rel.tag_name, &current) {
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

    Ok(Some(ReleaseInfo {
        version: rel.tag_name,
        html_url: rel.html_url,
        asset_name: asset.name.clone(),
        download_url: asset.browser_download_url.clone(),
        sha256_url,
    }))
}

fn verify_sha256(archive_path: &Path, expected_hex: &str) -> Result<(), String> {
    let out = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(archive_path)
        .output()
        .map_err(|e| format!("failed to run shasum: {e}"))?;
    if !out.status.success() {
        return Err("shasum verification failed".to_owned());
    }
    let actual = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let expected = expected_hex.trim().to_ascii_lowercase();
    if actual != expected {
        return Err(format!(
            "SHA-256 mismatch (expected {expected}, got {actual})"
        ));
    }
    Ok(())
}

/// Download `release`, verify its SHA-256 checksum, extract `Slackless.app`, and spawn a detached
/// swap-and-relaunch helper that waits for this PID to exit. Caller should immediately quit the
/// app (`cx.quit()`) upon `Ok(())`.
#[cfg(target_os = "macos")]
pub fn download_and_schedule_swap(release: &ReleaseInfo) -> Result<(), String> {
    let target_app = current_app_bundle().unwrap_or_else(|| {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let sys_apps = PathBuf::from("/Applications/Slackless.app");
        if sys_apps.exists() {
            sys_apps
        } else {
            home.join("Applications/Slackless.app")
        }
    });
    let parent = target_app
        .parent()
        .ok_or_else(|| "cannot determine Applications directory".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("cannot write to {}: {e}", parent.display()))?;

    let stage_dir = parent.join(format!(".slackless-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage_dir);
    std::fs::create_dir_all(&stage_dir).map_err(|e| {
        format!(
            "cannot create staging directory in {}: {e}",
            parent.display()
        )
    })?;

    let result = (|| -> Result<PathBuf, String> {
        let archive = stage_dir.join(&release.asset_name);
        let dl = Command::new("/usr/bin/curl")
            .args(["-fsSL", "--max-time", "180", "-o"])
            .arg(&archive)
            .arg(&release.download_url)
            .status()
            .map_err(|e| format!("download failed: {e}"))?;
        if !dl.success() {
            return Err(format!("download failed (curl exited {})", dl));
        }

        if let Some(sums_url) = &release.sha256_url {
            let sums_out = Command::new("/usr/bin/curl")
                .args(["-fsSL", "--max-time", "15", sums_url])
                .output()
                .map_err(|e| format!("failed to fetch SHA256SUMS.txt: {e}"))?;
            if !sums_out.status.success() {
                return Err("failed to fetch SHA256SUMS.txt".to_owned());
            }
            let sums_text = String::from_utf8_lossy(&sums_out.stdout);
            if let Some(expected) = sums_text.lines().find_map(|line| {
                let mut parts = line.split_whitespace();
                let hash = parts.next()?;
                let name = parts.next()?.trim_start_matches('*');
                (name == release.asset_name).then_some(hash)
            }) {
                verify_sha256(&archive, expected)?;
            }
        }

        let extract = Command::new("/usr/bin/tar")
            .args(["-xzf"])
            .arg(&archive)
            .arg("-C")
            .arg(&stage_dir)
            .status()
            .map_err(|e| format!("failed to extract update archive: {e}"))?;
        if !extract.success() {
            return Err("failed to extract update archive".to_owned());
        }

        let new_app = stage_dir.join("Slackless.app");
        if !new_app.join("Contents/MacOS/slackless-gui").is_file() {
            return Err("update archive did not contain Slackless.app".to_owned());
        }
        let _ = Command::new("/usr/bin/xattr")
            .args(["-dr", "com.apple.quarantine"])
            .arg(&new_app)
            .status();
        Ok(new_app)
    })();

    let new_app = match result {
        Ok(app) => app,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&stage_dir);
            return Err(err);
        }
    };

    // Spawn a detached shell script that waits for our PID to exit cleanly, atomically swaps the
    // `.app` bundle (with rollback on error), registers LaunchServices, and relaunches Slackless.
    let script = r#"
PID="$1"
NEW_APP="$2"
DEST="$3"
STAGE="$4"
i=0
while kill -0 "$PID" 2>/dev/null && [ "$i" -lt 150 ]; do
  sleep 0.1
  i=$((i + 1))
done
rm -rf "${DEST}.prev"
if [ -d "$DEST" ]; then
  mv "$DEST" "${DEST}.prev" 2>/dev/null || true
fi
if mv "$NEW_APP" "$DEST"; then
  rm -rf "${DEST}.prev"
else
  if [ -d "${DEST}.prev" ]; then
    mv "${DEST}.prev" "$DEST" 2>/dev/null || true
  fi
  rm -rf "$STAGE"
  exit 1
fi
rm -rf "$STAGE"
# Keep any installed `slackless` CLI symlink/binary synced to the updated bundle so the CLI always
# matches the app version.
CLI_BIN="${DEST}/Contents/MacOS/slackless"
if [ -x "$CLI_BIN" ]; then
  for CLI_LINK in /usr/local/bin/slackless "${HOME}/.local/bin/slackless"; do
    if [ -L "$CLI_LINK" ] || [ -e "$CLI_LINK" ]; then
      ln -sf "$CLI_BIN" "$CLI_LINK" 2>/dev/null || true
    fi
  done
fi
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$DEST" >/dev/null 2>&1 || true
open "$DEST"
"#;

    Command::new("/bin/sh")
        .args(["-c", script, "slackless-updater"])
        .arg(std::process::id().to_string())
        .arg(&new_app)
        .arg(&target_app)
        .arg(&stage_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| {
            let _ = std::fs::remove_dir_all(&stage_dir);
            format!("failed to launch updater helper: {e}")
        })?;

    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn download_and_schedule_swap(_release: &ReleaseInfo) -> Result<(), String> {
    Err("in-app updates are currently supported on macOS only".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calver_and_semver_comparison_works() {
        assert_eq!(parse_version("2026.11.24-01"), Some((2026, 11, 24, 1)));
        assert_eq!(parse_version("2026.11.24-12"), Some((2026, 11, 24, 12)));
        assert_eq!(parse_version("0.1.0"), Some((0, 1, 0, 0)));

        assert!(is_newer_version("2026.11.24-01", "0.1.0"));
        assert!(is_newer_version("2026.11.24-02", "2026.11.24-01"));
        assert!(is_newer_version("2026.11.24-10", "2026.11.24-09"));
        assert!(is_newer_version("2026.11.25-01", "2026.11.24-99"));
        assert!(!is_newer_version("2026.11.24-01", "2026.11.24-01"));
        assert!(!is_newer_version("2026.11.24-01", "2026.11.24-02"));
    }
}
