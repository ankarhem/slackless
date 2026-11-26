//! Shared GitHub Releases update checker, version comparator, checksum parser, install-path
//! resolver, code-signature verification and app-bundle swap used by both CLI (`slackless
//! update`) and GUI (`Check for Updates`).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::api::AssetClient;

/// GitHub repository hosting official Slackless releases.
pub const RELEASES_REPO: &str = "ankarhem/slackless";

/// GitHub REST endpoint for the latest official Slackless release.
pub const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/ankarhem/slackless/releases/latest";

/// GitHub REST endpoint for listing Slackless releases (used by the Beta channel).
pub const RELEASES_LIST_API: &str =
    "https://api.github.com/repos/ankarhem/slackless/releases?per_page=20";

/// Release channel for the Slackless auto-updater (`F-SHELL-164`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UpdateChannel {
    /// Only published, non-prerelease GitHub Releases (`/releases/latest`).
    #[default]
    Stable,
    /// Latest non-draft GitHub Release, including beta / pre-release builds (`/releases`).
    Beta,
}

impl UpdateChannel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
        }
    }

    pub const fn is_beta(self) -> bool {
        matches!(self, Self::Beta)
    }
}

const DEFAULT_USER_AGENT: &str = concat!("Slackless/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const METADATA_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// Largest release archive accepted; current archives are tens of MB.
const MAX_RELEASE_BYTES: u64 = 1024 * 1024 * 1024;

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
    /// True when this release is marked as a GitHub pre-release (Beta channel, `F-SHELL-164`).
    pub is_prerelease: bool,
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

/// Comparable version tuple supporting optional pre-release rank (e.g. `-alpha.1`, `-beta.2`,
/// `-rc.1`). Stable builds sort strictly higher than pre-releases sharing the same base version.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ComparableVersion {
    base: (u32, u32, u32, u32),
    /// `true` for stable releases, `false` for pre-releases (`false < true`).
    is_stable: bool,
    /// Pre-release stage (`alpha` < `beta` < `rc` < other).
    pre_stage: u8,
    /// Trailing numeric counter inside the pre-release tag (e.g. `beta.2` -> `2`).
    pre_num: u32,
}

fn parse_prerelease_tag(tag: &str) -> Option<(u8, u32)> {
    let lower = tag.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }
    let (stage, rest) = if let Some(r) = lower.strip_prefix("alpha") {
        (1, r)
    } else if let Some(r) = lower.strip_prefix("beta") {
        (2, r)
    } else if let Some(r) = lower.strip_prefix("rc") {
        (3, r)
    } else {
        let r = lower.strip_prefix("pre")?;
        (2, r)
    };
    let rest = rest.trim_start_matches(['.', '-']);
    let num = if rest.is_empty() {
        0
    } else {
        rest.parse::<u32>().ok()?
    };
    Some((stage, num))
}

fn parse_comparable_version(raw: &str) -> Option<ComparableVersion> {
    if let Some(base) = parse_version(raw) {
        return Some(ComparableVersion {
            base,
            is_stable: true,
            pre_stage: u8::MAX,
            pre_num: u32::MAX,
        });
    }
    let s = raw.trim().trim_start_matches('v');
    let (lhs, pre) = s.rsplit_once('-')?;
    let (pre_stage, pre_num) = parse_prerelease_tag(pre)?;
    let base = parse_version(lhs)?;
    Some(ComparableVersion {
        base,
        is_stable: false,
        pre_stage,
        pre_num,
    })
}

/// Returns `true` if `latest` is strictly newer than `current` (supporting both stable and
/// pre-release tags such as `2026.11.25-01-beta.2` or `v1.2.3-rc.1`).
pub fn is_newer_version(latest: &str, current: &str) -> bool {
    match (
        parse_comparable_version(latest),
        parse_comparable_version(current),
    ) {
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

fn release_from_gh(rel: GhRelease, channel: UpdateChannel) -> Result<Option<ReleaseInfo>, String> {
    if rel.draft || (rel.prerelease && !channel.is_beta()) {
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
        is_prerelease: rel.prerelease,
    }))
}

