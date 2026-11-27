//! Database statistics, storage breakdown, compaction (`VACUUM`), integrity checks, and safe
//! online backups (`VACUUM INTO`).

use std::path::PathBuf;

use super::*;

/// Maximum number of problem rows collected from `PRAGMA quick_check` / `integrity_check`.
const MAX_INTEGRITY_MESSAGES: u32 = 100;

/// Which SQLite consistency check [`Store::integrity_check_with`] runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityMode {
    /// `PRAGMA quick_check`: O(N) page/record structure check (skips index content verification).
    #[default]
    Quick,
    /// `PRAGMA integrity_check`: exhaustive check including index/table consistency.
    Full,
}

impl IntegrityMode {
    pub const fn pragma(self) -> &'static str {
        match self {
            Self::Quick => "quick_check",
            Self::Full => "integrity_check",
        }
    }
}

/// Outcome of a SQLite consistency check.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IntegrityReport {
    /// `true` when SQLite reported exactly `ok`.
    pub ok: bool,
    pub mode: IntegrityMode,
    /// Problem descriptions reported by SQLite (empty when `ok`), capped at 100 rows.
    pub problems: Vec<String>,
    /// Wall-clock duration of the check in milliseconds.
    pub elapsed_ms: u64,
}

impl IntegrityReport {
    /// One-line human summary (`"ok"` or the first problem plus a count).
    pub fn summary(&self) -> String {
        if self.ok {
            return "ok".to_owned();
        }
        match self.problems.as_slice() {
            [] => "integrity check failed".to_owned(),
            [only] => only.clone(),
            [first, rest @ ..] => format!("{first} (+{} more)", rest.len()),
        }
    }
}

/// On-disk footprint of the database file and its write-ahead log.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoreFileInfo {
    /// Main database file (`None` for in-memory stores).
    pub path: Option<PathBuf>,
    /// Allocated database bytes (`page_count * page_size`).
    pub db_bytes: u64,
    /// Bytes held by free (reclaimable by `VACUUM`) pages.
    pub freelist_bytes: u64,
    /// Current size of the `-wal` sidecar file (0 when absent).
    pub wal_bytes: u64,
}

impl Store {
    /// Path of the main database file, as SQLite reports it (`None` for in-memory stores).
    pub fn db_file_path(&self) -> Option<PathBuf> {
        let file: String = self
            .conn
            .query_row(
                "SELECT file FROM pragma_database_list WHERE name = 'main'",
                [],
                |r| r.get(0),
            )
            .ok()?;
        (!file.is_empty()).then(|| PathBuf::from(file))
    }

    /// Database, free-page, and WAL byte sizes (cheap: two pragmas and one `stat`).
    pub fn file_info(&self) -> Result<StoreFileInfo> {
        let page_size: i64 = self
            .conn
            .pragma_query_value(None, "page_size", |r| r.get(0))?;
        let page_count: i64 = self
            .conn
            .pragma_query_value(None, "page_count", |r| r.get(0))?;
        let freelist: i64 = self
            .conn
            .pragma_query_value(None, "freelist_count", |r| r.get(0))?;
        let page_size = page_size.max(0) as u64;
        let path = self.db_file_path();
        let wal_bytes = path
            .as_ref()
            .and_then(|p| std::fs::metadata(sidecar_path(p, "-wal")).ok())
            .map_or(0, |m| m.len());
        Ok(StoreFileInfo {
            path,
            db_bytes: (page_count.max(0) as u64).saturating_mul(page_size),
            freelist_bytes: (freelist.max(0) as u64).saturating_mul(page_size),
            wal_bytes,
        })
    }

    /// Run SQLite's fast structural consistency check (`PRAGMA quick_check`). Read-only and safe
    /// on [`Store::open_read_only`] handles.
    pub fn integrity_check(&self) -> Result<IntegrityReport> {
        self.integrity_check_with(IntegrityMode::Quick)
    }

