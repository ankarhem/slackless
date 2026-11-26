//! Durable offline send outbox (`outbox`, schema V13): persists queued/failed messages, thread
//! replies, and reactions across restarts with atomic status transitions for idempotent FIFO flush.

use super::*;

/// Kind of outgoing Slack mutation stored in the durable `outbox` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutboxKind {
    Message,
    Reply,
    ReactionAdd,
    ReactionRemove,
}

impl OutboxKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Reply => "reply",
            Self::ReactionAdd => "reaction_add",
            Self::ReactionRemove => "reaction_remove",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "message" => Some(Self::Message),
            "reply" => Some(Self::Reply),
            "reaction_add" => Some(Self::ReactionAdd),
            "reaction_remove" => Some(Self::ReactionRemove),
            _ => None,
        }
    }
}

/// Lifecycle state of one durable `outbox` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutboxStatus {
    Pending,
    Sending,
    Failed,
}

impl OutboxStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sending => "sending",
            Self::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "pending" => Some(Self::Pending),
            "sending" => Some(Self::Sending),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// One durable row from the SQLite `outbox` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxItem {
    pub id: i64,
    pub workspace_id: TeamId,
    pub channel_id: ChannelId,
    pub thread_ts: Option<Ts>,
    pub kind: OutboxKind,
    pub payload_json: String,
    pub status: OutboxStatus,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Input parameters for enqueuing a new durable `outbox` entry.
#[derive(Debug, Clone)]
pub struct NewOutboxItem<'a> {
    pub channel_id: &'a ChannelId,
    pub thread_ts: Option<&'a Ts>,
    pub kind: OutboxKind,
    pub payload_json: &'a str,
    pub last_error: Option<&'a str>,
}

const OUTBOX_COLUMNS: &str = "id, workspace_id, channel_id, thread_ts, kind, payload_json, status, attempts, last_error, created_at, updated_at";

fn row_to_outbox_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<OutboxItem> {
    let id: i64 = r.get(0)?;
    let ws: String = r.get(1)?;
    let channel: String = r.get(2)?;
    let thread_ts: Option<String> = r.get(3)?;
    let kind_raw: String = r.get(4)?;
    let payload_json: String = r.get(5)?;
    let status_raw: String = r.get(6)?;
    let attempts: i64 = r.get(7)?;
    let last_error: Option<String> = r.get(8)?;
    let created_at: i64 = r.get(9)?;
    let updated_at: i64 = r.get(10)?;

    let kind = OutboxKind::parse(&kind_raw).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            format!("invalid outbox kind: {kind_raw}").into(),
        )
    })?;
    let status = OutboxStatus::parse(&status_raw).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Text,
            format!("invalid outbox status: {status_raw}").into(),
        )
    })?;
    Ok(OutboxItem {
        id,
        workspace_id: TeamId::from(ws),
        channel_id: ChannelId::from(channel),
        thread_ts: thread_ts.filter(|s| !s.is_empty()).map(Ts::from),
        kind,
        payload_json,
        status,
        attempts: u32::try_from(attempts.max(0)).unwrap_or(u32::MAX),
        last_error,
        created_at,
        updated_at,
    })
}

impl Store {
    /// Insert a new pending outbox entry for `workspace` and return the persisted [`OutboxItem`].
    pub fn enqueue_outbox(
        &mut self,
        workspace: &TeamId,
        item: &NewOutboxItem<'_>,
    ) -> Result<OutboxItem> {
        self.ensure_writable()?;
        let now = now_secs();
        let thread_ts = item.thread_ts.filter(|ts| !ts.as_str().is_empty());
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO outbox
                 (workspace_id, channel_id, thread_ts, kind, payload_json, status, attempts,
                  last_error, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending', 0, ?6, ?7, ?7)",
        )?;
        stmt.execute(params![
            workspace.as_str(),
            item.channel_id.as_str(),
            thread_ts.map(Ts::as_str),
            item.kind.as_str(),
            item.payload_json,
            item.last_error,
            now,
        ])?;
        let id = self.conn.last_insert_rowid();
        Self::bump_message_revision();
        Ok(OutboxItem {
            id,
            workspace_id: workspace.clone(),
            channel_id: item.channel_id.clone(),
            thread_ts: thread_ts.cloned(),
            kind: item.kind,
            payload_json: item.payload_json.to_owned(),
            status: OutboxStatus::Pending,
            attempts: 0,
            last_error: item.last_error.map(str::to_owned),
            created_at: now,
            updated_at: now,
        })
    }