/// Parse a GitHub Releases JSON payload into [`ReleaseInfo`] for `channel`.
/// Draft payloads always return `Ok(None)`; pre-release payloads return `Ok(None)` on
/// [`UpdateChannel::Stable`] and `Ok(Some(_))` on [`UpdateChannel::Beta`].
pub fn parse_github_release_for_channel(
    payload: &[u8],
    channel: UpdateChannel,
) -> Result<Option<ReleaseInfo>, String> {
    let rel: GhRelease =
        serde_json::from_slice(payload).map_err(|e| format!("invalid GitHub release JSON: {e}"))?;
    release_from_gh(rel, channel)
}

/// Parse a GitHub Releases JSON payload into [`ReleaseInfo`]. Draft and prerelease payloads return
/// `Ok(None)` so neither CLI nor GUI accidentally installs an unpublished build.
pub fn parse_github_release(payload: &[u8]) -> Result<Option<ReleaseInfo>, String> {
    parse_github_release_for_channel(payload, UpdateChannel::Stable)
}

/// Parse a GitHub `/releases` array payload, selecting the newest non-draft release matching
/// `channel` (including pre-releases when [`UpdateChannel::Beta`] is selected, skipping drafts
/// and releases lacking a macOS arm64 archive).
pub fn parse_github_releases_list(
    payload: &[u8],
    channel: UpdateChannel,
) -> Result<Option<ReleaseInfo>, String> {
    let releases: Vec<GhRelease> = serde_json::from_slice(payload)
        .map_err(|e| format!("invalid GitHub releases list JSON: {e}"))?;
    let mut best: Option<ReleaseInfo> = None;
    for rel in releases {
        if rel.draft || (rel.prerelease && !channel.is_beta()) {
            continue;
        }
        let Ok(Some(info)) = release_from_gh(rel, channel) else {
            continue;
        };
        let replace = match &best {
            None => true,
            Some(prev) => match (
                parse_comparable_version(&info.version),
                parse_comparable_version(&prev.version),
            ) {
                (Some(cand), Some(cur)) => cand > cur,
                (Some(_), None) => true,
                _ => false,
            },
        };
        if replace {
            best = Some(info);
        }
    }
    Ok(best)
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
    crate::api::install_default_crypto_provider();
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

async fn fetch_release_from_url_for_channel(
    url: &str,
    channel: UpdateChannel,
    is_list: bool,
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
    if is_list {
        parse_github_releases_list(&bytes, channel)
    } else {
        parse_github_release_for_channel(&bytes, channel)
    }
}

/// Query GitHub Releases for `repo` on `channel` (`Stable` -> `/releases/latest`, `Beta` ->
/// `/releases?per_page=20` including pre-releases; `F-SHELL-164`).
pub async fn fetch_latest_release_for_channel(
    repo: &str,
    channel: UpdateChannel,
    user_agent: &str,
) -> Result<Option<ReleaseInfo>, String> {
    match channel {
        UpdateChannel::Stable => {
            let url = format!("https://api.github.com/repos/{repo}/releases/latest");
            fetch_release_from_url_for_channel(&url, channel, false, user_agent).await
        }
        UpdateChannel::Beta => {
            let url = format!("https://api.github.com/repos/{repo}/releases?per_page=20");
            fetch_release_from_url_for_channel(&url, channel, true, user_agent).await
        }
    }
}

/// Query GitHub Releases for the official Slackless repository on `channel` (`F-SHELL-164`).
pub async fn fetch_latest_release_on(
    channel: UpdateChannel,
) -> Result<Option<ReleaseInfo>, String> {
    fetch_latest_release_for_channel(RELEASES_REPO, channel, DEFAULT_USER_AGENT).await
}

/// Blocking wrapper around [`fetch_latest_release_for_channel`] for non-Tokio callers (e.g. GPUI
/// background executor threads).
pub fn fetch_latest_release_for_channel_blocking(
    repo: &str,
    channel: UpdateChannel,
    user_agent: &str,
) -> Result<Option<ReleaseInfo>, String> {
    crate::runtime::block_on(fetch_latest_release_for_channel(repo, channel, user_agent))
        .map_err(|e| format!("failed to start async runtime: {e}"))?
}

/// Download `release`'s archive (streamed through a `.part` file, 15s connect / 300s request
/// timeout) and `SHA256SUMS.txt` into `staging_dir`, verify the archive's SHA-256 checksum in pure
/// Rust via [`sha256_file`] and [`find_sha256_entry`], and return the verified tarball path.
/// Refuses unverified updates if `sha256_url` is missing.
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
    AssetClient::with_http_client(build_updater_client(user_agent, DOWNLOAD_TIMEOUT)?)
        .download_to_path(
            &release.download_url,
            &tarball_path,
            MAX_RELEASE_BYTES,
            |_, _| {},
        )
        .await
        .map_err(|e| format!("download failed: {e}"))?;

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
/// background executor threads): pure-Rust SHA-256 verification on the shared async runtime.
pub fn download_and_verify_release_blocking(
    release: &ReleaseInfo,
    staging_dir: &Path,
    user_agent: &str,
) -> Result<PathBuf, String> {
    crate::runtime::block_on(download_and_verify_release(
        release,
        staging_dir,
        user_agent,
    ))
    .map_err(|e| format!("failed to start async runtime: {e}"))?
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

/// What the installed app's designated requirement demands of an update bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureRequirement {
    /// The installed bundle is signed with a certificate; the update must satisfy exactly this
    /// designated requirement (`codesign -R=<requirement>`), i.e. come from the same identity.
    Designated(String),
    /// No certificate-based requirement is available (ad-hoc / `cdhash`-only installed bundle, or
    /// nothing installed yet): the update's own signature must still verify intact, but its
    /// identity is trusted to the SHA-256 checksum alone.
    ChecksumOnly,
}

/// Extract the designated requirement from the output of `codesign -d -r-`, which prints
/// `designated => <requirement>` (`# designated => ...` in verbose mode). Returns `None` when no
/// designated requirement is present (unsigned or unreadable code).
pub fn parse_designated_requirement(codesign_output: &str) -> Option<String> {
    codesign_output.lines().find_map(|line| {
        let line = line.trim();
        let line = line.strip_prefix("# ").unwrap_or(line);
        line.strip_prefix("designated => ")
            .map(str::trim)
            .filter(|req| !req.is_empty())
            .map(str::to_owned)
    })
}

/// Decide how an update must be verified from the designated requirement of the installed
/// (running) bundle. Requirements naming a certificate or an anchor (e.g.
/// `identifier "dev.slackless.gui" and certificate leaf = H"…"`) are enforced as-is; ad-hoc
/// `cdhash H"…"` or no requirement at all leaves no identity to pin to, so the checksum stays the
/// only identity check.
pub fn signature_requirement(designated: Option<&str>) -> SignatureRequirement {
    match designated.map(str::trim) {
        Some(req) if req.contains("certificate") || req.contains("anchor") => {
            SignatureRequirement::Designated(req.to_owned())
        }
        _ => SignatureRequirement::ChecksumOnly,
    }
}

/// The bundle whose designated requirement an update must match: the running app bundle when
/// launched from one (GUI), else the installed `target_app` when there is one (CLI), else nothing.
pub fn update_signature_reference(target_app: &Path) -> Option<PathBuf> {
    current_app_bundle()
        .filter(|running| running.is_dir())
        .or_else(|| target_app.is_dir().then(|| target_app.to_path_buf()))
}

#[cfg(target_os = "macos")]
const CODESIGN: &str = "/usr/bin/codesign";

/// Read the designated requirement of `app` with `/usr/bin/codesign -d -r-`. `None` when `codesign`
/// reports no designated requirement (unsigned, missing or malformed bundle).
#[cfg(target_os = "macos")]
pub fn designated_requirement_of(app: &Path) -> Option<String> {
    let out = Command::new(CODESIGN)
        .args(["-d", "-r-"])
        .arg(app)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_designated_requirement(&String::from_utf8_lossy(&out.stdout))
}

/// Run `/usr/bin/codesign --verify --deep --strict` on `app`, optionally also requiring it to
/// satisfy `requirement` (`-R=<text>`). Fails with `codesign`'s stderr when the bundle is unsigned,
/// tampered with, or signed by a different identity than the requirement names.
#[cfg(target_os = "macos")]
fn codesign_verify(app: &Path, requirement: Option<&str>) -> Result<(), String> {
    let mut cmd = Command::new(CODESIGN);
    cmd.args(["--verify", "--deep", "--strict"]);
    if let Some(requirement) = requirement {
        cmd.arg(format!("-R={requirement}"));
    }
    let out = cmd
        .arg(app)
        .output()
        .map_err(|e| format!("failed to run codesign on {}: {e}", app.display()))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
    Err(if stderr.is_empty() {
        format!("codesign exited with {} for {}", out.status, app.display())
    } else {
        stderr
    })
}

/// Verify the extracted update bundle `new_app` after extraction and before any swap, against
/// `reference_app` (the running or installed `Slackless.app`, see
/// [`update_signature_reference`]):
///
/// 1. `/usr/bin/codesign --verify --deep --strict <new_app>` must pass: the bundle is signed and
///    intact (the SHA-256 checksum from the same release only proves it is what it claims to be).
/// 2. When the reference bundle carries a certificate-based designated requirement, the same
///    `-R=<requirement>` must pass for `new_app`; an update signed by anyone else is refused.
/// 3. Without a certificate-based reference (ad-hoc / `cdhash`-only install, or no install yet),
///    a warning is logged and the checksum remains the only identity check.
#[cfg(target_os = "macos")]
pub fn verify_update_signature(new_app: &Path, reference_app: Option<&Path>) -> Result<(), String> {
    let designated = reference_app.and_then(designated_requirement_of);
    verify_signature_requirement(new_app, signature_requirement(designated.as_deref()))
}

#[cfg(target_os = "macos")]
fn verify_signature_requirement(
    new_app: &Path,
    requirement: SignatureRequirement,
) -> Result<(), String> {
    codesign_verify(new_app, None)?;
    match requirement {
        SignatureRequirement::Designated(requirement) => {
            codesign_verify(new_app, Some(&requirement))
                .map_err(|e| format!("update is not signed like the installed app: {e}"))
        }
        SignatureRequirement::ChecksumOnly => {
            // shortcut: identity check is checksum-only while the installed app is ad-hoc signed or
            // missing, upgrade once every install carries a certificate-based designated requirement
            tracing::warn!(
                new_app = %new_app.display(),
                "installed app has no certificate-based designated requirement; verifying the update by checksum only"
            );
            Ok(())
        }
    }
}

/// Replace `target_app` with `new_app`: move an existing `target_app` aside to
/// `<target_app>.prev` (e.g. `Slackless.app.prev`, after dropping any stale backup), move the new
/// bundle into place (copying with `ditto` when the two live on different volumes), then remove
/// the backup. When the move fails the previous bundle is moved back and the error returned, so
/// the installed app is never left missing. Renaming the bundle of a running app is fine on macOS.
#[cfg(target_os = "macos")]
pub fn swap_app_bundle(new_app: &Path, target_app: &Path) -> Result<(), String> {
    let prev_app = target_app.with_extension("app.prev");
    if prev_app.exists()
        && let Err(e) = std::fs::remove_dir_all(&prev_app)
    {
        return Err(format!(
            "cannot remove stale backup {}: {e}",
            prev_app.display()
        ));
    }
    let had_existing = target_app.exists();
    if had_existing {
        std::fs::rename(target_app, &prev_app)
            .map_err(|e| format!("cannot move {} aside: {e}", target_app.display()))?;
    }
    if let Err(err) = move_dir_across_fs(new_app, target_app) {
        if had_existing
            && prev_app.exists()
            && let Err(rb_err) = std::fs::rename(&prev_app, target_app)
        {
            return Err(format!(
                "cannot install {} ({err}); restoring {} also failed ({rb_err})",
                target_app.display(),
                prev_app.display()
            ));
        }
        return Err(err);
    }
    if prev_app.exists()
        && let Err(e) = std::fs::remove_dir_all(&prev_app)
    {
        tracing::warn!(error = %e, backup = %prev_app.display(), "installed update but could not remove previous bundle");
    }
    Ok(())
}

/// Move directory `src` to `dst`, falling back to `/usr/bin/ditto` (which preserves metadata and
/// resource forks) when the two are on different volumes and `rename` therefore fails.
#[cfg(target_os = "macos")]
fn move_dir_across_fs(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() {
        return Err(format!("source bundle does not exist: {}", src.display()));
    }
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    let out = Command::new("/usr/bin/ditto")
        .arg(src)
        .arg(dst)
        .output()
        .map_err(|e| format!("failed to run ditto: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ditto exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let _ = std::fs::remove_dir_all(src);
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
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    /// Per-test scratch directory under the temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "slackless-core-updater-{label}-{}-{}",
                std::process::id(),
                TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
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
    fn parses_designated_requirements_and_classifies_them() {
        // `codesign -d -r-` stdout for a certificate-signed Slackless.app.
        let cert_based = "designated => identifier \"dev.slackless.gui\" and certificate leaf = H\"3f2e79ed498717dce5f8b5074fee9bc924ee3885\"\n";
        let cert_requirement = "identifier \"dev.slackless.gui\" and certificate leaf = H\"3f2e79ed498717dce5f8b5074fee9bc924ee3885\"";
        assert_eq!(
            parse_designated_requirement(cert_based).as_deref(),
            Some(cert_requirement)
        );
        assert_eq!(
            signature_requirement(Some(cert_requirement)),
            SignatureRequirement::Designated(cert_requirement.to_owned())
        );

        // Ad-hoc signed (universal binary): `cdhash`-only, nothing to pin the identity to.
        let adhoc = "designated => cdhash H\"2309b035501b9492e21770fbb06dad25dffc6718\" or cdhash H\"6009d85589bbb3ff435b37c56c34d5c9ec54a021\" or cdhash H\"4e44e246e3242c993cea3ca9e816b20a42149afc\"\n";
        assert!(
            parse_designated_requirement(adhoc).is_some_and(|req| req.starts_with("cdhash H\""))
        );
        assert_eq!(
            signature_requirement(parse_designated_requirement(adhoc).as_deref()),
            SignatureRequirement::ChecksumOnly
        );

        // Verbose output prefixes the line with `# `; Apple's own anchor counts as identity too.
        assert_eq!(
            parse_designated_requirement("Executable=/bin/ls\n# designated => anchor apple\n")
                .as_deref(),
            Some("anchor apple")
        );
        assert_eq!(
            signature_requirement(Some("anchor apple")),
            SignatureRequirement::Designated("anchor apple".to_owned())
        );

        // Unsigned code prints no designated requirement at all.
        assert_eq!(parse_designated_requirement(""), None);
        assert_eq!(parse_designated_requirement("designated => \n"), None);
        assert_eq!(
            signature_requirement(None),
            SignatureRequirement::ChecksumOnly
        );
    }

    #[cfg(target_os = "macos")]
    fn adhoc_signed_bundle(at: &Path) {
        let macos = at.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).expect("create bundle");
        std::fs::copy("/bin/sleep", macos.join("slackless-gui")).expect("copy a Mach-O");
        std::fs::write(
            at.join("Contents/Info.plist"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>CFBundleExecutable</key><string>slackless-gui</string><key>CFBundleIdentifier</key><string>dev.slackless.test</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>\n",
        )
        .expect("write Info.plist");
        let out = Command::new(CODESIGN)
            .args(["--force", "--sign", "-"])
            .arg(at)
            .output()
            .expect("run codesign");
        assert!(
            out.status.success(),
            "codesign failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn verify_update_signature_checks_integrity_then_installed_identity() {
        let dir = TempDir::new("signature");
        let new_app = dir.path().join("stage/Slackless.app");
        let installed = dir.path().join("Apps/Slackless.app");
        adhoc_signed_bundle(&new_app);
        adhoc_signed_bundle(&installed);

        // The reference is the installed bundle when not running from one, nothing when there is
        // no install yet.
        assert_eq!(
            update_signature_reference(&installed),
            Some(installed.clone())
        );
        assert_eq!(
            update_signature_reference(&dir.path().join("missing/Slackless.app")),
            None
        );

        // An ad-hoc (cdhash-only) install pins no identity: the intact update passes, as does an
        // update with no install to compare against.
        assert!(
            designated_requirement_of(&installed).is_some_and(|req| req.starts_with("cdhash H\""))
        );
        verify_update_signature(&new_app, Some(&installed))
            .expect("ad-hoc install falls back to checksum-only");
        verify_update_signature(&new_app, None).expect("no install yet");
        assert_eq!(
            designated_requirement_of(&dir.path().join("nope.app")),
            None
        );

        // A certificate-based requirement is enforced: an update not signed by that identity is
        // refused, while the bundle's own requirement passes.
        let cert = "identifier \"dev.slackless.gui\" and certificate leaf = H\"3f2e79ed498717dce5f8b5074fee9bc924ee3885\"";
        assert!(
            verify_signature_requirement(
                &new_app,
                SignatureRequirement::Designated(cert.to_owned())
            )
            .is_err()
        );
        let own = designated_requirement_of(&new_app).expect("ad-hoc DR");
        verify_signature_requirement(&new_app, SignatureRequirement::Designated(own))
            .expect("bundle satisfies its own requirement");

        // A tampered bundle fails the integrity check before any identity check, unsigned code
        // fails it as well.
        std::fs::write(new_app.join("Contents/Info.plist"), "broken").expect("tamper");
        assert!(verify_update_signature(&new_app, Some(&installed)).is_err());
        let unsigned = dir.path().join("unsigned/Slackless.app");
        std::fs::create_dir_all(unsigned.join("Contents/MacOS")).expect("mkdir unsigned");
        std::fs::copy("/bin/sleep", unsigned.join("Contents/MacOS/slackless-gui")).expect("copy");
        assert!(verify_update_signature(&unsigned, None).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn swap_app_bundle_installs_upgrades_and_rolls_back() {
        let dir = TempDir::new("swap");
        let target = dir.path().join("Apps/Slackless.app");
        let prev = target.with_extension("app.prev");
        std::fs::create_dir_all(target.parent().expect("parent")).expect("mkdir Apps");
        let staged = |version: &str| {
            let app = dir.path().join(format!("stage-{version}/Slackless.app"));
            std::fs::create_dir_all(app.join("Contents")).expect("mkdir stage");
            std::fs::write(app.join("Contents/version"), version).expect("write version");
            app
        };
        let installed =
            || std::fs::read_to_string(target.join("Contents/version")).expect("version");

        // Fresh install via swap_app_bundle.
        swap_app_bundle(&staged("v1"), &target).expect("fresh install");
        assert_eq!(installed(), "v1");
        assert!(!prev.exists());

        // A stale backup left by an interrupted update is dropped; the upgrade replaces v1 with v2.
        std::fs::create_dir_all(&prev).expect("mkdir stale backup");
        std::fs::write(prev.join("stale"), b"").expect("write stale");
        swap_app_bundle(&staged("v2"), &target).expect("upgrade install");
        assert_eq!(installed(), "v2");
        assert!(!prev.exists());

        // Failed swap (missing new_app) rolls back to v2 intact.
        assert!(swap_app_bundle(&dir.path().join("stage-missing/Slackless.app"), &target).is_err());
        assert_eq!(installed(), "v2");
        assert!(!prev.exists());

        // Plain directory move (rename) and a missing source.
        let src = dir.path().join("move-src");
        let dst = dir.path().join("move-dst");
        std::fs::create_dir_all(&src).expect("mkdir move-src");
        std::fs::write(src.join("marker"), b"ok").expect("write marker");
        move_dir_across_fs(&src, &dst).expect("move dir");
        assert_eq!(
            std::fs::read_to_string(dst.join("marker")).expect("marker"),
            "ok"
        );
        assert!(!src.exists());
        assert!(move_dir_across_fs(&src, &dir.path().join("move-dst-2")).is_err());
    }

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
        let file = dir.join("file.txt");
        std::fs::write(&file, b"hi").expect("write file");
        assert!(!dir_is_writable(&file));
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
            is_prerelease: false,
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
            is_prerelease: false,
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

    #[test]
    fn beta_channel_parses_prereleases_and_compares_versions_accurately() {
        assert_eq!(UpdateChannel::Stable.as_str(), "stable");
        assert_eq!(UpdateChannel::Beta.as_str(), "beta");
        assert!(!UpdateChannel::Stable.is_beta());
        assert!(UpdateChannel::Beta.is_beta());

        // Pre-release version comparisons (`F-SHELL-164`).
        assert!(is_newer_version("2026.11.26-01-beta.1", "2026.11.25-02"));
        assert!(is_newer_version(
            "2026.11.26-01-beta.2",
            "2026.11.26-01-beta.1"
        ));
        assert!(is_newer_version(
            "2026.11.26-01-rc.1",
            "2026.11.26-01-beta.9"
        ));
        assert!(is_newer_version("2026.11.26-01", "2026.11.26-01-rc.1"));
        assert!(!is_newer_version("2026.11.26-01-beta.1", "2026.11.26-01"));
        assert!(!is_newer_version(
            "2026.11.26-01-beta.1",
            "2026.11.26-01-beta.2"
        ));

        let list = json!([
            {
                "tag_name": "2026.11.27-01-beta.1",
                "html_url": "https://example.com/draft",
                "draft": true,
                "prerelease": true,
                "assets": [
                    {
                        "name": "Slackless-2026.11.27-01-beta.1-macos-arm64.app.tar.gz",
                        "browser_download_url": "https://example.com/draft.tar.gz"
                    }
                ]
            },
            {
                "tag_name": "2026.11.26-01-beta.2",
                "html_url": "https://example.com/beta2",
                "body": "- Beta preview feature",
                "draft": false,
                "prerelease": true,
                "assets": [
                    {
                        "name": "Slackless-2026.11.26-01-beta.2-macos-arm64.app.tar.gz",
                        "browser_download_url": "https://example.com/beta2.tar.gz"
                    },
                    {
                        "name": "SHA256SUMS.txt",
                        "browser_download_url": "https://example.com/beta2-sums.txt"
                    }
                ]
            },
            {
                "tag_name": "2026.11.25-02",
                "html_url": "https://example.com/stable",
                "body": "- Stable release",
                "draft": false,
                "prerelease": false,
                "assets": [
                    {
                        "name": "Slackless-2026.11.25-02-macos-arm64.app.tar.gz",
                        "browser_download_url": "https://example.com/stable.tar.gz"
                    }
                ]
            }
        ])
        .to_string();

        let stable = parse_github_releases_list(list.as_bytes(), UpdateChannel::Stable)
            .expect("valid list")
            .expect("stable release");
        assert_eq!(stable.version, "2026.11.25-02");
        assert!(!stable.is_prerelease);

        let beta = parse_github_releases_list(list.as_bytes(), UpdateChannel::Beta)
            .expect("valid list")
            .expect("beta release");
        assert_eq!(beta.version, "2026.11.26-01-beta.2");
        assert!(beta.is_prerelease);
        assert_eq!(beta.notes, vec!["Beta preview feature"]);

        // If a newer stable release exists than the newest prerelease, Beta picks the newer stable.
        let stable_newer = json!([
            {
                "tag_name": "2026.11.26-01-beta.1",
                "html_url": "https://example.com/beta",
                "draft": false,
                "prerelease": true,
                "assets": [{ "name": "Slackless-2026.11.26-01-beta.1-macos-arm64.app.tar.gz", "browser_download_url": "https://example.com/b.tar.gz" }]
            },
            {
                "tag_name": "2026.11.26-01",
                "html_url": "https://example.com/stable",
                "draft": false,
                "prerelease": false,
                "assets": [{ "name": "Slackless-2026.11.26-01-macos-arm64.app.tar.gz", "browser_download_url": "https://example.com/s.tar.gz" }]
            }
        ])
        .to_string();
        let chosen = parse_github_releases_list(stable_newer.as_bytes(), UpdateChannel::Beta)
            .expect("valid")
            .expect("release");
        assert_eq!(chosen.version, "2026.11.26-01");
        assert!(!chosen.is_prerelease);

        // Older unparseable/non-standard tags must NEVER overwrite a valid newer release.
        let with_nonstandard = json!([
            {
                "tag_name": "2026.11.26-01",
                "html_url": "https://example.com/valid",
                "draft": false,
                "prerelease": false,
                "assets": [{ "name": "Slackless-2026.11.26-01-macos-arm64.app.tar.gz", "browser_download_url": "https://example.com/v.tar.gz" }]
            },
            {
                "tag_name": "nightly",
                "html_url": "https://example.com/nightly",
                "draft": false,
                "prerelease": true,
                "assets": [{ "name": "Slackless-nightly-macos-arm64.app.tar.gz", "browser_download_url": "https://example.com/n.tar.gz" }]
            },
            {
                "tag_name": "v1.0-custom",
                "html_url": "https://example.com/custom",
                "draft": false,
                "prerelease": false,
                "assets": [{ "name": "Slackless-v1.0-custom-macos-arm64.app.tar.gz", "browser_download_url": "https://example.com/c.tar.gz" }]
            }
        ])
        .to_string();
        let chosen_beta =
            parse_github_releases_list(with_nonstandard.as_bytes(), UpdateChannel::Beta)
                .expect("valid")
                .expect("release");
        assert_eq!(chosen_beta.version, "2026.11.26-01");

        // And a valid release arriving AFTER a leading unparseable tag replaces it.
        let unparseable_first = json!([
            {
                "tag_name": "nightly",
                "html_url": "https://example.com/nightly",
                "draft": false,
                "prerelease": true,
                "assets": [{ "name": "Slackless-nightly-macos-arm64.app.tar.gz", "browser_download_url": "https://example.com/n.tar.gz" }]
            },
            {
                "tag_name": "2026.11.26-01",
                "html_url": "https://example.com/valid",
                "draft": false,
                "prerelease": false,
                "assets": [{ "name": "Slackless-2026.11.26-01-macos-arm64.app.tar.gz", "browser_download_url": "https://example.com/v.tar.gz" }]
            }
        ])
        .to_string();
        let chosen_after =
            parse_github_releases_list(unparseable_first.as_bytes(), UpdateChannel::Beta)
                .expect("valid")
                .expect("release");
        assert_eq!(chosen_after.version, "2026.11.26-01");
    }
}
