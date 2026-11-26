//! In-app auto-updater (`F-SHELL-163`) backed by GitHub Releases (`ankarhem/slackless`).
//!
//! Delegates pure-Rust release checks (`reqwest`), streaming download + SHA-256 verification
//! (`sha2`), archive extraction, code-signature verification, the `.app` swap with rollback and
//! CLI symlink installation to [`slackless_core::domain::updater`], while handling the relaunch
//! of the running GUI app once it has quit.

#[cfg(target_os = "macos")]
use std::ffi::OsStr;
#[cfg(target_os = "macos")]
use std::path::Path;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;

pub use slackless_core::domain::updater::{
    RELEASES_REPO, ReleaseInfo, UpdateChannel, current_app_bundle, dir_in_path, is_newer_version,
};
use slackless_core::domain::updater::{
    cli_candidate_dirs, fetch_latest_release_for_channel_blocking, install_cli_symlink_at,
    is_dev_version, resolve_cli_link_path,
};
#[cfg(target_os = "macos")]
use slackless_core::domain::updater::{
    download_and_verify_release_blocking, extract_tar_gz, resolve_target_app_bundle,
    swap_app_bundle, update_signature_reference, verify_update_signature,
};

/// LaunchServices registration tool, run after a swap so Spotlight / `open` see the new bundle.
#[cfg(target_os = "macos")]
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
#[cfg(test)]
pub use slackless_core::domain::updater::{
    find_sha256_entry, is_safe_asset_name, parse_release_highlights, parse_version,
};

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
pub fn install_cli_symlink() -> Result<PathBuf, String> {
    let bundle = current_app_bundle()
        .ok_or_else(|| "Slackless.app does not contain a bundled CLI executable".to_owned())?;
    let link = cli_link_path();
    install_cli_symlink_at(&bundle, &link)
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

/// Query GitHub Releases for the latest macOS arm64 release on `channel` (`Stable` or `Beta`,
/// `F-SHELL-164`) via pure-Rust `reqwest`. Returns `Ok(Some(info))` if a newer version is
/// available, `Ok(None)` if already up to date.
pub fn check_for_update_on(channel: UpdateChannel) -> Result<Option<ReleaseInfo>, String> {
    let ua = format!("Slackless/{CURRENT_VERSION}");
    let Some(info) = fetch_latest_release_for_channel_blocking(RELEASES_REPO, channel, &ua)? else {
        return Ok(None);
    };
    let current = installed_version();
    if !is_newer_version(&info.version, &current) {
        return Ok(None);
    }
    Ok(Some(info))
}

/// Download `release` via pure-Rust `reqwest`, verify its SHA-256 checksum in pure Rust (`sha2`),
/// extract `Slackless.app`, verify its code signature against the running app, swap it into place
/// in-process (rolling back on failure), refresh the CLI symlink, register it with LaunchServices
/// and spawn a detached helper that relaunches the new bundle once this process has exited.
///
/// Caller should quit the app (`cx.quit()`) upon `Ok(())`. On `Err` the swap was rolled back (or
/// never attempted), so the app keeps running and can show the error.
#[cfg(target_os = "macos")]
pub fn install_update(release: &ReleaseInfo) -> Result<(), String> {
    let target_app = resolve_target_app_bundle()
        .ok_or_else(|| "cannot determine Applications directory".to_owned())?;
    let parent = target_app
        .parent()
        .ok_or_else(|| "cannot determine Applications directory".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("cannot write to {}: {e}", parent.display()))?;

    // Stage next to the target so the swap is a same-volume rename.
    let stage_dir = parent.join(format!(".slackless-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage_dir);
    std::fs::create_dir_all(&stage_dir).map_err(|e| {
        format!(
            "cannot create staging directory in {}: {e}",
            parent.display()
        )
    })?;

    let swapped = stage_and_swap(release, &stage_dir, &target_app);
    let _ = std::fs::remove_dir_all(&stage_dir);
    swapped?;

    // From here on the new bundle is installed whatever happens below.
    if let Some(link) = slackless_core::domain::updater::install_cli_symlink(&target_app) {
        tracing::debug!(link = %link.display(), "CLI symlink refreshed after update");
    }
    let _ = Command::new(LSREGISTER)
        .args(["-f"])
        .arg(&target_app)
        .status();
    schedule_relaunch(&target_app).map_err(|e| {
        format!(
            "Slackless was updated but will not relaunch automatically ({e}); quit and reopen Slackless"
        )
    })
}

/// Download, verify, extract, verify the signature and swap the new bundle into `target_app`.
#[cfg(target_os = "macos")]
fn stage_and_swap(
    release: &ReleaseInfo,
    stage_dir: &Path,
    target_app: &Path,
) -> Result<(), String> {
    let ua = format!("Slackless/{CURRENT_VERSION}");
    let archive = download_and_verify_release_blocking(release, stage_dir, &ua)?;
    extract_tar_gz(&archive, stage_dir)?;

    let new_app = stage_dir.join("Slackless.app");
    if !new_app.join("Contents/MacOS/slackless-gui").is_file() {
        return Err("update archive did not contain Slackless.app".to_owned());
    }
    verify_update_signature(&new_app, update_signature_reference(target_app).as_deref())?;
    let _ = Command::new("/usr/bin/xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&new_app)
        .status();

    // Renaming the running app's bundle is fine on macOS; a failed move rolls back, leaving the
    // running app in place.
    swap_app_bundle(&new_app, target_app)
}

/// Relaunch `app` with `open` once this process has exited (the relaunch has to happen after the
/// quit, so it runs in a detached helper rather than in-process).
#[cfg(target_os = "macos")]
fn schedule_relaunch(app: &Path) -> Result<(), String> {
    spawn_after_exit(
        std::process::id(),
        &[OsStr::new("/usr/bin/open"), app.as_os_str()],
    )
}

/// Spawn a detached `/bin/sh` helper that waits for `pid` to exit and then runs `command`
/// (an argument list, never interpolated into the script). Gives up without running anything if
/// `pid` is still alive after five minutes.
#[cfg(target_os = "macos")]
fn spawn_after_exit(pid: u32, command: &[&OsStr]) -> Result<(), String> {
    const SCRIPT: &str = r#"
PID="$1"
shift
i=0
# shortcut: poll the PID every 100 ms for up to 5 minutes, upgrade to a kqueue watcher if a bound ever matters
while kill -0 "$PID" 2>/dev/null && [ "$i" -lt 3000 ]; do
  sleep 0.1
  i=$((i + 1))
done
kill -0 "$PID" 2>/dev/null && exit 0
exec "$@"
"#;
    Command::new("/bin/sh")
        .arg("-c")
        .arg(SCRIPT)
        .arg("slackless-relaunch")
        .arg(pid.to_string())
        .args(command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to launch relaunch helper: {e}"))
}

#[cfg(not(target_os = "macos"))]
pub fn install_update(_release: &ReleaseInfo) -> Result<(), String> {
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

    #[cfg(target_os = "macos")]
    #[test]
    fn relaunch_helper_runs_its_command_only_after_the_pid_exits() {
        use std::io::{BufRead, BufReader};
        use std::process::Stdio;
        use std::sync::mpsc;
        use std::time::Duration;

        struct TempDir(PathBuf);
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let dir = TempDir(
            std::env::temp_dir().join(format!("slackless-gui-relaunch-{}", std::process::id())),
        );
        let _ = std::fs::remove_dir_all(&dir.0);
        std::fs::create_dir_all(&dir.0).expect("create temp dir");

        // The helper reports through a FIFO, so the test blocks on the result instead of polling.
        let fifo = dir.0.join("result");
        let status = Command::new("/usr/bin/mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(status.success(), "mkfifo failed");
        let (tx, rx) = mpsc::channel();
        let reader_fifo = fifo.clone();
        let reader = std::thread::spawn(move || {
            let file = std::fs::File::open(&reader_fifo).expect("open fifo");
            let mut line = String::new();
            let _ = BufReader::new(file).read_line(&mut line);
            let _ = tx.send(line);
        });

        // A child whose exit the test controls: `cat` ends once its stdin closes. The helper's
        // command records whether that PID was still alive when the helper got to running it.
        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn cat");
        let pid = child.id();
        let script =
            "if kill -0 \"$2\" 2>/dev/null; then echo alive > \"$1\"; else echo gone > \"$1\"; fi";
        spawn_after_exit(
            pid,
            &[
                OsStr::new("/bin/sh"),
                OsStr::new("-c"),
                OsStr::new(script),
                OsStr::new("sh"),
                fifo.as_os_str(),
                OsStr::new(&pid.to_string()),
            ],
        )
        .expect("spawn helper");
        drop(child.stdin.take());
        child.wait().expect("cat exits");

        let result = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("helper must run its command after the pid exits");
        assert_eq!(result.trim(), "gone");
        reader.join().expect("reader thread");
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
