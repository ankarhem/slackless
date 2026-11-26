//! Dedicated sidecar SQLite vector index (`semantic_index.sqlite`) and Reciprocal Rank Fusion
//! (RRF) primitives for local semantic & hybrid search.
//!
//! Stored strictly in its own SQLite file next to the workspace archive databases so the core
//! archive schema (`schema.rs`) stays completely untouched and zero-conflict.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params, params_from_iter};
use slackless_types::TeamId;

use crate::Result;

/// Local search ranking strategy.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    /// Classic instant SQLite FTS5 keyword & Slack modifier search.
    #[default]
    Keyword,
    /// Dense vector cosine similarity search over local ONNX embeddings.
    Semantic,
    /// Reciprocal Rank Fusion (RRF) blending SQLite FTS5 + local semantic embeddings.
    Hybrid,
}

impl SearchMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Keyword => "Keyword",
            Self::Semantic => "Semantic",
            Self::Hybrid => "Hybrid",
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Semantic => "semantic",
            Self::Hybrid => "hybrid",
        }
    }

    pub const fn uses_embeddings(self) -> bool {
        matches!(self, Self::Semantic | Self::Hybrid)
    }
}

/// Kind of archived Slack document stored in the sidecar vector index.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SemanticDocKind {
    Message,
    Snippet,
    Canvas,
    FileTranscript,
    HuddleTranscript,
}

impl SemanticDocKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Snippet => "snippet",
            Self::Canvas => "canvas",
            Self::FileTranscript => "file_transcript",
            Self::HuddleTranscript => "huddle_transcript",
        }
    }

    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "message" => Some(Self::Message),
            "snippet" => Some(Self::Snippet),
            "canvas" => Some(Self::Canvas),
            "file_transcript" => Some(Self::FileTranscript),
            "huddle_transcript" => Some(Self::HuddleTranscript),
            _ => None,
        }
    }
}

/// One vector record to insert or update in [`SemanticIndex`].
#[derive(Debug, Clone)]
pub struct SemanticDocInput {
    pub workspace: TeamId,
    pub kind: SemanticDocKind,
    /// Stable document identifier within `(workspace, kind)`:
    /// - `Message` / `Snippet` / `Canvas`: `"<channel_id>:<ts>"`
    /// - `FileTranscript`: `"<file_id>"`
    /// - `HuddleTranscript`: `"<segment_id>"`
    pub doc_id: String,
    pub channel_id: String,
    pub ts: String,
    pub content_hash: String,
    pub embedding: Vec<f32>,
}

/// One nearest-neighbor hit returned from [`SemanticIndex::search`].
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticHit {
    pub workspace: TeamId,
    pub kind: SemanticDocKind,
    pub doc_id: String,
    pub channel_id: String,
    pub ts: String,
    /// Cosine similarity score in `[-1.0, 1.0]` (higher is more similar).
    pub score: f32,
}

/// A [`SemanticHit`] with its position in the scan, ordered best first: by score (descending),
/// then `ts` (newest first), then `doc_id`, then scan order. A max-heap of them keeps the worst
/// retained hit on top.
struct RankedHit {
    hit: SemanticHit,
    scan_ix: usize,
}

impl RankedHit {
    /// How a candidate (`score`, `ts`, `doc_id` at `scan_ix`) ranks against `self`:
    /// [`Ordering::Less`] when the candidate comes first.
    fn rank_candidate(&self, score: f32, ts: &str, doc_id: &str, scan_ix: usize) -> Ordering {
        self.hit
            .score
            .total_cmp(&score)
            .then_with(|| self.hit.ts.as_str().cmp(ts))
            .then_with(|| doc_id.cmp(&self.hit.doc_id))
            .then_with(|| scan_ix.cmp(&self.scan_ix))
    }
}

impl Ord for RankedHit {
    fn cmp(&self, other: &Self) -> Ordering {
        other.rank_candidate(self.hit.score, &self.hit.ts, &self.hit.doc_id, self.scan_ix)
    }
}

