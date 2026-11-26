//! Local cache hygiene: bounded media-cache retention (size + age policy) and semantic vector
//! index pruning for workspaces whose archives no longer exist.
//!
//! All helpers are blocking filesystem/SQLite work: call them on a background executor/thread.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use slackless_store::SemanticIndex;
use slackless_types::TeamId;

/// Default per-directory media cache byte cap (256 MiB).
pub const DEFAULT_MEDIA_CACHE_MAX_MB: u32 = 256;
/// Default media cache max file age (30 days).
pub const DEFAULT_MEDIA_CACHE_MAX_DAYS: u32 = 30;

/// Retention policy for on-disk media caches. A `0` limit disables that dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaCachePolicy {
    /// Maximum bytes kept per cache directory (`0` = unlimited).
    pub max_bytes: u64,
    /// Files untouched (by mtime) for longer than this many days are removed (`0` = never).
    pub max_age_days: u32,
}

impl Default for MediaCachePolicy {
    fn default() -> Self {
        Self::from_mb_days(DEFAULT_MEDIA_CACHE_MAX_MB, DEFAULT_MEDIA_CACHE_MAX_DAYS)
    }
}

impl MediaCachePolicy {
    /// Policy from user-facing megabytes (MiB) and days.
    pub fn from_mb_days(max_mb: u32, max_age_days: u32) -> Self {
        Self {
            max_bytes: u64::from(max_mb).saturating_mul(1024 * 1024),
            max_age_days,
        }
    }

    /// Maximum file age, if age-based eviction is enabled.
    pub fn max_age(&self) -> Option<Duration> {
        (self.max_age_days > 0)
            .then(|| Duration::from_secs(u64::from(self.max_age_days).saturating_mul(86_400)))
    }
}

/// Outcome of a cache prune pass (summed across directories when pruning several).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneReport {
    pub removed_files: u64,
    pub removed_bytes: u64,
    pub remaining_files: u64,
    pub remaining_bytes: u64,
}

impl PruneReport {
    fn absorb(&mut self, other: PruneReport) {
        self.removed_files += other.removed_files;
        self.removed_bytes = self.removed_bytes.saturating_add(other.removed_bytes);
        self.remaining_files += other.remaining_files;
        self.remaining_bytes = self.remaining_bytes.saturating_add(other.remaining_bytes);
    }
}

/// Every media cache directory slackless writes: the temp playback cache
/// (`$TMPDIR/slackless-media`), legacy audio cache (`$TMPDIR/slackless-audio`), the persistent
/// public avatar/emoji cache (`<cache_dir>/media`), and slackless-huddle's cache of Slack's
/// huddle sounds (`<cache_dir>/sounds`, or `$TMPDIR/slackless-sounds` without a cache dir).
pub fn default_media_cache_dirs() -> Vec<PathBuf> {
    let tmp = std::env::temp_dir();
    let mut dirs = vec![
        tmp.join("slackless-media"),
        tmp.join("slackless-audio"),
        tmp.join("slackless-sounds"),
    ];
    if let Some(disk) = crate::fetcher::disk_cache_dir() {
        dirs.push(disk);
    }
    if let Some(cache) = crate::paths::app_cache_dir() {
        dirs.push(cache.join("sounds"));
    }
    dirs
}

/// Total bytes of regular files directly inside `dir` (0 when missing).
pub fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

/// Total bytes across [`default_media_cache_dirs`].
pub fn media_cache_bytes() -> u64 {
    default_media_cache_dirs()
        .iter()
        .map(|d| dir_bytes(d))
        .sum()
}

/// Apply `max_bytes` / `max_age_days` retention to the regular files directly inside `dir`.
///
/// First removes every file older than `max_age_days` (by mtime; cache hits refresh mtime), then
/// evicts least-recently-modified files until the directory fits within `max_bytes`. `keep` (the
/// file just written or opened) is never removed. A `0` limit disables that dimension.
pub fn prune_media_cache_policy(
    dir: &Path,
    max_bytes: u64,
    max_age_days: u32,
    keep: Option<&Path>,
) -> PruneReport {
    prune_media_cache_at(
        dir,
        MediaCachePolicy {
            max_bytes,
            max_age_days,
        },
        keep,
        SystemTime::now(),
    )
}

