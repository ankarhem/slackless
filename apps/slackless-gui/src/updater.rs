//! In-app auto-updater (`F-SHELL-163`) backed by GitHub Releases (`ankarhem/slackless`).
//!
//! Delegates version parsing, release parsing, checksum lookup, and path resolution to
//! [`slackless_core::domain::updater`], while handling the macOS bundle swap & CLI symlink
//! lifecycle for the running GUI app.

use std::path::{Path, PathBuf};
use std::process::Command;

pub use slackless_core::domain::updater::{
    RELEASES_REPO, ReleaseInfo, current_app_bundle, dir_in_path, find_sha256_entry,
    is_newer_version, is_safe_asset_name,
};
use slackless_core::domain::updater::{
    cli_candidate_dirs, is_dev_version, parse_github_release, resolve_cli_link_path,
    resolve_target_app_bundle,
};
#[cfg(test)]
pub use slackless_core::domain::updater::{parse_release_highlights, parse_version};

/// Release version embedded at build time (`SLACKLESS_VERSION` in CI, falling back to Cargo.toml).
pub const CURRENT_VERSION: &str = match option_env!("SLACKLESS_VERSION") {
    Some(v) if !v.is_empty() => v,
    _ => env!("CARGO_PKG_VERSION"),
};

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

/// Installation status of the bundled `slackless` CLI symlink (`~/.local/bin/slackless`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliInstallStatus {
    /// Symlinked directly to this bundle's `Contents/MacOS/slackless`.
    Installed(PathBuf),
    /// Exists, but is a stale copy or points to a different bundle path.
    OutOfSync(PathBuf),
    /// Bundled CLI is present in `Slackless.app`, ready to symlink.
    NotInstalled,
    /// Running unbundled (e.g. `cargo run`) or without `Contents/MacOS/slackless`.
    Unavailable,
}

/// Best writable symlink path for the `slackless` CLI (`/usr/local/bin/slackless` when writable,
/// else `~/.local/bin/slackless`).
pub fn cli_link_path() -> PathBuf {
    resolve_cli_link_path().unwrap_or_else(|| PathBuf::from(".local/bin/slackless"))
}

/// Locate the matching `slackless` CLI executable embedded inside the running `Slackless.app`.
pub fn bundled_cli_path() -> Option<PathBuf> {
    let cli = current_app_bundle()?.join("Contents/MacOS/slackless");
    cli.is_file().then_some(cli)
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
    is_dev_version(&installed_version())
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
            && out.status.success()
        {
            let v = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if !v.is_empty() {
                return v;
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

    let Some(info) = parse_github_release(&out.stdout)? else {
        return Ok(None);
    };
    let current = installed_version();
    if !is_newer_version(&info.version, &current) {
        return Ok(None);
    }
    Ok(Some(info))
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
    let target_app = resolve_target_app_bundle()
        .ok_or_else(|| "cannot determine Applications directory".to_owned())?;
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
        if !is_safe_asset_name(&release.asset_name) {
            return Err(format!(
                "invalid release asset name: {}",
                release.asset_name
            ));
        }
        let sums_url = release.sha256_url.as_deref().ok_or_else(|| {
            "release is missing SHA256SUMS.txt; refusing unverified update".to_owned()
        })?;
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

        let sums_out = Command::new("/usr/bin/curl")
            .args(["-fsSL", "--max-time", "15", sums_url])
            .output()
            .map_err(|e| format!("failed to fetch SHA256SUMS.txt: {e}"))?;
        if !sums_out.status.success() {
            return Err("failed to fetch SHA256SUMS.txt".to_owned());
        }
        let sums_text = String::from_utf8_lossy(&sums_out.stdout);
        let expected = find_sha256_entry(&sums_text, &release.asset_name).ok_or_else(|| {
            format!(
                "SHA256SUMS.txt has no checksum entry for {}",
                release.asset_name
            )
        })?;
        verify_sha256(&archive, &expected)?;

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

    #[test]
    fn validates_asset_names_and_sha256_entries() {
        assert!(is_safe_asset_name(
            "Slackless-2026.11.24-03-macos-arm64.app.tar.gz"
        ));
        assert!(!is_safe_asset_name(""));
        assert!(!is_safe_asset_name(".."));
        assert!(!is_safe_asset_name("../evil.tar.gz"));
        assert!(!is_safe_asset_name("sub/evil.tar.gz"));
        assert!(!is_safe_asset_name("sub\\evil.tar.gz"));

        let good_hash = "a".repeat(64);
        let sums = format!("{good_hash}  Slackless-2026.11.24-03-macos-arm64.app.tar.gz\n");
        assert_eq!(
            find_sha256_entry(&sums, "Slackless-2026.11.24-03-macos-arm64.app.tar.gz"),
            Some(good_hash)
        );
        assert_eq!(find_sha256_entry(&sums, "other.tar.gz"), None);
        assert_eq!(
            find_sha256_entry("notahash  Slackless.tar.gz\n", "Slackless.tar.gz"),
            None
        );
    }

    #[test]
    fn parses_release_highlights_and_skips_install_section() {
        let body = r#"
## What's Changed

### Features
- Add cross-workspace CLI search (`abc1234`)
* Show release notes preview in Preferences (`def5678`)

### Bug Fixes
- Keep message scrolling smooth over hover actions

## Install / Update (macOS Apple Silicon)
- Run `slackless update --restart`
"#;
        assert_eq!(
            parse_release_highlights(body),
            vec![
                "Add cross-workspace CLI search",
                "Show release notes preview in Preferences",
                "Keep message scrolling smooth over hover actions",
            ]
        );
    }
}
