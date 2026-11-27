//! Self-update command (`slackless update`) backed by GitHub Releases.
//!
//! Delegates version comparison, GitHub release fetching (filtering out drafts and prereleases),
//! pure-Rust HTTPS download & SHA-256 verification, archive extraction, code-signature
//! verification against the installed app, macOS install-path resolution, the `.app` swap with
//! `.app.prev` rollback, and CLI symlink refresh to `slackless_core::domain::updater`. Safely
//! stops a running `Slackless.app` before swapping on `--restart` and relaunches it afterwards.

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

/// Executable name of the GUI inside `Slackless.app` (`CFBundleExecutable` in
/// `scripts/bundle-macos.sh`). `pgrep -x` matches executable names, so the bundle's display name
/// `Slackless` would never match the running app.
#[cfg(target_os = "macos")]
const APP_PROCESS_NAME: &str = "slackless-gui";

#[cfg(target_os = "macos")]
fn is_slackless_app_running() -> bool {
    Command::new("/usr/bin/pgrep")
        .args(["-x", APP_PROCESS_NAME])
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

/// Sanitize a release version string before embedding it in a staging directory name so untrusted
/// characters or path traversal segments (`../`, `/`) can never escape the staging root.
#[cfg(any(test, target_os = "macos"))]
fn sanitize_stage_component(version: &str) -> String {
    let cleaned: String = version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() || cleaned.contains("..") || cleaned.starts_with('.') {
        "release".to_owned()
    } else {
        cleaned
    }
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

    let stage_root = target_app
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    let safe_version = sanitize_stage_component(&release.version);
    let stage_dir = stage_root.join(format!(
        ".slackless-cli-update-{}-{safe_version}",
        std::process::id(),
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

    // Refuse an update that is not signed like the installed app, before quitting anything.
    let reference = updater::update_signature_reference(target_app);
    updater::verify_update_signature(&new_app, reference.as_deref())
        .map_err(|err| anyhow::anyhow!(err))?;

    // Stop a running Slackless.app and verify process exit BEFORE swapping the bundle on disk.
    if restart {
        quit_running_app_and_wait()?;
    }

    // Atomic swap with `.prev` rollback.
    updater::swap_app_bundle(&new_app, target_app).map_err(|err| anyhow::anyhow!(err))?;

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
    use slackless_core::domain::updater::{find_sha256_entry, is_safe_asset_name, parse_version};

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

    /// Child half of [`quit_detection_matches_the_bundle_executable_name`]: with the env marker
    /// set, announce on stdout and block until stdin closes; returns at once otherwise.
    #[test]
    fn pgrep_probe_child() {
        if std::env::var_os("SLACKLESS_PGREP_PROBE").is_none() {
            return;
        }
        println!("ready");
        let _ = std::io::stdin().read_line(&mut String::new());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn quit_detection_matches_the_bundle_executable_name() {
        use std::io::{BufRead, BufReader};
        use std::process::Stdio;

        // A real Slackless.app on this machine would make the "not running" checks meaningless.
        if is_slackless_app_running() {
            eprintln!("skipping: a {APP_PROCESS_NAME} process is already running");
            return;
        }

        // Run this test binary again under the bundle's executable name (CFBundleExecutable in
        // scripts/bundle-macos.sh; hard link where the volumes allow, else a copy): `pgrep -x`
        // matches the executable name of a process, whatever APP_PROCESS_NAME claims.
        let dir = std::env::temp_dir().join(format!("slackless-cli-pgrep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let exe = std::env::current_exe().expect("current exe");
        let probe = dir.join("slackless-gui");
        if std::fs::hard_link(&exe, &probe).is_err() {
            std::fs::copy(&exe, &probe).expect("copy test binary");
        }

        let mut child = Command::new(&probe)
            .args(["--exact", "update::tests::pgrep_probe_child", "--nocapture"])
            .env("SLACKLESS_PGREP_PROBE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn probe");
        let stdout = child.stdout.take().expect("piped stdout");
        let ready = BufReader::new(stdout)
            .lines()
            .any(|line| line.is_ok_and(|line| line == "ready"));
        assert!(ready, "probe must come up and announce itself");
        assert!(
            is_slackless_app_running(),
            "pgrep must find a process named {APP_PROCESS_NAME}"
        );

        drop(child.stdin.take());
        child.wait().expect("probe exits once stdin closes");
        assert!(
            !is_slackless_app_running(),
            "pgrep must not find the exited probe"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_stage_component_rejects_traversal_and_special_chars() {
        assert_eq!(sanitize_stage_component("2026.11.27-01"), "2026.11.27-01");
        assert_eq!(sanitize_stage_component("../../etc/passwd"), "release");
        assert_eq!(sanitize_stage_component(".hidden"), "release");
        assert_eq!(sanitize_stage_component(""), "release");
        assert_eq!(sanitize_stage_component("v1.0/bad name"), "v1.0_bad_name");
    }
}