/// [`prune_media_cache_policy`] with an explicit `now` (deterministic tests).
pub fn prune_media_cache_at(
    dir: &Path,
    policy: MediaCachePolicy,
    keep: Option<&Path>,
    now: SystemTime,
) -> PruneReport {
    let mut report = PruneReport::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return report;
    };
    let mut files: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        files.push((entry.path(), meta.len(), modified));
    }
    // Oldest first (LRU approximation: cache hits refresh mtime).
    files.sort_by_key(|(_, _, modified)| *modified);
    let is_kept = |p: &Path| keep.is_some_and(|k| k == p);
    let max_age = policy.max_age();

    let mut survivors: Vec<(PathBuf, u64)> = Vec::with_capacity(files.len());
    for (path, len, modified) in files {
        let expired = max_age.is_some_and(|age| {
            now.duration_since(modified)
                .is_ok_and(|elapsed| elapsed > age)
        });
        if expired && !is_kept(&path) && remove_cached_file(&path) {
            report.removed_files += 1;
            report.removed_bytes = report.removed_bytes.saturating_add(len);
        } else {
            survivors.push((path, len));
        }
    }

    let mut total: u64 = survivors.iter().map(|(_, len)| *len).sum();
    let mut remaining_files = survivors.len() as u64;
    if policy.max_bytes > 0 {
        for (path, len) in &survivors {
            if total <= policy.max_bytes {
                break;
            }
            if is_kept(path) {
                continue;
            }
            if remove_cached_file(path) {
                total = total.saturating_sub(*len);
                remaining_files -= 1;
                report.removed_files += 1;
                report.removed_bytes = report.removed_bytes.saturating_add(*len);
            }
        }
    }
    report.remaining_files = remaining_files;
    report.remaining_bytes = total;
    report
}

fn remove_cached_file(path: &Path) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "could not evict cached media file");
            false
        }
    }
}

/// Apply `policy` to each directory in `dirs`, summing the results.
pub fn prune_media_dirs(dirs: &[PathBuf], policy: MediaCachePolicy) -> PruneReport {
    let now = SystemTime::now();
    let mut total = PruneReport::default();
    for dir in dirs {
        total.absorb(prune_media_cache_at(dir, policy, None, now));
    }
    total
}

/// Apply `policy` to every [`default_media_cache_dirs`] entry.
pub fn prune_default_media_caches(policy: MediaCachePolicy) -> PruneReport {
    prune_media_dirs(&default_media_cache_dirs(), policy)
}

/// Suggested archive backup file name: `slackless-<team>-<YYYY-MM-DD>.sqlite` (UTC date of
/// `now_secs`). Unsafe filename characters in `team` are replaced with `_`.
pub fn default_backup_file_name(team: &str, now_secs: i64) -> String {
    let (y, m, d) = slackless_types::time::civil_from_days(now_secs.div_euclid(86_400));
    let safe: String = team
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.is_empty() {
        "archive".to_owned()
    } else {
        safe
    };
    format!("slackless-{safe}-{y:04}-{m:02}-{d:02}.sqlite")
}

/// Delete semantic vectors for every workspace not in `live` (archives that were removed),
/// returning the number of deleted vectors. Compacts the index when anything was removed.
pub fn prune_semantic_index_workspaces(
    index: &mut SemanticIndex,
    live: &HashSet<TeamId>,
) -> std::result::Result<usize, String> {
    let indexed = index
        .workspace_ids()
        .map_err(|e| format!("failed to list semantic index workspaces: {e}"))?;
    let mut removed = 0usize;
    for ws in indexed.iter().filter(|ws| !live.contains(*ws)) {
        removed += index
            .delete_workspace(ws)
            .map_err(|e| format!("failed to prune semantic vectors for {ws}: {e}"))?;
    }
    if removed > 0 {
        index
            .vacuum()
            .map_err(|e| format!("failed to compact semantic index: {e}"))?;
    }
    Ok(removed)
}

