//! Self-update command (`slackless update`) backed by GitHub Releases.
//!
//! Delegates version comparison, GitHub release fetching (filtering out drafts and prereleases),
//! pure-Rust HTTPS download & SHA-256 verification, archive extraction, macOS install-path
//! resolution, and CLI symlink refresh to `slackless_core::domain::updater`. Safely stops a
//! running `Slackless.app` before swapping on `--restart` and performs an atomic swap with
//! `.app.prev` rollback.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use slackless_core::domain::updater::{self, ReleaseInfo, UpdateChannel, is_newer_version};

use crate::output;
use crate::{CLI_VERSION, Ctx, fail};

#[cfg(target_os = "macos")]
const QUIT_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(target_os = "macos")]
const QUIT_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[cfg(target_os = "macos")]
fn is_slackless_app_running() -> bool {
    Command::new("pgrep")
        .args(["-x", "Slackless"])
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(target_os = "macos")]
fn quit_running_app_and_wait() -> Result<()> {
    if !is_slackless_app_running() {
        return Ok(());
    }
    let status = Command::new("osascript")
        .args(["-e", "tell application \"Slackless\" to quit"])
        .status()
        .context("requesting Slackless.app to quit")?;
    if !status.success() {
        anyhow::bail!("failed to ask Slackless.app to quit (osascript exited with {status})");
    }
    let deadline = Instant::now() + QUIT_WAIT_TIMEOUT;
    while Instant::now() < deadline {
        if !is_slackless_app_running() {
            return Ok(());
        }
        std::thread::sleep(QUIT_POLL_INTERVAL);
    }
    if is_slackless_app_running() {
        anyhow::bail!(
            "Slackless.app is still running after {}s; quit it manually and retry `slackless update`",
            QUIT_WAIT_TIMEOUT.as_secs()
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
async fn download_and_install(
    release: &ReleaseInfo,
    restart: bool,
) -> Result<(PathBuf, Option<PathBuf>)> {
    let target_app = updater::resolve_target_app_bundle().ok_or_else(|| {
        anyhow::anyhow!("could not resolve Slackless.app install path (HOME not set)")
    })?;
    if let Some(parent) = target_app.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let stage_dir = std::env::temp_dir().join(format!(
        "slackless-cli-update-{}-{}",
        std::process::id(),
        release.version
    ));
    if stage_dir.exists() {
        std::fs::remove_dir_all(&stage_dir)
            .with_context(|| format!("clearing previous staging dir {}", stage_dir.display()))?;
    }
    std::fs::create_dir_all(&stage_dir).context("creating update staging directory")?;

    let install_result = install_in_stage(release, &stage_dir, &target_app, restart).await;
    let _ = std::fs::remove_dir_all(&stage_dir);
    install_result
}

#[cfg(target_os = "macos")]
async fn install_in_stage(
    release: &ReleaseInfo,
    stage_dir: &Path,
    target_app: &Path,
    restart: bool,
) -> Result<(PathBuf, Option<PathBuf>)> {
    let user_agent = format!("Slackless/{CLI_VERSION}");
    let tarball_path = updater::download_and_verify_release(release, stage_dir, &user_agent)
        .await
        .map_err(|err| anyhow::anyhow!("{err}"))?;

    updater::extract_tar_gz(&tarball_path, stage_dir).map_err(|err| anyhow::anyhow!("{err}"))?;

    let new_app = stage_dir.join("Slackless.app");
    if !new_app.is_dir() {
        anyhow::bail!("archive did not contain Slackless.app");
    }
    let _ = Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&new_app)
        .status();

    // Stop a running Slackless.app and verify process exit BEFORE swapping the bundle on disk.
    if restart {
        quit_running_app_and_wait()?;
    }

    // Atomic swap with `.prev` rollback.
    swap_app_bundle(&new_app, target_app)?;

    // Refresh CLI symlink to point at the newly installed bundle.
    let linked_cli = updater::install_cli_symlink(target_app);

    let _ = Command::new(
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
    )
    .args(["-f"])
    .arg(target_app)
    .status();

    if restart {
        let open_status = Command::new("open")
            .arg(target_app)
            .status()
            .context("relaunching Slackless.app")?;
        if !open_status.success() {
            anyhow::bail!(
                "updated {}, but relaunching failed (`open` exited with {open_status})",
                target_app.display()
            );
        }
    }

    Ok((target_app.to_path_buf(), linked_cli))
}

#[cfg(target_os = "macos")]
pub(crate) fn swap_app_bundle(new_app: &Path, target_app: &Path) -> Result<()> {
    let prev_app = target_app.with_extension("app.prev");
    if prev_app.exists() {
        std::fs::remove_dir_all(&prev_app)
            .with_context(|| format!("removing stale backup {}", prev_app.display()))?;
    }
    let had_existing = target_app.exists();
    if had_existing {
        std::fs::rename(target_app, &prev_app)
            .with_context(|| format!("moving existing {} aside", target_app.display()))?;
    }
    if let Err(err) = move_dir_across_fs(new_app, target_app) {
        if had_existing
            && prev_app.exists()
            && let Err(rb_err) = std::fs::rename(&prev_app, target_app)
        {
            anyhow::bail!(
                "installing to {} failed ({err:#}); rollback from {} also failed ({rb_err})",
                target_app.display(),
                prev_app.display()
            );
        }
        return Err(err).with_context(|| format!("installing to {}", target_app.display()));
    }
    if prev_app.exists() {
        std::fs::remove_dir_all(&prev_app)
            .with_context(|| format!("cleaning up previous bundle {}", prev_app.display()))?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn move_dir_across_fs(src: &Path, dst: &Path) -> Result<()> {
    if !src.exists() {
        anyhow::bail!("source bundle does not exist: {}", src.display());
    }
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    let out = Command::new("ditto")
        .arg(src)
        .arg(dst)
        .output()
        .context("copying Slackless.app with ditto")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("ditto exited with {}: {}", out.status, stderr.trim());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
async fn download_and_install(
    _release: &ReleaseInfo,
    _restart: bool,
) -> Result<(PathBuf, Option<PathBuf>)> {
    anyhow::bail!("self-update installation is currently supported on macOS only")
}

/// `slackless update [--check] [--force] [--restart] [--beta]`.
pub async fn run_update(
    ctx: &Ctx,
    check_only: bool,
    force: bool,
    restart: bool,
    beta: bool,
) -> Result<ExitCode> {
    let channel = if beta {
        UpdateChannel::Beta
    } else {
        UpdateChannel::Stable
    };
    let release = match updater::fetch_latest_release_on(channel).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            let label = if beta { "beta or stable" } else { "stable" };
            let human = format!(
                "No published {label} release found (Slackless {} is up to date).\n",
                CLI_VERSION
            );
            output::emit(
                ctx.json,
                &serde_json::json!({
                    "ok": true,
                    "channel": channel.as_str(),
                    "current": CLI_VERSION,
                    "latest": serde_json::Value::Null,
                    "prerelease": false,
                    "update_available": false,
                    "updated": false,
                }),
                &human,
            );
            return Ok(ExitCode::SUCCESS);
        }
        Err(err) => return Ok(fail(ctx, "update_check_failed", &err)),
    };
    let available = is_newer_version(&release.version, CLI_VERSION);
    let pre_tag = if release.is_prerelease {
        " (beta/pre-release)"
    } else {
        ""
    };

    if check_only || (!available && !force) {
        let human = if available {
            let cmd = if beta {
                "slackless update --beta"
            } else {
                "slackless update"
            };
            format!(
                "Update available: {} -> {}{pre_tag}\nRelease notes: {}\nRun `{cmd}` to install.\n",
                CLI_VERSION, release.version, release.html_url
            )
        } else {
            format!(
                "Slackless {} is up to date ({} channel).\n",
                CLI_VERSION,
                channel.as_str()
            )
        };
        output::emit(
            ctx.json,
            &serde_json::json!({
                "ok": true,
                "channel": channel.as_str(),
                "current": CLI_VERSION,
                "latest": release.version,
                "prerelease": release.is_prerelease,
                "update_available": available,
                "updated": false,
                "release_url": release.html_url,
            }),
            &human,
        );
        return Ok(ExitCode::SUCCESS);
    }

    if !ctx.json {
        eprintln!(
            "Downloading Slackless {}{pre_tag} (currently {})...",
            release.version, CLI_VERSION
        );
    }
    let (app_path, cli_path) = match download_and_install(&release, restart).await {
        Ok(res) => res,
        Err(err) => return Ok(fail(ctx, "update_install_failed", &format!("{err:#}"))),
    };

    let mut human = format!(
        "Updated Slackless {} -> {}{pre_tag} at {}\n",
        CLI_VERSION,
        release.version,
        app_path.display()
    );
    if let Some(cli) = &cli_path {
        human.push_str(&format!("CLI linked at {}\n", cli.display()));
    }
    if restart {
        human.push_str("Relaunched Slackless.app.\n");
    } else {
        human.push_str("Restart Slackless.app if it is currently running.\n");
    }
    output::emit(
        ctx.json,
        &serde_json::json!({
            "ok": true,
            "channel": channel.as_str(),
            "current": CLI_VERSION,
            "latest": release.version,
            "prerelease": release.is_prerelease,
            "update_available": available,
            "updated": true,
            "restarted": restart,
            "app_path": app_path.display().to_string(),
            "cli_path": cli_path.as_ref().map(|p| p.display().to_string()),
            "release_url": release.html_url,
        }),
        &human,
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    use slackless_core::domain::updater::{
        dir_is_writable, find_sha256_entry, is_safe_asset_name, parse_version,
    };

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "slackless-cli-update-test-{}-{}-{n}",
                label,
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn delegates_version_and_checksum_validation_to_core_updater() {
        assert_eq!(parse_version("2026.11.24-03"), Some((2026, 11, 24, 3)));
        assert_eq!(parse_version("v0.1.0"), Some((0, 1, 0, 0)));
        assert_eq!(parse_version("1.2.3-rc1"), None);
        assert!(is_newer_version("2026.11.24-03", "2026.11.24-02"));
        assert!(!is_newer_version("2026.11.24-02", "2026.11.24-02"));

        assert!(is_safe_asset_name(
            "Slackless-2026.11.24-03-macos-arm64.app.tar.gz"
        ));
        assert!(!is_safe_asset_name("../escape.tar.gz"));

        let hash_upper = "0123456789ABCDEF".repeat(4);
        let sums = format!("{hash_upper} *Slackless-2026.11.24-03-macos-arm64.app.tar.gz\n");
        assert_eq!(
            find_sha256_entry(&sums, "Slackless-2026.11.24-03-macos-arm64.app.tar.gz"),
            Some(hash_upper.to_ascii_lowercase())
        );
    }

    #[test]
    fn cli_parses_update_beta_flag() {
        use clap::Parser as _;
        let parsed = crate::cli::Cli::try_parse_from(["slackless", "update", "--check", "--beta"])
            .expect("parse update --beta");
        match parsed.command {
            crate::cli::Commands::Update(args) => {
                assert!(args.check);
                assert!(args.beta);
                assert!(!args.force);
                assert!(!args.restart);
            }
            _ => panic!("expected Update subcommand"),
        }

        let stable =
            crate::cli::Cli::try_parse_from(["slackless", "update"]).expect("parse default update");
        match stable.command {
            crate::cli::Commands::Update(args) => {
                assert!(!args.beta);
            }
            _ => panic!("expected Update subcommand"),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dir_is_writable_and_bundle_swap_including_rollback() {
        let dir = TempDir::new("swap");
        assert!(dir_is_writable(dir.path()));
        assert!(!dir_is_writable(&dir.path().join("does-not-exist")));
        let plain_file = dir.path().join("file.txt");
        std::fs::write(&plain_file, b"hi").unwrap();
        assert!(!dir_is_writable(&plain_file));

        // Fresh install via swap_app_bundle.
        let stage1 = dir.path().join("stage1/Slackless.app");
        let target = dir.path().join("Apps/Slackless.app");
        std::fs::create_dir_all(stage1.join("Contents")).unwrap();
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(stage1.join("Contents/version"), b"v1").unwrap();

        swap_app_bundle(&stage1, &target).expect("fresh install");
        assert_eq!(
            std::fs::read_to_string(target.join("Contents/version")).unwrap(),
            "v1"
        );
        assert!(!target.with_extension("app.prev").exists());

        // Upgrade replaces v1 with v2 and removes .app.prev.
        let stage2 = dir.path().join("stage2/Slackless.app");
        std::fs::create_dir_all(stage2.join("Contents")).unwrap();
        std::fs::write(stage2.join("Contents/version"), b"v2").unwrap();

        swap_app_bundle(&stage2, &target).expect("upgrade install");
        assert_eq!(
            std::fs::read_to_string(target.join("Contents/version")).unwrap(),
            "v2"
        );
        assert!(!target.with_extension("app.prev").exists());

        // Failed swap (missing new_app) rolls back to v2 intact.
        let missing_stage = dir.path().join("stage3/Missing.app");
        assert!(swap_app_bundle(&missing_stage, &target).is_err());
        assert_eq!(
            std::fs::read_to_string(target.join("Contents/version")).unwrap(),
            "v2"
        );
        assert!(!target.with_extension("app.prev").exists());

        // Direct move_dir_across_fs test.
        let src = dir.path().join("move-src");
        let dst = dir.path().join("move-dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("marker"), b"ok").unwrap();
        move_dir_across_fs(&src, &dst).expect("move dir");
        assert_eq!(std::fs::read_to_string(dst.join("marker")).unwrap(), "ok");
    }
}