    /// Run `PRAGMA quick_check` or the exhaustive `PRAGMA integrity_check`.
    pub fn integrity_check_with(&self, mode: IntegrityMode) -> Result<IntegrityReport> {
        let started = std::time::Instant::now();
        let sql = format!("PRAGMA {}({MAX_INTEGRITY_MESSAGES})", mode.pragma());
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut lines = Vec::new();
        for row in rows {
            lines.push(row?);
        }
        let ok = lines.len() == 1 && lines[0].eq_ignore_ascii_case("ok");
        Ok(IntegrityReport {
            ok,
            mode,
            problems: if ok { Vec::new() } else { lines },
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }

    /// Write a consistent, compacted snapshot of the whole database to `dest` using
    /// `VACUUM INTO`, returning the backup file size in bytes.
    ///
    /// Safe while other connections read and write (it reads one WAL snapshot). The snapshot is
    /// first written to a sibling temp file and atomically renamed over `dest`, so a failed or
    /// interrupted backup never leaves a truncated file behind. Rejects destinations that are
    /// the live database (or its `-wal` / `-shm` / `-journal` sidecars), directories, or paths
    /// whose parent directory does not exist.
    pub fn backup_to(&self, dest: &Path) -> Result<u64> {
        let dest = validate_backup_dest(dest, self.db_file_path().as_deref())?;
        let parent = dest
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let file_name = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "backup.sqlite".to_owned());
        let tmp = parent.join(format!(
            ".{file_name}.partial-{}-{}",
            std::process::id(),
            now_nanos()
        ));
        let _ = std::fs::remove_file(&tmp);
        let tmp_str = tmp.to_str().ok_or_else(|| {
            StoreError::InvalidBackupPath(format!("path is not valid UTF-8: {}", dest.display()))
        })?;
        // `PRAGMA query_only` (set on read-only handles) also blocks `VACUUM INTO`'s output file.
        // The connection itself stays `SQLITE_OPEN_READ_ONLY`, so the live database is still
        // protected while the pragma is briefly lifted.
        if self.read_only {
            self.conn.pragma_update(None, "query_only", false)?;
        }
        let vacuumed = self.conn.execute("VACUUM INTO ?1", params![tmp_str]);
        if self.read_only {
            let _ = self.conn.pragma_update(None, "query_only", true);
        }
        if let Err(err) = vacuumed {
            let _ = std::fs::remove_file(&tmp);
            return Err(err.into());
        }
        if let Err(err) = std::fs::rename(&tmp, &dest) {
            let _ = std::fs::remove_file(&tmp);
            return Err(err.into());
        }
        Ok(std::fs::metadata(&dest)?.len())
    }

    /// Summary statistics for `workspace` and overall SQLite database footprint.
    pub fn stats(&self, workspace: &TeamId) -> Result<StoreStats> {
        let ws = workspace.as_str();
        self.with_read_tx(|tx| {
            let count = |sql: &str| -> Result<i64> {
                let mut stmt = tx.prepare_cached(sql)?;
                Ok(stmt.query_row(params![ws], |r| r.get(0))?)
            };
            let messages = count("SELECT count(*) FROM message_current WHERE workspace_id = ?1")?;
            let rev_rows = count("SELECT count(*) FROM message_revision WHERE workspace_id = ?1")?;
            let revised_msgs = count(
                "SELECT count(*) FROM (
                     SELECT DISTINCT channel_id, ts FROM message_revision WHERE workspace_id = ?1
                 )",
            )?;
            let revisions = (messages - revised_msgs).max(0) + rev_rows;
            let conversations = count("SELECT count(*) FROM conversation WHERE workspace_id = ?1")?;
            let users = count("SELECT count(*) FROM \"user\" WHERE workspace_id = ?1")?;
            let offline_queued =
                count("SELECT count(*) FROM offline_queue WHERE workspace_id = ?1")?
                    + count("SELECT count(*) FROM outbox WHERE workspace_id = ?1")?;
            let page_count: i64 = tx.pragma_query_value(None, "page_count", |r| r.get(0))?;
            let page_size: i64 = tx.pragma_query_value(None, "page_size", |r| r.get(0))?;
            Ok(StoreStats {
                messages: messages.max(0) as u64,
                revisions: revisions.max(0) as u64,
                conversations: conversations.max(0) as u64,
                users: users.max(0) as u64,
                offline_queued: offline_queued.max(0) as u64,
                page_count: page_count.max(0) as u64,
                page_size: page_size.max(0) as u64,
            })
        })
    }