impl PartialOrd for RankedHit {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for RankedHit {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for RankedHit {}

/// Borrow text column `ix` of `row` without copying it.
fn column_str<'r>(row: &'r rusqlite::Row<'_>, ix: usize) -> rusqlite::Result<&'r str> {
    Ok(row.get_ref(ix)?.as_str()?)
}

/// Summary statistics for `semantic_index.sqlite`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SemanticIndexStats {
    pub total_docs: u64,
    pub messages: u64,
    pub snippets: u64,
    pub canvases: u64,
    pub file_transcripts: u64,
    pub huddle_transcripts: u64,
    pub workspaces: u64,
    pub model_id: Option<String>,
    pub dims: Option<usize>,
    pub db_bytes: u64,
}

/// Standard RRF constant (`k = 60` per Cormack, Clarke & Buettcher 2009).
pub const DEFAULT_RRF_K: f64 = 60.0;

/// Single-list Reciprocal Rank Fusion contribution for a 1-based `rank` (`rank >= 1`).
#[inline]
pub fn rrf_score(rank_1_based: usize, k: f64) -> f64 {
    let r = rank_1_based.max(1) as f64;
    let denom = k.max(1.0) + r;
    1.0 / denom
}

/// Blend multiple ranked key lists using weighted Reciprocal Rank Fusion (RRF).
///
/// Each input slice is `(weight, ranked_keys)` where `ranked_keys[0]` is rank 1. Duplicate keys
/// within the same ranked list keep their best (earliest) rank. Returns `(key, fused_score)` sorted
/// descending by score (breaking ties deterministically by earliest appearance).
pub fn fuse_rrf<K>(ranked_lists: &[(f64, Vec<K>)], k: f64) -> Vec<(K, f64)>
where
    K: Eq + std::hash::Hash + Clone,
{
    let mut scores: HashMap<K, (f64, usize)> = HashMap::new();
    let mut order_seq = 0usize;

    for (weight, list) in ranked_lists {
        if *weight <= 0.0 {
            continue;
        }
        let mut seen_in_list: HashSet<&K> = HashSet::with_capacity(list.len());
        for (idx, key) in list.iter().enumerate() {
            if !seen_in_list.insert(key) {
                continue;
            }
            let contrib = weight * rrf_score(idx + 1, k);
            scores
                .entry(key.clone())
                .and_modify(|(s, _)| *s += contrib)
                .or_insert_with(|| {
                    let seq = order_seq;
                    order_seq += 1;
                    (contrib, seq)
                });
        }
    }

    let mut out: Vec<(K, f64, usize)> = scores
        .into_iter()
        .map(|(key, (score, seq))| (key, score, seq))
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.2.cmp(&b.2)));
    out.into_iter()
        .map(|(key, score, _)| (key, score))
        .collect()
}

/// L2-normalize `vec` in place. Leaves zero or non-finite vectors untouched (zeroed if non-finite).
pub fn l2_normalize(vec: &mut [f32]) {
    let mut sum_sq = 0.0_f64;
    for &x in vec.iter() {
        if x.is_finite() {
            sum_sq += f64::from(x) * f64::from(x);
        }
    }
    if sum_sq <= 0.0 || !sum_sq.is_finite() {
        for x in vec.iter_mut() {
            *x = 0.0;
        }
        return;
    }
    let inv = (1.0 / sum_sq.sqrt()) as f32;
    for x in vec.iter_mut() {
        *x = if x.is_finite() { *x * inv } else { 0.0 };
    }
}

/// Cosine similarity between two equal-length vectors. Returns `0.0` for mismatched, empty,
/// or zero-norm vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0_f64;
    let mut norm_a = 0.0_f64;
    let mut norm_b = 0.0_f64;
    for (&x, &y) in a.iter().zip(b.iter()) {
        let xf = f64::from(x);
        let yf = f64::from(y);
        dot += xf * yf;
        norm_a += xf * xf;
        norm_b += yf * yf;
    }
    let denom = (norm_a * norm_b).sqrt();
    if denom <= 0.0 || !denom.is_finite() {
        0.0
    } else {
        (dot / denom).clamp(-1.0, 1.0) as f32
    }
}