/// Prune the default `semantic_index.sqlite` of workspaces with no archive on disk.
/// Returns `Ok(0)` when the index does not exist yet.
pub fn prune_default_semantic_index() -> std::result::Result<usize, String> {
    let Some(path) = crate::paths::semantic_index_path() else {
        return Ok(0);
    };
    if !path.is_file() {
        return Ok(0);
    }
    let live: HashSet<TeamId> = crate::search::discover_local_archives()
        .into_iter()
        .map(|(ws, _)| ws)
        .collect();
    let mut index = SemanticIndex::open(&path)
        .map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    prune_semantic_index_workspaces(&mut index, &live)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::sync::atomic::{AtomicU64, Ordering};

    use slackless_store::{SemanticDocInput, SemanticDocKind};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "slackless-core-maint-{label}-{}-{n}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const DAY: u64 = 86_400;

    fn base() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    fn write_aged(dir: &Path, name: &str, len: usize, age_days: u64) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, vec![0u8; len]).unwrap();
        let f = OpenOptions::new().append(true).open(&path).unwrap();
        f.set_modified(base() - Duration::from_secs(age_days * DAY))
            .unwrap();
        path
    }

    #[test]
    fn policy_conversions_and_defaults() {
        let p = MediaCachePolicy::from_mb_days(2, 7);
        assert_eq!(p.max_bytes, 2 * 1024 * 1024);
        assert_eq!(p.max_age(), Some(Duration::from_secs(7 * DAY)));
        assert_eq!(MediaCachePolicy::from_mb_days(0, 0).max_age(), None);
        let d = MediaCachePolicy::default();
        assert_eq!(d.max_bytes, 256 * 1024 * 1024);
        assert_eq!(d.max_age_days, 30);
    }

    #[test]
    fn prune_removes_expired_files_then_lru_until_under_byte_cap() {
        let dir = TempDir::new("prune");
        let ancient = write_aged(&dir.0, "ancient.mp4", 100, 40);
        let old = write_aged(&dir.0, "old.mp4", 100, 10);
        let mid = write_aged(&dir.0, "mid.mp4", 100, 5);
        let fresh = write_aged(&dir.0, "fresh.mp4", 100, 1);
        std::fs::create_dir_all(dir.0.join("subdir")).unwrap();

        let report = prune_media_cache_at(
            &dir.0,
            MediaCachePolicy {
                max_bytes: 250,
                max_age_days: 30,
            },
            None,
            base(),
        );
        // Age pass drops `ancient`; byte pass then drops the oldest survivor (`old`).
        assert!(!ancient.exists());
        assert!(!old.exists());
        assert!(mid.exists() && fresh.exists());
        assert!(
            dir.0.join("subdir").is_dir(),
            "directories are never touched"
        );
        assert_eq!(report.removed_files, 2);
        assert_eq!(report.removed_bytes, 200);
        assert_eq!(report.remaining_files, 2);
        assert_eq!(report.remaining_bytes, 200);
    }

    #[test]
    fn prune_never_removes_keep_and_zero_limits_disable_dimensions() {
        let dir = TempDir::new("keep");
        let oldest = write_aged(&dir.0, "a.m4a", 300, 90);
        let newer = write_aged(&dir.0, "b.m4a", 300, 2);

        // Unlimited policy is a no-op.
        let noop = prune_media_cache_at(
            &dir.0,
            MediaCachePolicy {
                max_bytes: 0,
                max_age_days: 0,
            },
            None,
            base(),
        );
        assert_eq!(noop.removed_files, 0);
        assert_eq!(noop.remaining_bytes, 600);

        // `keep` survives both the age pass and the byte pass even though it is the oldest.
        let report = prune_media_cache_at(
            &dir.0,
            MediaCachePolicy {
                max_bytes: 100,
                max_age_days: 30,
            },
            Some(&oldest),
            base(),
        );
        assert!(oldest.exists());
        assert!(!newer.exists());
        assert_eq!(report.removed_files, 1);
        assert_eq!(report.remaining_files, 1);
    }

    #[test]
    fn prune_missing_dir_is_empty_report_and_dirs_sum() {
        let dir = TempDir::new("sum");
        let missing = dir.0.join("nope");
        assert_eq!(
            prune_media_cache_policy(&missing, 1, 1, None),
            PruneReport::default()
        );
        let a = dir.0.join("a");
        let b = dir.0.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("1"), vec![0u8; 64]).unwrap();
        std::fs::write(b.join("2"), vec![0u8; 32]).unwrap();
        assert_eq!(dir_bytes(&a), 64);
        let total = prune_media_dirs(&[a, b, missing], MediaCachePolicy::from_mb_days(1, 30));
        assert_eq!(total.removed_files, 0);
        assert_eq!(total.remaining_files, 2);
        assert_eq!(total.remaining_bytes, 96);
    }

    #[test]
    fn media_cache_dirs_cover_the_huddle_sound_cache() {
        // slackless-huddle caches Slack's lifecycle sounds under `<cache>/sounds`, or under
        // `$TMPDIR/slackless-sounds` without a cache dir: Clear cache, Prune and the size
        // count must reach both.
        let dirs = default_media_cache_dirs();
        assert!(dirs.contains(&std::env::temp_dir().join("slackless-sounds")));
        if let Some(cache) = crate::paths::app_cache_dir() {
            assert!(dirs.contains(&cache.join("sounds")), "{dirs:?}");
        }
    }

    #[test]
    fn default_backup_file_name_is_dated_and_sanitized() {
        assert_eq!(
            default_backup_file_name("T0123ABC", 1_764_150_000),
            "slackless-T0123ABC-2025-11-26.sqlite"
        );
        assert_eq!(
            default_backup_file_name("../evil/ws", 0),
            "slackless-___evil_ws-1970-01-01.sqlite"
        );
        assert_eq!(
            default_backup_file_name("", 86_400),
            "slackless-archive-1970-01-02.sqlite"
        );
    }

    #[test]
    fn semantic_index_prune_drops_orphaned_workspaces_only() {
        let mut idx = SemanticIndex::open_in_memory().unwrap();
        idx.ensure_model("m", 2).unwrap();
        let doc = |ws: &str, id: &str| SemanticDocInput {
            workspace: TeamId::from(ws),
            kind: SemanticDocKind::Message,
            doc_id: id.into(),
            channel_id: "C1".into(),
            ts: "1.0".into(),
            content_hash: "h".into(),
            embedding: vec![0.0, 1.0],
        };
        idx.upsert_batch(&[doc("T_LIVE", "1"), doc("T_GONE", "2"), doc("T_GONE", "3")])
            .unwrap();
        let live: HashSet<TeamId> = [TeamId::from("T_LIVE")].into_iter().collect();
        assert_eq!(prune_semantic_index_workspaces(&mut idx, &live).unwrap(), 2);
        assert_eq!(idx.workspace_ids().unwrap(), vec![TeamId::from("T_LIVE")]);
        // Idempotent.
        assert_eq!(prune_semantic_index_workspaces(&mut idx, &live).unwrap(), 0);
    }
}