    /// Detailed per-table counts and the top `top_n` channels by archived message count.
    pub fn storage_breakdown(&self, workspace: &TeamId, top_n: usize) -> Result<StorageBreakdown> {
        let ws = workspace.as_str();
        self.with_read_tx(|tx| {
            let table_queries = [
                (
                    "message_current",
                    "SELECT count(*) FROM message_current WHERE workspace_id = ?1",
                ),
                (
                    "message_revision",
                    "SELECT count(*) FROM message_revision WHERE workspace_id = ?1",
                ),
                (
                    "conversation",
                    "SELECT count(*) FROM conversation WHERE workspace_id = ?1",
                ),
                (
                    "user",
                    "SELECT count(*) FROM \"user\" WHERE workspace_id = ?1",
                ),
                (
                    "custom_emoji",
                    "SELECT count(*) FROM custom_emoji WHERE workspace_id = ?1",
                ),
                (
                    "offline_queue",
                    "SELECT count(*) FROM offline_queue WHERE workspace_id = ?1",
                ),
                (
                    "outbox",
                    "SELECT count(*) FROM outbox WHERE workspace_id = ?1",
                ),
                (
                    "bot_profile",
                    "SELECT count(*) FROM bot_profile WHERE workspace_id = ?1",
                ),
                (
                    "huddle_transcript",
                    "SELECT count(*) FROM huddle_transcript WHERE workspace_id = ?1",
                ),
                (
                    "file_transcript",
                    "SELECT count(*) FROM file_transcript WHERE workspace_id = ?1",
                ),
                (
                    "message_file",
                    "SELECT count(*) FROM message_file WHERE workspace_id = ?1",
                ),
                (
                    "channel_section",
                    "SELECT count(*) FROM channel_section WHERE workspace_id = ?1",
                ),
                ("team", "SELECT count(*) FROM team WHERE workspace_id = ?1"),
                (
                    "prefs",
                    "SELECT count(*) FROM prefs WHERE workspace_id = ?1",
                ),
                (
                    "local_state",
                    "SELECT count(*) FROM local_state WHERE workspace_id = ?1",
                ),
            ];
            let mut tables = Vec::with_capacity(table_queries.len());
            for (name, sql) in table_queries {
                let mut stmt = tx.prepare_cached(sql)?;
                let count: i64 = stmt.query_row(params![ws], |r| r.get(0))?;
                tables.push((name.to_owned(), count.max(0) as u64));
            }
            let mut stmt = tx.prepare_cached(
                "SELECT m.channel_id, c.name, count(*) AS cnt
                 FROM message_current m
                 LEFT JOIN conversation c
                   ON c.workspace_id = m.workspace_id AND c.id = m.channel_id
                 WHERE m.workspace_id = ?1
                 GROUP BY m.channel_id
                 ORDER BY cnt DESC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![ws, super::sql_limit(top_n)], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, i64>(2)?.max(0) as u64,
                ))
            })?;
            let mut top_channels = Vec::new();
            for row in rows {
                top_channels.push(row?);
            }
            Ok(StorageBreakdown {
                tables,
                top_channels,
            })
        })
    }

    /// Compress any legacy uncompressed `message_current` JSON rows to zlib BLOBs, checkpoint
    /// WAL, compact unused pages (`VACUUM`), and refresh query planner statistics.
    pub fn vacuum(&mut self) -> Result<()> {
        self.ensure_writable()?;
        // Compress legacy plain-TEXT message payloads in batches (skipping ephemeral rows so
        // `drop_ephemeral` can keep using `json_extract`). Because `message_current_au` is scoped
        // to `text, file_names, file_transcripts`, updating `json` alone never churns FTS5.
        const BATCH: usize = 5_000;
        let mut last_id: i64 = 0;
        loop {
            let mut batch: Vec<(i64, String, Vec<u8>)> = Vec::with_capacity(BATCH);
            {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT id, json FROM message_current
                     WHERE id > ?1 AND typeof(json) = 'text' AND length(json) >= 160
                       AND json_valid(json) = 1
                       AND COALESCE(json_extract(json, '$.is_ephemeral'), 0) = 0
                     ORDER BY id ASC LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![last_id, BATCH as i64], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })?;
                let mut seen = 0usize;
                for row in rows {
                    let (id, text) = row?;
                    last_id = id;
                    seen += 1;
                    if let SqlValue::Blob(blob) = encode_json_value(&text, false) {
                        batch.push((id, text, blob));
                    }
                }
                if seen == 0 {
                    break;
                }
            }
            if !batch.is_empty() {
                let tx = self
                    .conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                {
                    // Optimistic CAS predicate: only replace if no concurrent writer modified
                    // `json` between our SELECT and this UPDATE.
                    let mut upd = tx.prepare_cached(
                        "UPDATE message_current SET json = ?1
                         WHERE id = ?2 AND typeof(json) = 'text' AND json = ?3",
                    )?;
                    for (id, orig_text, blob) in &batch {
                        upd.execute(params![blob, id, orig_text])?;
                    }
                }
                tx.commit()?;
            }
        }
        self.conn.execute_batch(
            "PRAGMA wal_checkpoint(TRUNCATE);
             VACUUM;
             PRAGMA optimize;",
        )?;
        Ok(())
    }
}

