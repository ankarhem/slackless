//! Self-update command (`slackless update`) backed by GitHub Releases.
//!
//! Fetches the latest release metadata from `ankarhem/slackless`, verifies the downloaded macOS
//! `.app.tar.gz` against `SHA256SUMS.txt`, atomically swaps `Slackless.app` (with `.prev`
//! rollback), and refreshes the CLI symlink (`/usr/local/bin/slackless` or
//! `~/.local/bin/slackless`).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::output;
use crate::{CLI_VERSION, Ctx, fail};

const LATEST_RELEASE_API: &str = "https://api.github.com/repos/ankarhem/slackless/releases/latest";

#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub version: String,
    pub html_url: String,
    pub tarball_url: String,
    pub tarball_name: String,
    pub checksums_url: Option<String>,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// Parse a `YYYY.MM.DD-xx` CalVer or `X.Y.Z` semver tag into a comparable tuple.
pub fn parse_version(raw: &str) -> Option<(u32, u32, u32, u32)> {
    let s = raw.trim().trim_start_matches('v');
    let (core, seq) = match s.split_once('-') {
        Some((c, rest)) => {
            let num = rest
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<u32>()
                .unwrap_or(0);
            (c, num)
        }
        None => (s, 0),
    };
    let mut parts = core.split('.');
    let a = parts.next()?.parse::<u32>().ok()?;
    let b = parts.next()?.parse::<u32>().ok()?;
    let c = parts.next()?.parse::<u32>().ok()?;
    Some((a, b, c, seq))
}

pub fn is_newer_version(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => latest != current,
    }
}

fn fetch_latest_release() -> Result<ReleaseInfo> {
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "--connect-timeout",
            "8",
            "--max-time",
            "20",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: slackless-cli-updater",
            LATEST_RELEASE_API,
        ])
        .output()
        .context("running curl to check GitHub Releases")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("GitHub Releases check failed: {}", stderr.trim());
    }
    let rel: GhRelease =
        serde_json::from_slice(&out.stdout).context("parsing GitHub Releases response")?;
    let tarball = rel
        .assets
        .iter()
        .find(|a| a.name.ends_with("-macos-arm64.app.tar.gz"))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "release {} has no macOS arm64 .app.tar.gz asset",
                rel.tag_name
            )
        })?;
    let checksums = rel
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS.txt")
        .map(|a| a.browser_download_url.clone());
    Ok(ReleaseInfo {
        version: rel.tag_name,
        html_url: rel.html_url,
        tarball_url: tarball.browser_download_url.clone(),
        tarball_name: tarball.name.clone(),
        checksums_url: checksums,
    })
}

