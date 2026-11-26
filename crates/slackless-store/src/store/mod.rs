//! The blocking [`Store`]. Drive it from a dedicated thread or `spawn_blocking`.

mod archive;
mod bots;
mod catchup;
mod file_transcripts;
mod maintenance;
mod memberships;
mod messages;
mod offline;
mod outbox;
mod search;
#[cfg(test)]
mod tests;
mod transcripts;

pub use archive::{ConversationSummary, DigestChannel, UserSummary};
pub use catchup::RecentFileTranscript;
pub use file_transcripts::{
    StoredFileTranscript, WorkspaceFileTranscript, extract_inline_file_body,
    extract_inline_file_transcript,
};
pub use maintenance::{IntegrityMode, IntegrityReport, StoreFileInfo};
pub use outbox::{NewOutboxItem, OutboxItem, OutboxKind, OutboxStatus};
pub use search::WorkspaceStoredMessage;
#[cfg(test)]
pub use transcripts::WorkspaceHuddleTranscriptSegment;
pub use transcripts::{
    HuddleTranscriptSegment, HuddleTranscriptSession, NewHuddleTranscriptSegment,
};

pub(crate) use messages::decode_json_value;

use std::collections::{HashMap, HashSet};
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
    params_from_iter,
};

use crate::query::{Has, SearchQuery};
use crate::types::time::now_epoch_secs;
use crate::types::{
    BotId, BotProfile, ChannelId, ChannelSection, Conversation, ConversationKind, Message,
    MessageSubtype, Team, TeamId, Ts, User, UserId,
};
use crate::{Result, StoreError, schema};

/// A message as the store currently renders it.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredMessage {
    pub channel: ChannelId,
    /// Latest revision. For a deleted message this is the last content seen before deletion.
    pub message: Message,
    /// Slack deleted the message; render a tombstone (the content is kept locally).
    pub deleted: bool,
}

/// Human and app members of a conversation, as Slack's header counts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MemberCounts {
    /// Human members: neither bots nor deactivated people (and not `USLACKBOT`). A member
    /// with no stored profile counts as a human whose identity is merely unknown.
    pub humans: u32,
    /// App/bot members that are still active (not deactivated, not `USLACKBOT`).
    pub apps: u32,
}

/// One entry of a message's append-only revision log.
#[derive(Debug, Clone, PartialEq)]
pub struct Revision {
    /// `""` for the original post, `edited.ts` for an edit, the deletion event ts for a tombstone.
    pub edit_ts: String,
    pub deleted: bool,
    pub message: Message,
}

/// Summary metrics for the local SQLite database (`Preferences -> Advanced -> Local storage`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub messages: u64,
    pub revisions: u64,
    pub conversations: u64,
    pub users: u64,
    pub offline_queued: u64,
    pub page_count: u64,
    pub page_size: u64,
}

impl StoreStats {
    /// Total allocated SQLite database bytes (`page_count * page_size`).
    pub fn db_bytes(&self) -> u64 {
        self.page_count.saturating_mul(self.page_size)
    }
}

/// Detailed per-table row counts and largest channels for `slackless stats --breakdown`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageBreakdown {
    pub tables: Vec<(String, u64)>,
    pub top_channels: Vec<(String, Option<String>, u64)>,
}

impl Drop for Store {
    fn drop(&mut self) {
        if !self.read_only {
            let _ = self.conn.execute_batch("PRAGMA optimize;");
        }
    }
}

/// Monotonic revision bumped whenever message, transcript, or conversation rows are mutated in
/// any [`Store`] instance. Lets secondary windows skip idle SQLite polling with a single atomic
/// load.
static MESSAGE_REVISION: AtomicU64 = AtomicU64::new(1);

/// Durable local-first message store; the rendering source of truth.
pub struct Store {
    pub(crate) conn: Connection,
    read_only: bool,
    /// Per-handle revision, read only by the test probe [`Store::local_message_revision`].
    #[cfg(test)]
    local_revision: AtomicU64,
}

impl Store {
    /// Current monotonic mutation revision (O(1) atomic read, zero SQLite I/O).
    pub fn message_revision() -> u64 {
        MESSAGE_REVISION.load(Ordering::Acquire)
    }

    /// Monotonic mutation revision scoped to this [`Store`] handle alone, so concurrent stores
    /// (and parallel tests) can observe exact local bumps without cross-store interference.
    #[cfg(test)]
    pub fn local_message_revision(&self) -> u64 {
        self.local_revision.load(Ordering::Acquire)
    }