/// `db` with `suffix` appended to its file name (`T1.sqlite` + `-wal` -> `T1.sqlite-wal`).
fn sidecar_path(db: &Path, suffix: &str) -> PathBuf {
    let mut raw = db.as_os_str().to_owned();
    raw.push(suffix);
    PathBuf::from(raw)
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

/// Absolute, symlink-resolved form of `path`, canonicalizing the parent directory when the file
/// itself does not exist yet.
fn normalize_path(path: &Path) -> Option<PathBuf> {
    if let Ok(canonical) = path.canonicalize() {
        return Some(canonical);
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path.file_name()?;
    parent.canonicalize().ok().map(|p| p.join(name))
}

/// Whether two existing paths refer to the same file (same device + inode on Unix), catching
/// hard links and case-insensitive aliases that string comparison misses.
fn same_existing_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(ma), Ok(mb)) => ma.dev() == mb.dev() && ma.ino() == mb.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        false
    }
}

/// Validate a `VACUUM INTO` destination and return its normalized absolute path.
pub(crate) fn validate_backup_dest(dest: &Path, live_db: Option<&Path>) -> Result<PathBuf> {
    let invalid = |msg: String| StoreError::InvalidBackupPath(msg);
    if dest.as_os_str().is_empty() {
        return Err(invalid("path is empty".to_owned()));
    }
    if dest.file_name().is_none() {
        return Err(invalid(format!("{} does not name a file", dest.display())));
    }
    if dest.is_dir() {
        return Err(invalid(format!("{} is a directory", dest.display())));
    }
    let parent = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(invalid(format!(
            "parent directory {} does not exist",
            parent.display()
        )));
    }
    let normalized = normalize_path(dest)
        .ok_or_else(|| invalid(format!("cannot resolve {}", dest.display())))?;
    if let Some(db) = live_db {
        let db_norm = normalize_path(db).unwrap_or_else(|| db.to_path_buf());
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let live = sidecar_path(&db_norm, suffix);
            if normalized == live || same_existing_file(&normalized, &live) {
                return Err(invalid(format!(
                    "refusing to overwrite the live database file {}",
                    live.display()
                )));
            }
        }
    }
    Ok(normalized)
}