    /// Look up one outbox row by primary key `id`.
    pub fn get_outbox_item(&self, id: i64) -> Result<Option<OutboxItem>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {OUTBOX_COLUMNS} FROM outbox WHERE id = ?1"
        ))?;
        Ok(stmt.query_row(params![id], row_to_outbox_item).optional()?)
    }

    /// List all outbox items for `workspace` in FIFO order (`id ASC`).
    pub fn list_outbox(&self, workspace: &TeamId) -> Result<Vec<OutboxItem>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {OUTBOX_COLUMNS} FROM outbox WHERE workspace_id = ?1 ORDER BY id ASC"
        ))?;
        let rows = stmt.query_map(params![workspace.as_str()], row_to_outbox_item)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// List only `pending` outbox items for `workspace` in FIFO order (`id ASC`).
    pub fn list_outbox_pending(&self, workspace: &TeamId) -> Result<Vec<OutboxItem>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {OUTBOX_COLUMNS} FROM outbox
             WHERE workspace_id = ?1 AND status = 'pending'
             ORDER BY id ASC"
        ))?;
        let rows = stmt.query_map(params![workspace.as_str()], row_to_outbox_item)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// List outbox items for a specific channel (or specific thread when `thread_ts` is `Some`),
    /// ordered FIFO (`id ASC`).
    pub fn list_outbox_for_channel(
        &self,
        workspace: &TeamId,
        channel: &ChannelId,
        thread_ts: Option<&Ts>,
    ) -> Result<Vec<OutboxItem>> {
        match thread_ts {
            Some(ts) => {
                let mut stmt = self.conn.prepare_cached(&format!(
                    "SELECT {OUTBOX_COLUMNS} FROM outbox
                     WHERE workspace_id = ?1 AND channel_id = ?2 AND thread_ts = ?3
                     ORDER BY id ASC"
                ))?;
                let rows = stmt.query_map(
                    params![workspace.as_str(), channel.as_str(), ts.as_str()],
                    row_to_outbox_item,
                )?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            }
            None => {
                let mut stmt = self.conn.prepare_cached(&format!(
                    "SELECT {OUTBOX_COLUMNS} FROM outbox
                     WHERE workspace_id = ?1 AND channel_id = ?2 AND thread_ts IS NULL
                     ORDER BY id ASC"
                ))?;
                let rows = stmt.query_map(
                    params![workspace.as_str(), channel.as_str()],
                    row_to_outbox_item,
                )?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            }
        }
    }

    /// Atomically claim an outbox row for delivery (`status != 'sending'` -> `'sending'`),
    /// incrementing `attempts`. Returns `true` if this caller claimed the row, or `false` if it
    /// was already in-flight or deleted (guaranteeing idempotent delivery).
    pub fn mark_outbox_sending(&mut self, id: i64) -> Result<bool> {
        self.ensure_writable()?;
        let mut stmt = self.conn.prepare_cached(
            "UPDATE outbox
             SET status = 'sending', attempts = attempts + 1, updated_at = ?2
             WHERE id = ?1 AND status != 'sending'",
        )?;
        let changed = stmt.execute(params![id, now_secs()])?;
        if changed > 0 {
            Self::bump_message_revision();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Mark an outbox row as `failed` with `error` stored in `last_error`.
    pub fn mark_outbox_failed(&mut self, id: i64, error: &str) -> Result<bool> {
        self.ensure_writable()?;
        let mut stmt = self.conn.prepare_cached(
            "UPDATE outbox
             SET status = 'failed', last_error = ?2, updated_at = ?3
             WHERE id = ?1",
        )?;
        let changed = stmt.execute(params![id, error, now_secs()])?;
        if changed > 0 {
            Self::bump_message_revision();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Reset a single outbox row back to `pending` (e.g. user clicked Retry while still offline).
    pub fn reset_outbox_item_pending(&mut self, id: i64) -> Result<bool> {
        self.ensure_writable()?;
        let mut stmt = self.conn.prepare_cached(
            "UPDATE outbox
             SET status = 'pending', last_error = NULL, updated_at = ?2
             WHERE id = ?1",
        )?;
        let changed = stmt.execute(params![id, now_secs()])?;
        if changed > 0 {
            Self::bump_message_revision();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Recover any rows left in `'sending'` after an unclean process exit by moving them back to
    /// `'pending'`. Returns the number of rows recovered.
    pub fn reset_outbox_sending_to_pending(&mut self, workspace: &TeamId) -> Result<usize> {
        self.ensure_writable()?;
        let mut stmt = self.conn.prepare_cached(
            "UPDATE outbox
             SET status = 'pending', updated_at = ?2
             WHERE workspace_id = ?1 AND status = 'sending'",
        )?;
        let changed = stmt.execute(params![workspace.as_str(), now_secs()])?;
        if changed > 0 {
            Self::bump_message_revision();
        }
        Ok(changed)
    }

    /// Delete one outbox row after successful delivery or explicit user discard.
    pub fn delete_outbox_item(&mut self, id: i64) -> Result<bool> {
        self.ensure_writable()?;
        let mut stmt = self
            .conn
            .prepare_cached("DELETE FROM outbox WHERE id = ?1")?;
        let deleted = stmt.execute(params![id])?;
        if deleted > 0 {
            Self::bump_message_revision();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Delete every outbox row belonging to `workspace`.
    pub fn clear_outbox(&mut self, workspace: &TeamId) -> Result<usize> {
        self.ensure_writable()?;
        let mut stmt = self
            .conn
            .prepare_cached("DELETE FROM outbox WHERE workspace_id = ?1")?;
        let deleted = stmt.execute(params![workspace.as_str()])?;
        if deleted > 0 {
            Self::bump_message_revision();
        }
        Ok(deleted)
    }

    /// Return `(pending_or_sending_count, failed_count)` for `workspace`.
    pub fn outbox_counts(&self, workspace: &TeamId) -> Result<(usize, usize)> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT
                 COALESCE(SUM(CASE WHEN status IN ('pending', 'sending') THEN 1 ELSE 0 END), 0),
                 COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0)
             FROM outbox
             WHERE workspace_id = ?1",
        )?;
        let (pending, failed): (i64, i64) =
            stmt.query_row(params![workspace.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok((pending.max(0) as usize, failed.max(0) as usize))
    }
}