#[cfg(target_os = "macos")]
fn resolve_target_app_bundle() -> PathBuf {
    // 1. If this CLI binary lives inside an installed `Slackless.app`, update that exact bundle.
    if let Ok(exe) = std::env::current_exe().and_then(std::fs::canonicalize) {
        for ancestor in exe.ancestors() {
            if ancestor
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("app"))
            {
                return ancestor.to_path_buf();
            }
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let user_app = home.join("Applications/Slackless.app");
    let sys_app = PathBuf::from("/Applications/Slackless.app");
    if user_app.is_dir() {
        return user_app;
    }
    if sys_app.is_dir() {
        return sys_app;
    }
    if dir_is_writable(Path::new("/Applications")) {
        sys_app
    } else {
        user_app
    }
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
fn resolve_cli_link_path() -> PathBuf {
    let usr_local = PathBuf::from("/usr/local/bin");
    if dir_is_writable(&usr_local) {
        return usr_local.join("slackless");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".local/bin/slackless")
}

#[cfg(target_os = "macos")]
fn download_and_install(
    release: &ReleaseInfo,
    restart: bool,
) -> Result<(PathBuf, Option<PathBuf>)> {
    let target_app = resolve_target_app_bundle();
    if let Some(parent) = target_app.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let stage_dir = std::env::temp_dir().join(format!(
        "slackless-cli-update-{}-{}",
        std::process::id(),
        release.version
    ));
    let _ = std::fs::remove_dir_all(&stage_dir);
    std::fs::create_dir_all(&stage_dir).context("creating update staging directory")?;

    let install_result = (|| -> Result<(PathBuf, Option<PathBuf>)> {
        let tarball_path = stage_dir.join(&release.tarball_name);
        let dl = Command::new("curl")
            .args(["-fsSL", "--max-time", "300", "-o"])
            .arg(&tarball_path)
            .arg(&release.tarball_url)
            .output()
            .context("downloading release tarball")?;
        if !dl.status.success() {
            anyhow::bail!(
                "download failed: {}",
                String::from_utf8_lossy(&dl.stderr).trim()
            );
        }

        if let Some(sums_url) = &release.checksums_url {
            let sums_path = stage_dir.join("SHA256SUMS.txt");
            let sums_dl = Command::new("curl")
                .args(["-fsSL", "--max-time", "30", "-o"])
                .arg(&sums_path)
                .arg(sums_url)
                .output()
                .context("downloading SHA256SUMS.txt")?;
            if sums_dl.status.success() {
                let sums_text = std::fs::read_to_string(&sums_path).unwrap_or_default();
                if let Some(expected) = sums_text.lines().find_map(|line| {
                    let mut parts = line.split_whitespace();
                    let hash = parts.next()?;
                    let name = parts.next()?.trim_start_matches('*');
                    (name == release.tarball_name).then(|| hash.to_ascii_lowercase())
                }) {
                    let actual_out = Command::new("shasum")
                        .args(["-a", "256"])
                        .arg(&tarball_path)
                        .output()
                        .context("computing SHA-256 checksum")?;
                    let actual = String::from_utf8_lossy(&actual_out.stdout)
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    if actual != expected {
                        anyhow::bail!(
                            "SHA-256 checksum mismatch (expected {expected}, got {actual})"
                        );
                    }
                }
            }
        }

        let unpack = Command::new("tar")
            .args(["-xzf"])
            .arg(&tarball_path)
            .arg("-C")
            .arg(&stage_dir)
            .output()
            .context("extracting Slackless.app")?;
        if !unpack.status.success() {
            anyhow::bail!(
                "extract failed: {}",
                String::from_utf8_lossy(&unpack.stderr).trim()
            );
        }
        let new_app = stage_dir.join("Slackless.app");
        if !new_app.is_dir() {
            anyhow::bail!("archive did not contain Slackless.app");
        }
        let _ = Command::new("xattr")
            .args(["-dr", "com.apple.quarantine"])
            .arg(&new_app)
            .status();

        // Atomic swap with `.prev` rollback.
        let prev_app = target_app.with_extension("app.prev");
        let _ = std::fs::remove_dir_all(&prev_app);
        let had_existing = target_app.exists();
        if had_existing {
            std::fs::rename(&target_app, &prev_app)
                .with_context(|| format!("moving existing {} aside", target_app.display()))?;
        }
        if let Err(err) = move_dir_across_fs(&new_app, &target_app) {
            if had_existing && prev_app.exists() {
                let _ = std::fs::rename(&prev_app, &target_app);
            }
            return Err(err).with_context(|| format!("installing to {}", target_app.display()));
        }
        let _ = std::fs::remove_dir_all(&prev_app);

        // Refresh CLI symlink to point at the newly installed bundle.
        let bundled_cli = target_app.join("Contents/MacOS/slackless");
        let linked_cli = if bundled_cli.is_file() {
            let link = resolve_cli_link_path();
            if let Some(parent) = link.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::remove_file(&link);
            if std::os::unix::fs::symlink(&bundled_cli, &link).is_ok() {
                Some(link)
            } else {
                None
            }
        } else {
            None
        };

        let _ = Command::new(
            "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
        )
        .args(["-f"])
        .arg(&target_app)
        .status();

        if restart {
            let _ = Command::new("osascript")
                .args(["-e", "tell application \"Slackless\" to quit"])
                .status();
            std::thread::sleep(std::time::Duration::from_millis(600));
            let _ = Command::new("open").arg(&target_app).status();
        }

        Ok((target_app, linked_cli))
    })();

    let _ = std::fs::remove_dir_all(&stage_dir);
    install_result
}

#[cfg(target_os = "macos")]
fn move_dir_across_fs(src: &Path, dst: &Path) -> Result<()> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    let status = Command::new("ditto")
        .arg(src)
        .arg(dst)
        .status()
        .context("copying Slackless.app with ditto")?;
    if !status.success() {
        anyhow::bail!("ditto exited with {status}");
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn download_and_install(
    _release: &ReleaseInfo,
    _restart: bool,
) -> Result<(PathBuf, Option<PathBuf>)> {
    anyhow::bail!("self-update installation is currently supported on macOS only")
}

/// `slackless update [--check] [--force] [--restart]`.
pub fn run_update(ctx: &Ctx, check_only: bool, force: bool, restart: bool) -> Result<ExitCode> {
    let release = match fetch_latest_release() {
        Ok(r) => r,
        Err(err) => return Ok(fail(ctx, "update_check_failed", &format!("{err:#}"))),
    };
    let available = is_newer_version(&release.version, CLI_VERSION);

    if check_only || (!available && !force) {
        let human = if available {
            format!(
                "Update available: {} -> {}\nRelease notes: {}\nRun `slackless update` to install.\n",
                CLI_VERSION, release.version, release.html_url
            )
        } else {
            format!("Slackless {} is up to date.\n", CLI_VERSION)
        };
        output::emit(
            ctx.json,
            &serde_json::json!({
                "ok": true,
                "current": CLI_VERSION,
                "latest": release.version,
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
            "Downloading Slackless {} (currently {})...",
            release.version, CLI_VERSION
        );
    }
    let (app_path, cli_path) = match download_and_install(&release, restart) {
        Ok(res) => res,
        Err(err) => return Ok(fail(ctx, "update_install_failed", &format!("{err:#}"))),
    };

    let mut human = format!(
        "Updated Slackless {} -> {} at {}\n",
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
            "current": CLI_VERSION,
            "latest": release.version,
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

    #[test]
    fn parses_calver_and_semver_versions() {
        assert_eq!(parse_version("2026.11.24-03"), Some((2026, 11, 24, 3)));
        assert_eq!(parse_version("v0.1.0"), Some((0, 1, 0, 0)));
        assert!(is_newer_version("2026.11.24-03", "2026.11.24-02"));
        assert!(is_newer_version("2026.11.24-01", "0.1.0"));
        assert!(!is_newer_version("2026.11.24-02", "2026.11.24-02"));
        assert!(!is_newer_version("2026.11.24-01", "2026.11.24-02"));
    }
}