/// Pack an `f32` embedding slice into a compact little-endian byte blob.
pub fn encode_f32_blob(vec: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(std::mem::size_of_val(vec));
    for &v in vec {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Unpack a little-endian `f32` byte blob, validating `expected_dims`.
pub fn decode_f32_blob(blob: &[u8], expected_dims: usize) -> Option<Vec<f32>> {
    if expected_dims == 0 || blob.len() != expected_dims.checked_mul(4)? {
        return None;
    }
    let mut out = Vec::with_capacity(expected_dims);
    for &bytes in blob.as_chunks::<4>().0 {
        out.push(f32::from_le_bytes(bytes));
    }
    Some(out)
}

/// Zero-allocation cosine similarity between a pre-normalized query vector and a stored
/// little-endian `f32` blob (which is also stored L2-normalized, with defensive norm fallback).
fn cosine_against_blob(query_unit: &[f32], blob: &[u8]) -> Option<f32> {
    if blob.len() != query_unit.len().checked_mul(4)? {
        return None;
    }
    let mut dot = 0.0_f32;
    let mut norm_b = 0.0_f32;
    for (&q, &bytes) in query_unit.iter().zip(blob.as_chunks::<4>().0) {
        let b = f32::from_le_bytes(bytes);
        dot += q * b;
        norm_b += b * b;
    }
    if norm_b <= 0.0 || !norm_b.is_finite() {
        return Some(0.0);
    }
    // Fast path when stored blob is already unit-normalized (|norm_b - 1| < 1e-3).
    let score = if (norm_b - 1.0).abs() < 1e-3 {
        dot
    } else {
        dot / norm_b.sqrt()
    };
    Some(score.clamp(-1.0, 1.0))
}

const SEMANTIC_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS semantic_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS semantic_embedding (
    workspace_id TEXT NOT NULL,
    doc_kind     TEXT NOT NULL,
    doc_id       TEXT NOT NULL,
    channel_id   TEXT NOT NULL DEFAULT '',
    ts           TEXT NOT NULL DEFAULT '',
    content_hash TEXT NOT NULL,
    dims         INTEGER NOT NULL,
    vec_blob     BLOB NOT NULL,
    updated_at   INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, doc_kind, doc_id)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS semantic_embedding_ws
    ON semantic_embedding (workspace_id, doc_kind);
"#;

/// Sidecar SQLite store holding dense document embeddings across all workspaces.
pub struct SemanticIndex {
    conn: Connection,
}

impl SemanticIndex {
    /// Open or create the sidecar semantic index at `path` (WAL mode).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            let _ = std::fs::create_dir_all(parent);
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.execute_batch(
            "PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -4000;
             PRAGMA mmap_size = 16777216;",
        );
        Self::init(conn)
    }

    /// Open an existing sidecar index read-only (falls back cleanly when caller needs writes).
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let _ = conn.execute_batch(
            "PRAGMA query_only = ON;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -2000;",
        );
        // Probe table readability so corrupt/incomplete files fail early.
        let _: i64 = conn.query_row("SELECT count(*) FROM semantic_meta", [], |r| r.get(0))?;
        Ok(Self { conn })
    }

    /// Open an ephemeral in-memory semantic index (unit & integration tests).
    pub fn open_in_memory() -> Result<Self> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_in_memory_with_flags(flags)?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(SEMANTIC_SCHEMA)?;
        Ok(Self { conn })
    }

    /// Read the active embedding `model_id` recorded in `semantic_meta`, if any.
    pub fn model_id(&self) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM semantic_meta WHERE key = 'model_id'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?)
    }

    /// Read the active embedding dimensionality recorded in `semantic_meta`, if any.
    pub fn dims(&self) -> Result<Option<usize>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM semantic_meta WHERE key = 'dims'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(raw.and_then(|s| s.parse::<usize>().ok()))
    }

    /// Ensure the index is configured for `(model_id, dims)`. If an existing index used a
    /// different model or dimensionality, clears stale vectors automatically and returns `true`.
    pub fn ensure_model(&mut self, model_id: &str, dims: usize) -> Result<bool> {
        let prev_model = self.model_id()?;
        let prev_dims = self.dims()?;
        let changed = prev_model.as_deref().is_some_and(|m| m != model_id)
            || prev_dims.is_some_and(|d| d != dims);
        let tx = self.conn.transaction()?;
        if changed {
            tx.execute("DELETE FROM semantic_embedding", [])?;
        }
        tx.execute(
            "INSERT INTO semantic_meta (key, value) VALUES ('model_id', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![model_id],
        )?;
        tx.execute(
            "INSERT INTO semantic_meta (key, value) VALUES ('dims', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![dims.to_string()],
        )?;
        tx.commit()?;
        Ok(changed)
    }

    /// Clear every stored vector in the index (for `--rebuild`).
    pub fn clear(&mut self) -> Result<usize> {
        let removed = self.conn.execute("DELETE FROM semantic_embedding", [])?;
        Ok(removed)
    }

    /// Distinct workspace ids that currently own vectors, sorted.
    pub fn workspace_ids(&self) -> Result<Vec<TeamId>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT workspace_id FROM semantic_embedding ORDER BY workspace_id",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(TeamId::from(row?));
        }
        Ok(out)
    }

    /// Delete every vector belonging to `workspace` (e.g. after its archive was removed).
    pub fn delete_workspace(&mut self, workspace: &TeamId) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM semantic_embedding WHERE workspace_id = ?1",
            params![workspace.as_str()],
        )?)
    }

    /// Checkpoint the WAL and compact free pages so pruned vectors give disk space back.
    pub fn vacuum(&mut self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
        Ok(())
    }

    /// Load existing `(doc_kind, doc_id) -> (content_hash, dims)` for one workspace so
    /// incremental indexing can skip unchanged documents.
    pub fn existing_hashes(
        &self,
        workspace: &TeamId,
    ) -> Result<HashMap<(SemanticDocKind, String), (String, usize)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT doc_kind, doc_id, content_hash, dims
             FROM semantic_embedding
             WHERE workspace_id = ?1",
        )?;
        let rows = stmt.query_map(params![workspace.as_str()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (kind_s, doc_id, hash, dims) = row?;
            if let Some(kind) = SemanticDocKind::from_tag(&kind_s)
                && dims > 0
            {
                out.insert((kind, doc_id), (hash, dims as usize));
            }
        }
        Ok(out)
    }

    /// Upsert a batch of embeddings in a single SQLite transaction, L2-normalizing each vector
    /// before packing so cosine search reduces to a fast dot product.
    pub fn upsert_batch(&mut self, docs: &[SemanticDocInput]) -> Result<usize> {
        if docs.is_empty() {
            return Ok(0);
        }
        let now = now_secs();
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO semantic_embedding
                     (workspace_id, doc_kind, doc_id, channel_id, ts, content_hash, dims, vec_blob, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (workspace_id, doc_kind, doc_id) DO UPDATE SET
                     channel_id   = excluded.channel_id,
                     ts           = excluded.ts,
                     content_hash = excluded.content_hash,
                     dims         = excluded.dims,
                     vec_blob     = excluded.vec_blob,
                     updated_at   = excluded.updated_at",
            )?;
            for doc in docs {
                if doc.embedding.is_empty() {
                    continue;
                }
                let mut unit = doc.embedding.clone();
                l2_normalize(&mut unit);
                let blob = encode_f32_blob(&unit);
                stmt.execute(params![
                    doc.workspace.as_str(),
                    doc.kind.as_str(),
                    doc.doc_id,
                    doc.channel_id,
                    doc.ts,
                    doc.content_hash,
                    unit.len() as i64,
                    blob,
                    now,
                ])?;
            }
        }
        tx.commit()?;
        Ok(docs.len())
    }

    /// Delete any indexed entries in `workspace` whose `(doc_kind, doc_id)` is no longer present
    /// in `live_keys`. Returns the count of pruned rows.
    pub fn prune_workspace(
        &mut self,
        workspace: &TeamId,
        live_keys: &HashSet<(SemanticDocKind, String)>,
    ) -> Result<usize> {
        let existing = self.existing_hashes(workspace)?;
        let stale: Vec<(SemanticDocKind, String)> = existing
            .into_keys()
            .filter(|k| !live_keys.contains(k))
            .collect();
        if stale.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        let mut removed = 0usize;
        {
            let mut del = tx.prepare_cached(
                "DELETE FROM semantic_embedding
                 WHERE workspace_id = ?1 AND doc_kind = ?2 AND doc_id = ?3",
            )?;
            for (kind, doc_id) in stale {
                removed += del.execute(params![workspace.as_str(), kind.as_str(), doc_id])?;
            }
        }
        tx.commit()?;
        Ok(removed)
    }

    /// Cosine-similarity nearest-neighbor search over stored embeddings.
    ///
    /// Optionally scopes to `workspaces` and/or `kinds`, filters by `min_score`, and returns the
    /// top `limit` hits sorted descending by similarity score (ties: newest `ts` first, then
    /// `doc_id`, then scan order).
    pub fn search(
        &self,
        query_embedding: &[f32],
        workspaces: Option<&[TeamId]>,
        kinds: Option<&[SemanticDocKind]>,
        limit: usize,
        min_score: f32,
    ) -> Result<Vec<SemanticHit>> {
        if limit == 0 || query_embedding.is_empty() {
            return Ok(Vec::new());
        }
        if workspaces.is_some_and(|w| w.is_empty()) || kinds.is_some_and(|k| k.is_empty()) {
            return Ok(Vec::new());
        }

        let mut query_unit = query_embedding.to_vec();
        l2_normalize(&mut query_unit);
        if query_unit.iter().all(|&v| v == 0.0) {
            return Ok(Vec::new());
        }

        let mut sql = String::from(
            "SELECT workspace_id, doc_kind, doc_id, channel_id, ts, vec_blob
             FROM semantic_embedding
             WHERE dims = ?",
        );
        let mut args: Vec<SqlValue> = vec![SqlValue::Integer(query_unit.len() as i64)];

        if let Some(ws_list) = workspaces {
            let ph = std::iter::repeat_n("?", ws_list.len())
                .collect::<Vec<_>>()
                .join(", ");
            sql.push_str(&format!(" AND workspace_id IN ({ph})"));
            for ws in ws_list {
                args.push(SqlValue::Text(ws.as_str().to_owned()));
            }
        }
        if let Some(kind_list) = kinds {
            let ph = std::iter::repeat_n("?", kind_list.len())
                .collect::<Vec<_>>()
                .join(", ");
            sql.push_str(&format!(" AND doc_kind IN ({ph})"));
            for kind in kind_list {
                args.push(SqlValue::Text(kind.as_str().to_owned()));
            }
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(args))?;
        // Rows are scored straight from their borrowed blobs; only the best `limit` hits are
        // kept, and a row's strings are copied only once it ranks into them.
        let mut best: BinaryHeap<RankedHit> = BinaryHeap::new();
        let mut scan_ix = 0usize;
        while let Some(row) = rows.next()? {
            let row_ix = scan_ix;
            scan_ix += 1;
            let Ok(blob) = row.get_ref(5)?.as_blob() else {
                continue;
            };
            let Some(score) = cosine_against_blob(&query_unit, blob) else {
                continue;
            };
            if score >= min_score {
                let Some(kind) = SemanticDocKind::from_tag(column_str(row, 1)?) else {
                    continue;
                };
                let doc_id = column_str(row, 2)?;
                let ts = column_str(row, 4)?;
                let full = best.len() >= limit;
                if full
                    && best
                        .peek()
                        .is_none_or(|worst| worst.rank_candidate(score, ts, doc_id, row_ix).is_ge())
                {
                    continue;
                }
                let ranked = RankedHit {
                    hit: SemanticHit {
                        workspace: TeamId::from(column_str(row, 0)?),
                        kind,
                        doc_id: doc_id.to_owned(),
                        channel_id: column_str(row, 3)?.to_owned(),
                        ts: ts.to_owned(),
                        score,
                    },
                    scan_ix: row_ix,
                };
                if !full {
                    best.push(ranked);
                } else if let Some(mut worst) = best.peek_mut() {
                    *worst = ranked;
                }
            }
        }
        Ok(best
            .into_sorted_vec()
            .into_iter()
            .map(|ranked| ranked.hit)
            .collect())
    }

    /// Return aggregate document counts, model metadata, and SQLite byte size.
    pub fn stats(&self) -> Result<SemanticIndexStats> {
        let mut stats = SemanticIndexStats {
            model_id: self.model_id()?,
            dims: self.dims()?,
            ..Default::default()
        };
        {
            let mut stmt = self.conn.prepare_cached(
                "SELECT doc_kind, COUNT(*) FROM semantic_embedding GROUP BY doc_kind",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            for row in rows {
                let (kind_s, count) = row?;
                let c = u64::try_from(count.max(0)).unwrap_or(0);
                stats.total_docs = stats.total_docs.saturating_add(c);
                match SemanticDocKind::from_tag(&kind_s) {
                    Some(SemanticDocKind::Message) => stats.messages = c,
                    Some(SemanticDocKind::Snippet) => stats.snippets = c,
                    Some(SemanticDocKind::Canvas) => stats.canvases = c,
                    Some(SemanticDocKind::FileTranscript) => stats.file_transcripts = c,
                    Some(SemanticDocKind::HuddleTranscript) => stats.huddle_transcripts = c,
                    None => {}
                }
            }
        }
        let ws_count: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT workspace_id) FROM semantic_embedding",
            [],
            |r| r.get(0),
        )?;
        stats.workspaces = u64::try_from(ws_count.max(0)).unwrap_or(0);
        let page_count: i64 = self
            .conn
            .pragma_query_value(None, "page_count", |r| r.get(0))
            .unwrap_or(0);
        let page_size: i64 = self
            .conn
            .pragma_query_value(None, "page_size", |r| r.get(0))
            .unwrap_or(0);
        stats.db_bytes = u64::try_from(page_count.max(0))
            .unwrap_or(0)
            .saturating_mul(u64::try_from(page_size.max(0)).unwrap_or(0));
        Ok(stats)
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_similarity_and_blob_roundtrip() {
        let mut a = vec![3.0_f32, 4.0, 0.0];
        let b = vec![6.0_f32, 8.0, 0.0];
        let c = vec![0.0_f32, 0.0, 5.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-5);
        assert!(cosine_similarity(&a, &c).abs() < 1e-5);
        assert_eq!(cosine_similarity(&a, &[1.0, 2.0]), 0.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);

        l2_normalize(&mut a);
        assert!((a[0] - 0.6).abs() < 1e-5);
        assert!((a[1] - 0.8).abs() < 1e-5);

        let blob = encode_f32_blob(&a);
        let decoded = decode_f32_blob(&blob, 3).expect("decode 3 dims");
        assert_eq!(decoded, a);
        assert!(decode_f32_blob(&blob, 4).is_none());
    }

    #[test]
    fn rrf_fusion_boosts_consensus_documents_and_deduplicates() {
        // Doc B appears in both FTS5 (#2) and Semantic (#1) -> should beat Doc A (#1 FTS only)
        // and Doc C (#2 Semantic only).
        let fts = vec!["doc_a", "doc_b", "doc_d", "doc_b"]; // duplicate doc_b ignored
        let sem = vec!["doc_b", "doc_c", "doc_a"];
        let fused = fuse_rrf(&[(1.0, fts), (1.0, sem)], DEFAULT_RRF_K);

        assert_eq!(fused[0].0, "doc_b");
        assert_eq!(fused[1].0, "doc_a");
        // Verify exact mathematical RRF score for doc_b: 1/(60+2) + 1/(60+1).
        let expected_b = rrf_score(2, 60.0) + rrf_score(1, 60.0);
        assert!((fused[0].1 - expected_b).abs() < 1e-12);
        // Verify doc_a: 1/(60+1) + 1/(60+3).
        let expected_a = rrf_score(1, 60.0) + rrf_score(3, 60.0);
        assert!((fused[1].1 - expected_a).abs() < 1e-12);
        assert!(fused[0].1 > fused[1].1);
        assert_eq!(fused.len(), 4);
    }

    #[test]
    fn sidecar_index_incremental_upsert_search_prune_and_model_switch() {
        let mut idx = SemanticIndex::open_in_memory().unwrap();
        assert!(!idx.ensure_model("minilm-v2", 4).unwrap());

        let ws1 = TeamId::from("T1");
        let ws2 = TeamId::from("T2");
        idx.upsert_batch(&[
            SemanticDocInput {
                workspace: ws1.clone(),
                kind: SemanticDocKind::Message,
                doc_id: "C1:1700000001.000000".into(),
                channel_id: "C1".into(),
                ts: "1700000001.000000".into(),
                content_hash: "h1".into(),
                embedding: vec![1.0, 0.0, 0.0, 0.0],
            },
            SemanticDocInput {
                workspace: ws1.clone(),
                kind: SemanticDocKind::Canvas,
                doc_id: "C1:1700000002.000000".into(),
                channel_id: "C1".into(),
                ts: "1700000002.000000".into(),
                content_hash: "h2".into(),
                embedding: vec![0.8, 0.6, 0.0, 0.0],
            },
            SemanticDocInput {
                workspace: ws2.clone(),
                kind: SemanticDocKind::HuddleTranscript,
                doc_id: "42".into(),
                channel_id: "C2".into(),
                ts: "1700000003.000000".into(),
                content_hash: "h3".into(),
                embedding: vec![0.0, 1.0, 0.0, 0.0],
            },
        ])
        .unwrap();

        let stats = idx.stats().unwrap();
        assert_eq!(stats.total_docs, 3);
        assert_eq!(stats.messages, 1);
        assert_eq!(stats.canvases, 1);
        assert_eq!(stats.huddle_transcripts, 1);
        assert_eq!(stats.workspaces, 2);
        assert_eq!(stats.model_id.as_deref(), Some("minilm-v2"));
        assert_eq!(stats.dims, Some(4));

        // Cross-workspace search along X axis returns T1 message first, T1 canvas second.
        let hits = idx
            .search(&[1.0, 0.0, 0.0, 0.0], None, None, 10, 0.1)
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].doc_id, "C1:1700000001.000000");
        assert!((hits[0].score - 1.0).abs() < 1e-5);
        assert_eq!(hits[1].doc_id, "C1:1700000002.000000");
        assert!((hits[1].score - 0.8).abs() < 1e-5);

        // Workspace-scoped + kind-scoped search finds T2's huddle transcript.
        let t2_hits = idx
            .search(
                &[0.0, 1.0, 0.0, 0.0],
                Some(std::slice::from_ref(&ws2)),
                Some(&[SemanticDocKind::HuddleTranscript]),
                5,
                0.5,
            )
            .unwrap();
        assert_eq!(t2_hits.len(), 1);
        assert_eq!(t2_hits[0].workspace, ws2);
        assert_eq!(t2_hits[0].doc_id, "42");

        // Prune removes docs no longer live in T1 without touching T2.
        let mut live = HashSet::new();
        live.insert((SemanticDocKind::Message, "C1:1700000001.000000".to_owned()));
        assert_eq!(idx.prune_workspace(&ws1, &live).unwrap(), 1);
        assert_eq!(idx.stats().unwrap().total_docs, 2);

        // Switching models clears incompatible vectors automatically.
        assert!(idx.ensure_model("bge-small", 4).unwrap());
        assert_eq!(idx.stats().unwrap().total_docs, 0);
        assert_eq!(idx.model_id().unwrap().as_deref(), Some("bge-small"));
    }

    #[test]
    fn search_keeps_the_best_limit_with_ts_doc_id_and_scan_order_tie_breaks() {
        let mut idx = SemanticIndex::open_in_memory().unwrap();
        idx.ensure_model("m", 2).unwrap();
        let doc = |ws: &str, doc_id: &str, ts: &str, embedding: [f32; 2]| SemanticDocInput {
            workspace: TeamId::from(ws),
            kind: SemanticDocKind::Message,
            doc_id: doc_id.into(),
            channel_id: "C1".into(),
            ts: ts.into(),
            content_hash: "h".into(),
            embedding: embedding.to_vec(),
        };
        // Every exact match ties on score; "b"/"c" also tie on ts, and the two "d"s tie on
        // everything but their workspace, so scan order decides (T1's row is scanned first).
        idx.upsert_batch(&[
            doc("T1", "a", "1.0", [1.0, 0.0]),
            doc("T1", "c", "3.0", [1.0, 0.0]),
            doc("T1", "b", "3.0", [1.0, 0.0]),
            doc("T2", "d", "2.0", [1.0, 0.0]),
            doc("T1", "d", "2.0", [1.0, 0.0]),
            doc("T1", "e", "9.0", [0.6, 0.8]),
            doc("T1", "f", "9.0", [0.0, 1.0]),
        ])
        .unwrap();
        let ranked = |limit: usize| -> Vec<(String, String)> {
            idx.search(&[1.0, 0.0], None, None, limit, 0.1)
                .unwrap()
                .into_iter()
                .map(|hit| (hit.workspace.as_str().to_owned(), hit.doc_id))
                .collect()
        };
        let expected: Vec<(String, String)> = [
            ("T1", "b"),
            ("T1", "c"),
            ("T1", "d"),
            ("T2", "d"),
            ("T1", "a"),
            ("T1", "e"),
        ]
        .into_iter()
        .map(|(ws, doc_id)| (ws.to_owned(), doc_id.to_owned()))
        .collect();
        assert_eq!(ranked(100), expected);
        // Any limit keeps exactly the first `limit` hits of the full ranking, in order.
        for limit in 1..=expected.len() {
            assert_eq!(ranked(limit), expected[..limit], "limit {limit}");
        }
    }

    #[test]
    fn workspace_listing_deletion_and_vacuum() {
        let mut idx = SemanticIndex::open_in_memory().unwrap();
        idx.ensure_model("m", 2).unwrap();
        let doc = |ws: &str, id: &str| SemanticDocInput {
            workspace: TeamId::from(ws),
            kind: SemanticDocKind::Message,
            doc_id: id.into(),
            channel_id: "C1".into(),
            ts: "1.0".into(),
            content_hash: "h".into(),
            embedding: vec![1.0, 0.0],
        };
        idx.upsert_batch(&[doc("T2", "a"), doc("T1", "b"), doc("T2", "c")])
            .unwrap();
        assert_eq!(
            idx.workspace_ids().unwrap(),
            vec![TeamId::from("T1"), TeamId::from("T2")]
        );
        assert_eq!(idx.delete_workspace(&TeamId::from("T2")).unwrap(), 2);
        assert_eq!(idx.delete_workspace(&TeamId::from("T9")).unwrap(), 0);
        assert_eq!(idx.workspace_ids().unwrap(), vec![TeamId::from("T1")]);
        idx.vacuum().unwrap();
        assert_eq!(idx.stats().unwrap().total_docs, 1);
    }
}