    #[inline]
    pub(super) fn bump_message_revision(&self) {
        #[cfg(test)]
        self.local_revision.fetch_add(1, Ordering::Release);
        MESSAGE_REVISION.fetch_add(1, Ordering::Release);
    }

    /// Rejects mutations on handles opened via [`Store::open_read_only`].
    #[inline]
    pub(super) fn ensure_writable(&self) -> Result<()> {
        if self.read_only {
            return Err(StoreError::ReadOnly);
        }
        Ok(())
    }

    /// Runs `f` inside a `DEFERRED` read transaction so multi-query reads observe a single
    /// consistent WAL snapshot.
    pub(super) fn with_read_tx<T>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let tx = self.conn.unchecked_transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Opens (creating if needed) the database at `path` and migrates it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        // `Store` is `!Sync` and always externally synchronized, so disable SQLite's internal
        // per-statement mutex to avoid double locking.
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        let _ = conn.execute_batch(
            "PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -4000;
             PRAGMA mmap_size = 16777216;",
        );
        Self::init(conn)
    }

    /// Opens a read-only WAL connection to an existing database at `path`, allowing UI reads to
    /// run concurrently with background write transactions without waiting on the writer mutex.
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let _ = conn.execute_batch(
            "PRAGMA query_only = ON;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -2000;
             PRAGMA mmap_size = 8388608;",
        );
        // Probe header/schema readability eagerly so WAL-without-shm databases fail here instead
        // of returning a broken handle, and refuse databases written by a newer schema version.
        let found = schema::schema_version(&conn)?;
        if found > schema::SCHEMA_VERSION {
            return Err(StoreError::FutureSchema {
                found,
                supported: schema::SCHEMA_VERSION,
            });
        }
        Ok(Self {
            conn,
            read_only: true,
            #[cfg(test)]
            local_revision: AtomicU64::new(0),
        })
    }

    /// A fresh in-memory store (tests, ephemeral sessions).
    pub fn open_in_memory() -> Result<Self> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_in_memory_with_flags(flags)?;
        let _ = conn.execute_batch("PRAGMA temp_store = MEMORY; PRAGMA cache_size = -8000;");
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        let mut store = Self {
            conn,
            read_only: false,
            #[cfg(test)]
            local_revision: AtomicU64::new(0),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Applies pending schema migrations. Idempotent; `open*` already calls it.
    pub fn migrate(&mut self) -> Result<()> {
        self.ensure_writable()?;
        schema::migrate(&mut self.conn)
    }

    /// The applied schema version (`PRAGMA user_version`).
    pub fn schema_version(&self) -> Result<u32> {
        schema::schema_version(&self.conn)
    }

    /// The database file this store reads and writes; `None` for an in-memory store.
    pub fn path(&self) -> Option<&Path> {
        self.conn
            .path()
            .filter(|path| !path.is_empty())
            .map(Path::new)
    }

    pub fn upsert_conversation(&mut self, workspace: &TeamId, conv: &Conversation) -> Result<()> {
        self.upsert_conversations(workspace, std::slice::from_ref(conv))
    }

    /// Upsert a batch of conversations in a single SQLite transaction.
    pub fn upsert_conversations(
        &mut self,
        workspace: &TeamId,
        conversations: &[Conversation],
    ) -> Result<()> {
        self.ensure_writable()?;
        if conversations.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        let ts = now_epoch_secs();
        for conv in conversations {
            upsert_conversation_in_tx(&tx, workspace, conv, ts)?;
        }
        tx.commit()?;
        self.bump_message_revision();
        Ok(())
    }

    pub fn upsert_user(&mut self, workspace: &TeamId, user: &User) -> Result<()> {
        self.upsert_users(workspace, std::slice::from_ref(user))
    }

    /// Upsert a batch of users in a single SQLite transaction.
    pub fn upsert_users(&mut self, workspace: &TeamId, users: &[User]) -> Result<()> {
        self.ensure_writable()?;
        if users.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        let ts = now_epoch_secs();
        {
            let mut stmt = tx.prepare_cached(
                r#"INSERT INTO "user"
                       (workspace_id, id, name, real_name, display_name, is_bot, deleted, json,
                        updated_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                   ON CONFLICT (workspace_id, id) DO UPDATE SET
                       name = excluded.name, real_name = excluded.real_name,
                       display_name = excluded.display_name, is_bot = excluded.is_bot,
                       deleted = excluded.deleted, json = excluded.json,
                       updated_at = excluded.updated_at"#,
            )?;
            for user in users {
                stmt.execute(params![
                    workspace.as_str(),
                    user.id.as_str(),
                    user.name,
                    user.real_name
                        .as_deref()
                        .or(user.profile.real_name.as_deref()),
                    user.profile.display_name,
                    user.is_bot,
                    user.deleted,
                    serde_json::to_string(user)?,
                    ts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// One stored conversation by id, as last written.
    pub fn conversation(&self, workspace: &TeamId, id: &ChannelId) -> Result<Option<Conversation>> {
        load_conversation(&self.conn, workspace, id)
    }

    /// A `conversations.info` refresh of a stored conversation: overlays `fresh` without
    /// clobbering richer stored fields ([`Conversation::merge`]). A conversation not yet
    /// stored is inserted as-is.
    pub fn merge_conversation(&mut self, workspace: &TeamId, fresh: &Conversation) -> Result<()> {
        self.ensure_writable()?;
        // One IMMEDIATE transaction holds the write lock from before the read: the GUI and the
        // CLI write the same file, and a merge based on a row another writer replaced in between
        // would silently revert that write.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let merged = match load_conversation(&tx, workspace, &fresh.id)? {
            Some(mut stored) => {
                stored.merge(fresh);
                stored
            }
            None => fresh.clone(),
        };
        upsert_conversation_in_tx(&tx, workspace, &merged, now_epoch_secs())?;
        tx.commit()?;
        self.bump_message_revision();
        Ok(())
    }

    /// Every stored conversation of `workspace`, by name, as last upserted.
    pub fn conversations(&self, workspace: &TeamId) -> Result<Vec<Conversation>> {
        self.json_column(
            "SELECT json FROM conversation WHERE workspace_id = ?1 ORDER BY name",
            workspace,
        )?
        .iter()
        .map(|json| Ok(serde_json::from_str(json)?))
        .collect()
    }

    /// One stored user by id, as last written (single primary-key lookup).
    pub fn user(&self, workspace: &TeamId, id: &UserId) -> Result<Option<User>> {
        let json: Option<String> = self
            .conn
            .query_row(
                r#"SELECT json FROM "user" WHERE workspace_id = ?1 AND id = ?2"#,
                params![workspace.as_str(), id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.map(|j| serde_json::from_str(&j)).transpose()?)
    }

    /// One stored message by `(workspace, channel, ts)` (single primary-key lookup).
    pub fn message(
        &self,
        workspace: &TeamId,
        channel: &ChannelId,
        ts: &Ts,
    ) -> Result<Option<StoredMessage>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT channel_id, json, deleted FROM message_current
             WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
        )?;
        let row = stmt
            .query_row(
                params![workspace.as_str(), channel.as_str(), ts.as_str()],
                raw_row,
            )
            .optional()?;
        row.map(decode_stored_message).transpose()
    }

    /// Every stored user of `workspace`, as last upserted.
    pub fn users(&self, workspace: &TeamId) -> Result<Vec<User>> {
        self.json_column(
            r#"SELECT json FROM "user" WHERE workspace_id = ?1"#,
            workspace,
        )?
        .iter()
        .map(|json| Ok(serde_json::from_str(json)?))
        .collect()
    }

    /// Stores the workspace's team record (`client.userBoot.team`).
    pub fn upsert_team(&mut self, workspace: &TeamId, team: &Team) -> Result<()> {
        self.ensure_writable()?;
        self.conn.execute(
            "INSERT INTO team (workspace_id, id, json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (workspace_id, id) DO UPDATE SET
                 json = excluded.json, updated_at = excluded.updated_at",
            params![
                workspace.as_str(),
                team.id.as_str(),
                serde_json::to_string(team)?,
                now_epoch_secs(),
            ],
        )?;
        Ok(())
    }

    /// The workspace's own team record (`client.userBoot.team`, whose id is the workspace
    /// id), if hydrated. Connected organizations of shared channels are stored under the same
    /// workspace with their own ids, so the id pin keeps the own record answerable.
    pub fn team(&self, workspace: &TeamId) -> Result<Option<Team>> {
        let json = self
            .conn
            .query_row(
                "SELECT json FROM team WHERE workspace_id = ?1 AND id = ?1",
                params![workspace.as_str()],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(json.map(|json| serde_json::from_str(&json)).transpose()?)
    }

    /// Every stored team of `workspace`: its own record first, then the connected
    /// organizations Slack Connect shared channels introduced, by name.
    pub fn teams(&self, workspace: &TeamId) -> Result<Vec<Team>> {
        let teams: Vec<Team> = self
            .json_column("SELECT json FROM team WHERE workspace_id = ?1", workspace)?
            .iter()
            .map(|json| serde_json::from_str(json))
            .collect::<std::result::Result<_, _>>()?;
        let mut sorted = teams;
        sorted.sort_by(|a, b| {
            let own = |team: &Team| team.id == *workspace;
            own(b).cmp(&own(a)).then_with(|| a.name.cmp(&b.name))
        });
        Ok(sorted)
    }

    /// The distinct team ids, other than `workspace`, that the stored profiles of `members`
    /// belong to: the organizations a shared channel's external people are from.
    pub fn member_team_ids(&self, workspace: &TeamId, members: &[UserId]) -> Result<Vec<TeamId>> {
        let ask: Vec<&UserId> = members
            .iter()
            .filter(|id| id.as_str() != "USLACKBOT")
            .collect();
        if ask.is_empty() {
            return Ok(Vec::new());
        }
        // `team_id` lives in the stored JSON blob, not a column, so the rows parse here;
        // unreadable profiles read as own-team (a missing profile never invents an org).
        let mut teams: Vec<TeamId> = Vec::new();
        query_in_chunks(
            &self.conn,
            workspace,
            &ask,
            |id| id.as_str(),
            r#"SELECT json FROM "user" WHERE workspace_id = ?1 AND id"#,
            |r| r.get::<_, String>(0),
            |raw| {
                let Ok(user) = serde_json::from_str::<User>(&raw) else {
                    return;
                };
                if let Some(team) = user.team_id
                    && team != *workspace
                    && !teams.contains(&team)
                {
                    teams.push(team);
                }
            },
        )?;
        Ok(teams)
    }

    /// Replaces the workspace's sidebar sections wholesale: `users.channelSections.list`
    /// returns the complete set, so a stale section disappears by omission.
    pub fn replace_sections(
        &mut self,
        workspace: &TeamId,
        sections: &[ChannelSection],
    ) -> Result<()> {
        self.ensure_writable()?;
        let tx = self.conn.transaction()?;
        let ts = now_epoch_secs();
        {
            let mut del =
                tx.prepare_cached("DELETE FROM channel_section WHERE workspace_id = ?1")?;
            del.execute(params![workspace.as_str()])?;
            let mut ins = tx.prepare_cached(
                "INSERT INTO channel_section (workspace_id, id, seq, json, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (seq, section) in sections.iter().enumerate() {
                ins.execute(params![
                    workspace.as_str(),
                    section.id,
                    seq as i64,
                    serde_json::to_string(section)?,
                    ts,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The sidebar sections in display order: the `next_id` chain starting at the section no
    /// other section points to, then any leftovers in stored order.
    pub fn sections(&self, workspace: &TeamId) -> Result<Vec<ChannelSection>> {
        let stored: Vec<ChannelSection> = self
            .json_column(
                "SELECT json FROM channel_section WHERE workspace_id = ?1 ORDER BY seq",
                workspace,
            )?
            .iter()
            .map(|json| Ok(serde_json::from_str(json)?))
            .collect::<Result<_>>()?;
        Ok(display_order(stored))
    }

    /// Replaces the workspace's custom emoji (`emoji.list`); `alias:<name>` values are stored
    /// as received and resolved on read.
    pub fn replace_custom_emoji(
        &mut self,
        workspace: &TeamId,
        emoji: &[(String, String)],
    ) -> Result<()> {
        self.ensure_writable()?;
        let tx = self.conn.transaction()?;
        {
            let mut del = tx.prepare_cached("DELETE FROM custom_emoji WHERE workspace_id = ?1")?;
            del.execute(params![workspace.as_str()])?;
            let mut ins = tx.prepare_cached(
                "INSERT INTO custom_emoji (workspace_id, name, url) VALUES (?1, ?2, ?3)",
            )?;
            for (name, url) in emoji {
                ins.execute(params![workspace.as_str(), name, url])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Custom emoji as `(name, image_url)` pairs, by name; `alias:<name>` values are resolved
    /// to their target's URL and unresolvable aliases are dropped.
    pub fn custom_emoji(&self, workspace: &TeamId) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT name, url FROM custom_emoji WHERE workspace_id = ?1 ORDER BY name",
        )?;
        let rows = stmt.query_map(params![workspace.as_str()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let raw: Vec<(String, String)> = rows.collect::<rusqlite::Result<_>>()?;
        let by_name: HashMap<&str, &str> = raw
            .iter()
            .map(|(name, url)| (name.as_str(), url.as_str()))
            .collect();
        Ok(raw
            .iter()
            .filter_map(|(name, url)| {
                resolve_alias(&by_name, url).map(|url| (name.clone(), url.to_owned()))
            })
            .collect())
    }

    /// Stores the signed-in user's prefs blob (`client.userBoot.prefs`).
    pub fn replace_prefs(&mut self, workspace: &TeamId, prefs: &serde_json::Value) -> Result<()> {
        self.ensure_writable()?;
        self.conn.execute(
            "INSERT INTO prefs (workspace_id, json, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (workspace_id) DO UPDATE SET
                 json = excluded.json, updated_at = excluded.updated_at",
            params![
                workspace.as_str(),
                serde_json::to_string(prefs)?,
                now_epoch_secs()
            ],
        )?;
        Ok(())
    }

    /// A slackless-local value for `workspace` (state Slack does not keep for this client).
    pub fn local(&self, workspace: &TeamId, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM local_state WHERE workspace_id = ?1 AND key = ?2",
                params![workspace.as_str(), key],
                |r| r.get::<_, String>(0),
            )
            .optional()?)
    }

    /// Reads every slackless-local key/value for `workspace` in one query (avoids N separate
    /// lock + SQLite round-trips at startup).
    pub fn all_local(&self, workspace: &TeamId) -> Result<HashMap<String, String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT key, value FROM local_state WHERE workspace_id = ?1")?;
        let rows = stmt.query_map(params![workspace.as_str()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (k, v) = row?;
            out.insert(k, v);
        }
        Ok(out)
    }

    /// Sets a slackless-local value for `workspace`, replacing any previous one.
    pub fn set_local(&mut self, workspace: &TeamId, key: &str, value: &str) -> Result<()> {
        self.ensure_writable()?;
        self.conn.execute(
            "INSERT INTO local_state (workspace_id, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT (workspace_id, key) DO UPDATE SET value = excluded.value",
            params![workspace.as_str(), key, value],
        )?;
        Ok(())
    }

    /// The signed-in user's prefs blob, if hydrated (e.g. `display_real_names`).
    pub fn prefs(&self, workspace: &TeamId) -> Result<Option<serde_json::Value>> {
        let json = self
            .conn
            .query_row(
                "SELECT json FROM prefs WHERE workspace_id = ?1",
                params![workspace.as_str()],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(json.map(|json| serde_json::from_str(&json)).transpose()?)
    }

    /// The `json` column of `sql`'s rows; `?1` is bound to `workspace`.
    fn json_column(&self, sql: &str, workspace: &TeamId) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(sql)?;
        let rows = stmt.query_map(params![workspace.as_str()], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

/// Joins each `file_transcript ft` row with the newest live message carrying its file
/// (`message_file` -> `message_current`) as `carrier`, whose columns are NULL when no live message
/// carries the file. One correlated lookup per row, on `message_file`'s primary key.
const NEWEST_FILE_CARRIER_JOIN: &str = "LEFT JOIN message_current carrier ON carrier.id = (
        SELECT mf.message_id
        FROM message_file mf
        JOIN message_current mc ON mc.id = mf.message_id
        WHERE mf.workspace_id = ft.workspace_id AND mf.file_id = ft.file_id AND mc.deleted = 0
        ORDER BY mc.ts_key DESC
        LIMIT 1)";

use self::messages::*;

/// The stored JSON of one conversation, decoded.
fn load_conversation(
    conn: &Connection,
    workspace: &TeamId,
    id: &ChannelId,
) -> Result<Option<Conversation>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT json FROM conversation WHERE workspace_id = ?1 AND id = ?2",
            params![workspace.as_str(), id.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    Ok(json.map(|j| serde_json::from_str(&j)).transpose()?)
}

/// Inserts `conv` or replaces its row wholesale, stamped `updated_at = ts`.
fn upsert_conversation_in_tx(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    conv: &Conversation,
    ts: i64,
) -> Result<()> {
    let kind = match conv.kind() {
        ConversationKind::PublicChannel => "public_channel",
        ConversationKind::PrivateChannel => "private_channel",
        ConversationKind::Im => "im",
        ConversationKind::Mpim => "mpim",
    };
    let mut stmt = tx.prepare_cached(
        "INSERT INTO conversation
             (workspace_id, id, name, kind, is_archived, is_member, json, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (workspace_id, id) DO UPDATE SET
             name = excluded.name, kind = excluded.kind, is_archived = excluded.is_archived,
             is_member = excluded.is_member, json = excluded.json,
             updated_at = excluded.updated_at",
    )?;
    stmt.execute(params![
        workspace.as_str(),
        conv.id.as_str(),
        conv.name,
        kind,
        conv.is_archived,
        conv.is_member,
        serde_json::to_string(conv)?,
        ts,
    ])?;
    Ok(())
}

/// Builds `?, ?, ...` with `n` positional SQL placeholders.
pub(super) fn placeholders(n: usize) -> String {
    let mut out = String::with_capacity(n.saturating_mul(3));
    for i in 0..n {
        if i > 0 {
            out.push_str(", ");
        }
        out.push('?');
    }
    out
}

/// Runs a workspace-scoped `... IN (?, ...)` query across chunks of at most [`SQL_IN_CHUNK`]
/// values, passing each mapped row to `on_row`. `sql_before_in` must bind `?1` to `workspace_id`
/// and end immediately before `IN (...)`.
pub(super) fn query_in_chunks<T, R>(
    conn: &Connection,
    workspace: &TeamId,
    items: &[T],
    as_str: impl Fn(&T) -> &str,
    sql_before_in: &str,
    mut map_row: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<R>,
    mut on_row: impl FnMut(R),
) -> Result<()> {
    for chunk in items.chunks(SQL_IN_CHUNK) {
        let sql = format!("{sql_before_in} IN ({})", placeholders(chunk.len()));
        let mut args = Vec::with_capacity(chunk.len() + 1);
        args.push(SqlValue::Text(workspace.as_str().to_owned()));
        args.extend(
            chunk
                .iter()
                .map(|item| SqlValue::Text(as_str(item).to_owned())),
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), &mut map_row)?;
        for row in rows {
            on_row(row?);
        }
    }
    Ok(())
}

/// Orders sections for display: the `next_id` chain starting at the section no other section
/// points to, then every remaining section in stored order.
fn display_order(stored: Vec<ChannelSection>) -> Vec<ChannelSection> {
    let referenced: HashSet<&str> = stored
        .iter()
        .filter_map(|section| section.next_id.as_deref())
        .filter(|id| !id.is_empty())
        .collect();
    let head = stored.iter().find(|s| !referenced.contains(s.id.as_str()));
    let Some(head) = head else {
        return stored; // every section is pointed at (a cycle): stored order is the fallback
    };
    let head_id = head.id.clone();
    let index_by_id: HashMap<&str, usize> = stored
        .iter()
        .enumerate()
        .map(|(index, s)| (s.id.as_str(), index))
        .collect();
    let mut emitted = vec![false; stored.len()];
    let mut order: Vec<usize> = Vec::with_capacity(stored.len());
    let mut cursor = Some(head_id.as_str());
    while let Some(id) = cursor {
        let Some(&index) = index_by_id.get(id) else {
            break; // next_id points at a section that is not stored
        };
        if emitted[index] {
            break; // cycle guard
        }
        emitted[index] = true;
        cursor = stored[index].next_id.as_deref();
        order.push(index);
    }
    order
        .into_iter()
        .chain((0..stored.len()).filter(|&index| !emitted[index]))
        .map(|index| stored[index].clone())
        .collect()
}

/// Follows an `alias:<name>` chain to its target's URL; `None` when the chain dangles or
/// cycles. A plain URL passes through unchanged.
fn resolve_alias<'a>(by_name: &HashMap<&'a str, &'a str>, url: &'a str) -> Option<&'a str> {
    let mut url = url;
    let mut visited: HashSet<&str> = HashSet::new();
    while let Some(target) = url.strip_prefix("alias:") {
        if !visited.insert(target) {
            return None; // alias cycle
        }
        url = *by_name.get(target)?;
    }
    Some(url)
}
