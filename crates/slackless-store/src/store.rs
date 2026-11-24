//! The blocking [`Store`]. Drive it from a dedicated thread or `spawn_blocking`.

use std::collections::{HashMap, HashSet};
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, OptionalExtension, Transaction, params, params_from_iter};

use crate::Result;
use crate::query::{Has, SearchQuery};
use crate::schema;
use crate::types::{
    BotId, BotProfile, ChannelId, ChannelSection, Conversation, ConversationKind, Message,
    MessageSubtype, Team, TeamId, Ts, User, UserId,
};

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
        let _ = self.conn.execute_batch("PRAGMA optimize;");
    }
}

use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic revision bumped whenever message rows are mutated in any [`Store`] instance.
/// Lets secondary windows skip idle SQLite polling with a single atomic load.
static MESSAGE_REVISION: AtomicU64 = AtomicU64::new(1);

/// Durable local-first message store; the rendering source of truth.
pub struct Store {
    pub(crate) conn: Connection,
}

impl Store {
    /// Current monotonic message-mutation revision (O(1) atomic read, zero SQLite I/O).
    pub fn message_revision() -> u64 {
        MESSAGE_REVISION.load(Ordering::Acquire)
    }

    #[inline]
    fn bump_message_revision() {
        MESSAGE_REVISION.fetch_add(1, Ordering::Release);
    }
    /// Opens (creating if needed) the database at `path` and migrates it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
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
        let flags =
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let _ = conn.execute_batch(
            "PRAGMA query_only = ON;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -2000;
             PRAGMA mmap_size = 8388608;",
        );
        Ok(Self { conn })
    }

    /// A fresh in-memory store (tests, ephemeral sessions).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let _ = conn.execute_batch("PRAGMA temp_store = MEMORY; PRAGMA cache_size = -8000;");
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Applies pending schema migrations. Idempotent; `open*` already calls it.
    pub fn migrate(&mut self) -> Result<()> {
        schema::migrate(&mut self.conn)
    }

    /// The applied schema version (`PRAGMA user_version`).
    pub fn schema_version(&self) -> Result<u32> {
        schema::schema_version(&self.conn)
    }

    /// Records a message observed from history, replies, `chat.*` or the websocket.
    ///
    /// For a `message_changed` event pass its inner `message`: the edit is keyed by `edited.ts`
    /// and appended as a new revision. The current state follows the newest revision; once a
    /// message is deleted, later (stale) observations are logged but never resurrect it.
    pub fn upsert_message(
        &mut self,
        workspace: &TeamId,
        channel: &ChannelId,
        message: &Message,
    ) -> Result<()> {
        self.upsert_messages(workspace, channel, std::slice::from_ref(message))
    }

    /// Records a batch of messages for `channel` in a single SQLite transaction (one WAL commit and
    /// one revision bump for the whole page).
    pub fn upsert_messages(
        &mut self,
        workspace: &TeamId,
        channel: &ChannelId,
        messages: &[Message],
    ) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        for message in messages {
            upsert_message_in_tx(&tx, workspace, channel, message)?;
        }
        tx.commit()?;
        Self::bump_message_revision();
        Ok(())
    }

    /// Forgets the current state of ephemeral messages ("Only visible to you", like Giphy's
    /// preview): Slack keeps no copy, so one left from a past session would otherwise show
    /// forever. Their revisions stay in the log. Returns how many were dropped.
    pub fn drop_ephemeral(&mut self, workspace: &TeamId) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM message_current
             WHERE workspace_id = ?1 AND typeof(json) = 'text'
               AND json_extract(json, '$.is_ephemeral') = 1",
            params![workspace.as_str()],
        )?;
        if n > 0 {
            Self::bump_message_revision();
        }
        Ok(n)
    }

    /// Records a `message_deleted` as a tombstone revision carrying the prior content
    /// (`previous`, else the current stored state). Nothing is removed.
    ///
    /// `event_ts` keys the tombstone; when absent the current time is used.
    pub fn record_deletion(
        &mut self,
        workspace: &TeamId,
        channel: &ChannelId,
        deleted_ts: &Ts,
        event_ts: Option<&Ts>,
        previous: Option<&Message>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        archive_current_as_revision(&tx, workspace, channel, deleted_ts)?;
        let mut prior = match previous {
            Some(m) => m.clone(),
            None => {
                let stored: Option<String> = tx
                    .query_row(
                        "SELECT json FROM message_current
                         WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
                        params![workspace.as_str(), channel.as_str(), deleted_ts.as_str()],
                        |r| decode_json_value(r.get_ref(0)?),
                    )
                    .optional()?;
                match stored {
                    Some(json) => serde_json::from_str(&json)?,
                    None => Message::default(),
                }
            }
        };
        prior.ts = deleted_ts.clone();
        let edit_ts = event_ts.map_or_else(now_ts, |t| t.as_str().to_owned());
        let json = serde_json::to_string(&prior)?;
        let stored_json = encode_json_value(&json, is_ephemeral_message(&prior));
        insert_revision(
            &tx,
            workspace,
            channel,
            &prior,
            &edit_ts,
            true,
            &stored_json,
        )?;
        write_current(
            &tx,
            workspace,
            channel,
            &prior,
            &edit_ts,
            true,
            &stored_json,
        )?;
        tx.commit()?;
        Self::bump_message_revision();
        Ok(())
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
        if conversations.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        let ts = now_secs();
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO conversation
                     (workspace_id, id, name, kind, is_archived, is_member, json, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (workspace_id, id) DO UPDATE SET
                     name = excluded.name, kind = excluded.kind, is_archived = excluded.is_archived,
                     is_member = excluded.is_member, json = excluded.json,
                     updated_at = excluded.updated_at",
            )?;
            for conv in conversations {
                let kind = match conv.kind() {
                    ConversationKind::PublicChannel => "public_channel",
                    ConversationKind::PrivateChannel => "private_channel",
                    ConversationKind::Im => "im",
                    ConversationKind::Mpim => "mpim",
                };
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
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_user(&mut self, workspace: &TeamId, user: &User) -> Result<()> {
        self.upsert_users(workspace, std::slice::from_ref(user))
    }

    /// Upsert a batch of users in a single SQLite transaction.
    pub fn upsert_users(&mut self, workspace: &TeamId, users: &[User]) -> Result<()> {
        if users.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        let ts = now_secs();
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
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM conversation WHERE workspace_id = ?1 AND id = ?2",
                params![workspace.as_str(), id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.map(|j| serde_json::from_str(&j)).transpose()?)
    }

    /// A `conversations.info` refresh of a stored conversation: overlays `fresh` without
    /// clobbering richer stored fields ([`Conversation::merge`]). A conversation not yet
    /// stored is inserted as-is.
    pub fn merge_conversation(&mut self, workspace: &TeamId, fresh: &Conversation) -> Result<()> {
        let merged = match self.conversation(workspace, &fresh.id)? {
            Some(mut stored) => {
                stored.merge(fresh);
                stored
            }
            None => fresh.clone(),
        };
        self.upsert_conversation(workspace, &merged)
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
        Ok(collect(row.into_iter().map(Ok))?.into_iter().next())
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
        self.conn.execute(
            "INSERT INTO team (workspace_id, id, json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (workspace_id, id) DO UPDATE SET
                 json = excluded.json, updated_at = excluded.updated_at",
            params![
                workspace.as_str(),
                team.id.as_str(),
                serde_json::to_string(team)?,
                now_secs(),
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
        // Chunk IN lists to stay well below SQLITE_MAX_VARIABLE_NUMBER on large workspaces.
        let mut teams: Vec<TeamId> = Vec::new();
        for chunk in ask.chunks(SQL_IN_CHUNK) {
            let sql = format!(
                r#"SELECT json FROM "user" WHERE workspace_id = ?1 AND id IN ({})"#,
                vec!["?"; chunk.len()].join(", ")
            );
            let mut args = Vec::with_capacity(chunk.len() + 1);
            args.push(SqlValue::Text(workspace.as_str().to_owned()));
            args.extend(
                chunk
                    .iter()
                    .map(|id| SqlValue::Text(id.as_str().to_owned())),
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(args), |r| r.get::<_, String>(0))?;
            for row in rows {
                let Ok(user) = serde_json::from_str::<User>(&row?) else {
                    continue;
                };
                if let Some(team) = user.team_id {
                    if team != *workspace && !teams.contains(&team) {
                        teams.push(team);
                    }
                }
            }
        }
        Ok(teams)
    }

    /// Replaces the workspace's sidebar sections wholesale: `users.channelSections.list`
    /// returns the complete set, so a stale section disappears by omission.
    pub fn replace_sections(
        &mut self,
        workspace: &TeamId,
        sections: &[ChannelSection],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM channel_section WHERE workspace_id = ?1",
            params![workspace.as_str()],
        )?;
        for (seq, section) in sections.iter().enumerate() {
            tx.execute(
                "INSERT INTO channel_section (workspace_id, id, seq, json, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    workspace.as_str(),
                    section.id,
                    seq as i64,
                    serde_json::to_string(section)?,
                    now_secs(),
                ],
            )?;
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
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM custom_emoji WHERE workspace_id = ?1",
            params![workspace.as_str()],
        )?;
        for (name, url) in emoji {
            tx.execute(
                "INSERT INTO custom_emoji (workspace_id, name, url) VALUES (?1, ?2, ?3)",
                params![workspace.as_str(), name, url],
            )?;
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
        self.conn.execute(
            "INSERT INTO prefs (workspace_id, json, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (workspace_id) DO UPDATE SET
                 json = excluded.json, updated_at = excluded.updated_at",
            params![
                workspace.as_str(),
                serde_json::to_string(prefs)?,
                now_secs()
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

    /// Up to `limit` channel-level messages (roots, plain messages, thread broadcasts) older
    /// than `before` (newest when `None`), oldest first. Deleted messages are included.
    pub fn channel_history(
        &self,
        workspace: &TeamId,
        channel: &ChannelId,
        before: Option<&Ts>,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        let before_key = before.map_or(i64::MAX, ts_key);
        let mut stmt = self.conn.prepare_cached(
            "SELECT channel_id, json, deleted FROM (
                 SELECT channel_id, json, deleted, ts_key FROM message_current
                 WHERE workspace_id = ?1 AND channel_id = ?2 AND ts_key < ?3
                   AND (thread_ts IS NULL OR thread_ts = ts OR subtype = ?4)
                 ORDER BY ts_key DESC LIMIT ?5)
             ORDER BY ts_key ASC",
        )?;
        let rows = stmt.query_map(
            params![
                workspace.as_str(),
                channel.as_str(),
                before_key,
                MessageSubtype::ThreadBroadcast.as_str(),
                limit as i64,
            ],
            raw_row,
        )?;
        collect(rows)
    }

    /// Each of `channels`' newest undeleted channel-level message (roots, plain messages and
    /// thread broadcasts; replies excluded), using the `(workspace_id, channel_id, ts_key)` index
    /// directly (`ORDER BY ts_key DESC LIMIT 1` per channel) so large DM histories are never
    /// scanned by a window function.
    pub fn latest_messages(
        &self,
        workspace: &TeamId,
        channels: &[ChannelId],
    ) -> Result<HashMap<ChannelId, StoredMessage>> {
        if channels.is_empty() {
            return Ok(HashMap::new());
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT channel_id, json, deleted FROM message_current
             WHERE workspace_id = ?1 AND channel_id = ?2
               AND deleted = 0
               AND (thread_ts IS NULL OR thread_ts = ts OR subtype = ?3)
             ORDER BY ts_key DESC LIMIT 1",
        )?;
        let broadcast = MessageSubtype::ThreadBroadcast.as_str();
        let ws = workspace.as_str();
        let mut out = HashMap::with_capacity(channels.len());
        for channel in channels {
            let row = stmt
                .query_row(params![ws, channel.as_str(), broadcast], raw_row)
                .optional()?;
            if let Some(raw) = row {
                let (ch, json, deleted) = raw;
                let ch_id = ChannelId::from(ch);
                let mut message: Message = serde_json::from_str(&json)?;
                message.channel.get_or_insert_with(|| ch_id.clone());
                out.insert(
                    channel.clone(),
                    StoredMessage {
                        channel: ch_id,
                        message,
                        deleted,
                    },
                );
            }
        }
        Ok(out)
    }

    /// Up to `limit` newest undeleted messages sent by `user` across all conversations in
    /// `workspace`, newest first (used to populate Drafts & sent -> Sent across restarts).
    pub fn recent_messages_by_user(
        &self,
        workspace: &TeamId,
        user: &UserId,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT channel_id, json, deleted FROM message_current
             WHERE workspace_id = ?1 AND user_id = ?2 AND deleted = 0
             ORDER BY ts_key DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![workspace.as_str(), user.as_str(), limit as i64],
            raw_row,
        )?;
        collect(rows)
    }

    /// Which of `channels` have at least one stored message, deleted ones included: the
    /// DM-list preview backfill fetches only the conversations this leaves out.
    pub fn channels_with_messages(
        &self,
        workspace: &TeamId,
        channels: &[ChannelId],
    ) -> Result<HashSet<ChannelId>> {
        if channels.is_empty() {
            return Ok(HashSet::new());
        }
        let mut out = HashSet::new();
        for chunk in channels.chunks(SQL_IN_CHUNK) {
            let sql = format!(
                "SELECT DISTINCT channel_id FROM message_current
                 WHERE workspace_id = ?1 AND channel_id IN ({})",
                vec!["?"; chunk.len()].join(", ")
            );
            let mut args = Vec::with_capacity(chunk.len() + 1);
            args.push(SqlValue::Text(workspace.as_str().to_owned()));
            args.extend(
                chunk
                    .iter()
                    .map(|channel| SqlValue::Text(channel.as_str().to_owned())),
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(args), |r| r.get::<_, String>(0))?;
            for row in rows {
                out.insert(ChannelId::from(row?));
            }
        }
        Ok(out)
    }

    /// Splits `members` into Slack's header counts (see [`MemberCounts`]). The workspace's
    /// built-in `USLACKBOT` is neither a human nor an app; a deactivated member is neither.
    pub fn member_counts(&self, workspace: &TeamId, members: &[UserId]) -> Result<MemberCounts> {
        /// Slack's built-in workspace bot.
        const USLACKBOT: &str = "USLACKBOT";
        let ask: Vec<&UserId> = members
            .iter()
            .filter(|id| id.as_str() != USLACKBOT)
            .collect();
        let mut counts = MemberCounts {
            humans: ask.len() as u32,
            apps: 0,
        };
        if ask.is_empty() {
            return Ok(counts);
        }
        // Everyone starts as a human (unknown identities included); the stored profiles then
        // re-classify the bots and drop the deactivated.
        for chunk in ask.chunks(SQL_IN_CHUNK) {
            let sql = format!(
                r#"SELECT id, is_bot, deleted FROM "user" WHERE workspace_id = ?1 AND id IN ({})"#,
                vec!["?"; chunk.len()].join(", ")
            );
            let mut args = Vec::with_capacity(chunk.len() + 1);
            args.push(SqlValue::Text(workspace.as_str().to_owned()));
            args.extend(
                chunk
                    .iter()
                    .map(|id| SqlValue::Text(id.as_str().to_owned())),
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(args), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, bool>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            })?;
            for row in rows {
                let (_, is_bot, deleted) = row?;
                if deleted {
                    counts.humans -= 1;
                } else if is_bot {
                    counts.humans -= 1;
                    counts.apps += 1;
                }
            }
        }
        Ok(counts)
    }

    /// Which of `channels` are stored with `is_member = 1` (one query per chunk): the boot hydrate
    /// preserves these flags, because `client.userBoot` objects omit `is_member` and the
    /// membership sync (`users.conversations`) only runs after them.
    pub fn member_flags(
        &self,
        workspace: &TeamId,
        channels: &[ChannelId],
    ) -> Result<HashSet<ChannelId>> {
        if channels.is_empty() {
            return Ok(HashSet::new());
        }
        let mut out = HashSet::new();
        for chunk in channels.chunks(SQL_IN_CHUNK) {
            let sql = format!(
                "SELECT id FROM conversation
                 WHERE workspace_id = ?1 AND is_member = 1 AND id IN ({})",
                vec!["?"; chunk.len()].join(", ")
            );
            let mut args = Vec::with_capacity(chunk.len() + 1);
            args.push(SqlValue::Text(workspace.as_str().to_owned()));
            args.extend(
                chunk
                    .iter()
                    .map(|channel| SqlValue::Text(channel.as_str().to_owned())),
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(args), |r| r.get::<_, String>(0))?;
            for row in rows {
                out.insert(ChannelId::from(row?));
            }
        }
        Ok(out)
    }

    /// Records the signed-in user's channel memberships (`users.conversations`): the ids in
    /// `members` become `is_member = 1`, every other stored public/private channel `0`, the
    /// `json` copy of the flag included. One transaction; rows that already agree are not
    /// rewritten. Returns the ids whose flag flipped, in both directions, so callers announce
    /// exactly the changed rows. IM/MPIM rows are never touched: their flag is not channel
    /// membership.
    pub fn set_channel_memberships(
        &mut self,
        workspace: &TeamId,
        members: &[ChannelId],
    ) -> Result<Vec<ChannelId>> {
        /// Only channel rows carry a membership flag worth syncing.
        const CHANNEL_KINDS: &str = "('public_channel', 'private_channel')";
        let mut flipped = Vec::new();
        let tx = self.conn.transaction()?;
        let now = now_secs();
        if members.len() <= SQL_IN_CHUNK {
            if !members.is_empty() {
                let sql = format!(
                    "UPDATE conversation
                     SET is_member = 1, json = json_set(json, '$.is_member', json('true')),
                         updated_at = ?1
                     WHERE workspace_id = ?2 AND kind IN {CHANNEL_KINDS} AND is_member = 0
                       AND id IN ({}) RETURNING id",
                    vec!["?"; members.len()].join(", ")
                );
                let mut args = vec![
                    SqlValue::Integer(now),
                    SqlValue::Text(workspace.as_str().to_owned()),
                ];
                args.extend(
                    members
                        .iter()
                        .map(|id| SqlValue::Text(id.as_str().to_owned())),
                );
                flipped.extend(flipping(&tx, &sql, args)?);
            }
            let left_out = if !members.is_empty() {
                format!(" AND id NOT IN ({})", vec!["?"; members.len()].join(", "))
            } else {
                String::new()
            };
            let sql = format!(
                "UPDATE conversation
                 SET is_member = 0, json = json_set(json, '$.is_member', json('false')),
                     updated_at = ?1
                 WHERE workspace_id = ?2 AND kind IN {CHANNEL_KINDS} AND is_member = 1{left_out}
                 RETURNING id"
            );
            let mut args = vec![
                SqlValue::Integer(now),
                SqlValue::Text(workspace.as_str().to_owned()),
            ];
            args.extend(
                members
                    .iter()
                    .map(|id| SqlValue::Text(id.as_str().to_owned())),
            );
            flipped.extend(flipping(&tx, &sql, args)?);
        } else {
            tx.execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS _slackless_member_ids (id TEXT PRIMARY KEY) WITHOUT ROWID;
                 DELETE FROM _slackless_member_ids;",
            )?;
            {
                let mut ins =
                    tx.prepare("INSERT OR IGNORE INTO _slackless_member_ids (id) VALUES (?1)")?;
                for id in members {
                    ins.execute(params![id.as_str()])?;
                }
            }
            let sql_join = format!(
                "UPDATE conversation
                 SET is_member = 1, json = json_set(json, '$.is_member', json('true')),
                     updated_at = ?1
                 WHERE workspace_id = ?2 AND kind IN {CHANNEL_KINDS} AND is_member = 0
                   AND id IN (SELECT id FROM _slackless_member_ids)
                 RETURNING id"
            );
            flipped.extend(flipping(
                &tx,
                &sql_join,
                vec![
                    SqlValue::Integer(now),
                    SqlValue::Text(workspace.as_str().to_owned()),
                ],
            )?);
            let sql_leave = format!(
                "UPDATE conversation
                 SET is_member = 0, json = json_set(json, '$.is_member', json('false')),
                     updated_at = ?1
                 WHERE workspace_id = ?2 AND kind IN {CHANNEL_KINDS} AND is_member = 1
                   AND id NOT IN (SELECT id FROM _slackless_member_ids)
                 RETURNING id"
            );
            flipped.extend(flipping(
                &tx,
                &sql_leave,
                vec![
                    SqlValue::Integer(now),
                    SqlValue::Text(workspace.as_str().to_owned()),
                ],
            )?);
            tx.execute("DELETE FROM _slackless_member_ids", [])?;
        }
        tx.commit()?;
        Ok(flipped)
    }

    /// A thread's root and replies, oldest first.
    pub fn thread_replies(
        &self,
        workspace: &TeamId,
        channel: &ChannelId,
        thread_ts: &Ts,
    ) -> Result<Vec<StoredMessage>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT channel_id, json, deleted FROM message_current
             WHERE workspace_id = ?1 AND channel_id = ?2 AND (ts = ?3 OR thread_ts = ?3)
             ORDER BY ts_key ASC",
        )?;
        let rows = stmt.query_map(
            params![workspace.as_str(), channel.as_str(), thread_ts.as_str()],
            raw_row,
        )?;
        collect(rows)
    }

    /// Every revision of a message, in the order observed.
    pub fn revisions(
        &self,
        workspace: &TeamId,
        channel: &ChannelId,
        ts: &Ts,
    ) -> Result<Vec<Revision>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT edit_ts, deleted, json FROM message_revision
             WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3
             ORDER BY recorded_at, edit_ts",
        )?;
        let rows = stmt.query_map(
            params![workspace.as_str(), channel.as_str(), ts.as_str()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, bool>(1)?,
                    decode_json_value(r.get_ref(2)?)?,
                ))
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            let (edit_ts, deleted, json) = row?;
            out.push(Revision {
                edit_ts,
                deleted,
                message: serde_json::from_str(&json)?,
            });
        }
        if out.is_empty() {
            let current = self
                .conn
                .query_row(
                    "SELECT edit_ts, deleted, json FROM message_current
                     WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
                    params![workspace.as_str(), channel.as_str(), ts.as_str()],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, bool>(1)?,
                            decode_json_value(r.get_ref(2)?)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((edit_ts, deleted, json)) = current {
                out.push(Revision {
                    edit_ts,
                    deleted,
                    message: serde_json::from_str(&json)?,
                });
            }
        }
        Ok(out)
    }

    /// Local search: free text via FTS5 MATCH, modifiers as WHERE clauses. Newest first;
    /// deleted messages are excluded. `in:`/`from:` match an id or a stored name.
    pub fn search(
        &self,
        workspace: &TeamId,
        query: &SearchQuery,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        self.search_sorted(workspace, query, crate::query::SearchSort::Recent, limit)
    }

    /// Local search with explicit sort order (`Recent` = newest first; `Relevant` = FTS5 BM25
    /// relevance when free text is present, breaking ties by newest first).
    pub fn search_sorted(
        &self,
        workspace: &TeamId,
        query: &SearchQuery,
        sort: crate::query::SearchSort,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        let f = &query.filters;
        let fts = query.fts_match();
        let rank_relevant = sort == crate::query::SearchSort::Relevant && fts.is_some();
        let mut sql = if rank_relevant {
            String::from(
                "SELECT m.channel_id, m.json, m.deleted FROM message_current m
                 JOIN message_fts ON message_fts.rowid = m.id
                 WHERE m.workspace_id = ? AND m.deleted = 0 AND message_fts MATCH ?",
            )
        } else {
            String::from(
                "SELECT m.channel_id, m.json, m.deleted FROM message_current m
                 WHERE m.workspace_id = ? AND m.deleted = 0",
            )
        };
        let mut args = vec![SqlValue::Text(workspace.as_str().to_owned())];
        if let Some(fts) = fts {
            if rank_relevant {
                args.push(SqlValue::Text(fts));
            } else {
                sql.push_str(
                    " AND m.id IN (SELECT rowid FROM message_fts WHERE message_fts MATCH ?)",
                );
                args.push(SqlValue::Text(fts));
            }
        } else if let Some(excluded_fts) = query.fts_excluded_match() {
            sql.push_str(
                " AND m.id NOT IN (SELECT rowid FROM message_fts WHERE message_fts MATCH ?)",
            );
            args.push(SqlValue::Text(excluded_fts));
        }
        any_of(
            &mut sql,
            &mut args,
            &f.in_channels,
            "m.channel_id = ?1 OR m.channel_id IN (SELECT id FROM conversation
                 WHERE workspace_id = m.workspace_id AND name = ?1 COLLATE NOCASE)",
        );
        none_of(
            &mut sql,
            &mut args,
            &f.exclude_channels,
            "m.channel_id = ?1 OR m.channel_id IN (SELECT id FROM conversation
                 WHERE workspace_id = m.workspace_id AND name = ?1 COLLATE NOCASE)",
        );
        any_of(
            &mut sql,
            &mut args,
            &f.from_users,
            r#"m.user_id = ?1 OR m.user_id IN (SELECT id FROM "user"
                 WHERE workspace_id = m.workspace_id AND (name = ?1 COLLATE NOCASE
                   OR display_name = ?1 COLLATE NOCASE OR real_name = ?1 COLLATE NOCASE))"#,
        );
        none_of(
            &mut sql,
            &mut args,
            &f.exclude_users,
            r#"m.user_id = ?1 OR m.user_id IN (SELECT id FROM "user"
                 WHERE workspace_id = m.workspace_id AND (name = ?1 COLLATE NOCASE
                   OR display_name = ?1 COLLATE NOCASE OR real_name = ?1 COLLATE NOCASE))"#,
        );
        if let Some(before) = f.before {
            sql.push_str(" AND m.created_at < ?");
            args.push(SqlValue::Integer(before.unix_start()));
        }
        if let Some(after) = f.after {
            sql.push_str(" AND m.created_at >= ?");
            args.push(SqlValue::Integer(after.unix_start() + 86_400));
        }
        for has in &f.has {
            sql.push_str(match has {
                Has::Link => " AND m.has_link = 1",
                Has::File => " AND m.has_file = 1",
                Has::Pin => " AND m.is_pinned = 1",
                Has::Reaction => {
                    " AND EXISTS (SELECT 1 FROM reaction r WHERE r.workspace_id = m.workspace_id
                       AND r.channel_id = m.channel_id AND r.ts = m.ts)"
                }
            });
        }
        if let Some(user) = &f.includes_user {
            sql.push_str(" AND instr(m.text, ?) > 0");
            args.push(SqlValue::Text(format!("<@{user}>")));
        }
        if f.threads_only {
            sql.push_str(" AND m.thread_ts IS NOT NULL");
        }
        if rank_relevant {
            sql.push_str(" ORDER BY bm25(message_fts), m.ts_key DESC LIMIT ?");
        } else {
            sql.push_str(" ORDER BY m.ts_key DESC LIMIT ?");
        }
        args.push(SqlValue::Integer(limit as i64));

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), raw_row)?;
        collect(rows)
    }

    /// Summary statistics for `workspace` and overall SQLite database footprint.
    pub fn stats(&self, workspace: &TeamId) -> Result<StoreStats> {
        let ws = workspace.as_str();
        let messages: i64 = self.conn.query_row(
            "SELECT count(*) FROM message_current WHERE workspace_id = ?1",
            params![ws],
            |r| r.get(0),
        )?;
        let rev_rows: i64 = self.conn.query_row(
            "SELECT count(*) FROM message_revision WHERE workspace_id = ?1",
            params![ws],
            |r| r.get(0),
        )?;
        let revised_msgs: i64 = self.conn.query_row(
            "SELECT count(*) FROM (
                 SELECT DISTINCT channel_id, ts FROM message_revision WHERE workspace_id = ?1
             )",
            params![ws],
            |r| r.get(0),
        )?;
        let revisions = (messages - revised_msgs).max(0) + rev_rows;
        let conversations: i64 = self.conn.query_row(
            "SELECT count(*) FROM conversation WHERE workspace_id = ?1",
            params![ws],
            |r| r.get(0),
        )?;
        let users: i64 = self.conn.query_row(
            "SELECT count(*) FROM \"user\" WHERE workspace_id = ?1",
            params![ws],
            |r| r.get(0),
        )?;
        let offline_queued: i64 = self.conn.query_row(
            "SELECT count(*) FROM offline_queue WHERE workspace_id = ?1",
            params![ws],
            |r| r.get(0),
        )?;
        let page_count: i64 = self
            .conn
            .pragma_query_value(None, "page_count", |r| r.get(0))?;
        let page_size: i64 = self
            .conn
            .pragma_query_value(None, "page_size", |r| r.get(0))?;
        Ok(StoreStats {
            messages: messages.max(0) as u64,
            revisions: revisions.max(0) as u64,
            conversations: conversations.max(0) as u64,
            users: users.max(0) as u64,
            offline_queued: offline_queued.max(0) as u64,
            page_count: page_count.max(0) as u64,
            page_size: page_size.max(0) as u64,
        })
    }

    /// Detailed per-table counts and the top `top_n` channels by archived message count.
    pub fn storage_breakdown(&self, workspace: &TeamId, top_n: usize) -> Result<StorageBreakdown> {
        let ws = workspace.as_str();
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
                "reaction",
                "SELECT count(*) FROM reaction WHERE workspace_id = ?1",
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
        ];
        let mut tables = Vec::with_capacity(table_queries.len());
        for (name, sql) in table_queries {
            let count: i64 = self.conn.query_row(sql, params![ws], |r| r.get(0))?;
            tables.push((name.to_owned(), count.max(0) as u64));
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.channel_id, c.name, count(*) AS cnt
             FROM message_current m
             LEFT JOIN conversation c
               ON c.workspace_id = m.workspace_id AND c.id = m.channel_id
             WHERE m.workspace_id = ?1
             GROUP BY m.channel_id
             ORDER BY cnt DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![ws, top_n as i64], |r| {
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
    }

    /// Persist a resolved `bots.info` profile so cold launches render bot names and icons
    /// immediately on frame 0 without waiting on network lookups.
    pub fn upsert_bot_profile(
        &mut self,
        workspace: &TeamId,
        bot_id: &BotId,
        profile: &BotProfile,
    ) -> Result<()> {
        let json = serde_json::to_string(profile)?;
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO bot_profile (workspace_id, bot_id, json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (workspace_id, bot_id) DO UPDATE SET
                json = excluded.json,
                updated_at = excluded.updated_at",
        )?;
        stmt.execute(params![
            workspace.as_str(),
            bot_id.as_str(),
            json,
            now_secs()
        ])?;
        Ok(())
    }

    /// All persisted `bots.info` profiles for `workspace`.
    pub fn bot_profiles(&self, workspace: &TeamId) -> Result<HashMap<BotId, BotProfile>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT bot_id, json FROM bot_profile WHERE workspace_id = ?1")?;
        let rows = stmt.query_map(params![workspace.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, raw) = row?;
            if let Ok(profile) = serde_json::from_str::<BotProfile>(&raw) {
                out.insert(BotId::from(id), profile);
            }
        }
        Ok(out)
    }

    /// Compress any legacy uncompressed `message_current` JSON rows to zlib BLOBs, checkpoint
    /// WAL, compact unused pages (`VACUUM`), and refresh query planner statistics.
    pub fn vacuum(&self) -> Result<()> {
        // Compress legacy plain-TEXT message payloads in batches (skipping ephemeral rows so
        // `drop_ephemeral` can keep using `json_extract`). Because `V6` scopes `message_current_au`
        // to `text, file_names`, updating `json` alone never churns the FTS5 index.
        const BATCH: usize = 5_000;
        let mut last_id: i64 = 0;
        loop {
            let mut batch: Vec<(i64, Vec<u8>)> = Vec::with_capacity(BATCH);
            {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT id, json FROM message_current
                     WHERE id > ?1 AND typeof(json) = 'text' AND length(json) >= 160
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
                        batch.push((id, blob));
                    }
                }
                if seen == 0 {
                    break;
                }
            }
            if !batch.is_empty() {
                self.conn.execute_batch("BEGIN IMMEDIATE")?;
                let update_res = (|| -> Result<()> {
                    let mut upd = self
                        .conn
                        .prepare_cached("UPDATE message_current SET json = ?1 WHERE id = ?2")?;
                    for (id, blob) in &batch {
                        upd.execute(params![blob, id])?;
                    }
                    Ok(())
                })();
                match update_res {
                    Ok(()) => self.conn.execute_batch("COMMIT")?,
                    Err(e) => {
                        let _ = self.conn.execute_batch("ROLLBACK");
                        return Err(e);
                    }
                }
            }
        }
        self.conn.execute_batch(
            "PRAGMA wal_checkpoint(TRUNCATE);
             VACUUM;
             PRAGMA optimize;",
        )?;
        Ok(())
    }

    /// Append a serialized offline mutation payload for `workspace`, returning its row id.
    pub fn enqueue_offline(&self, workspace: &TeamId, payload: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO offline_queue (workspace_id, payload, created_at) VALUES (?1, ?2, ?3)",
            params![workspace.as_str(), payload, now_secs()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// List persisted offline mutations for `workspace` in FIFO order `(id, payload)`.
    pub fn list_offline(&self, workspace: &TeamId) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, payload FROM offline_queue WHERE workspace_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![workspace.as_str()], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Remove one persisted offline mutation by row id (after flush or user discard).
    pub fn delete_offline(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM offline_queue WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Remove all persisted offline mutations for `workspace`.
    pub fn clear_offline(&self, workspace: &TeamId) -> Result<()> {
        self.conn.execute(
            "DELETE FROM offline_queue WHERE workspace_id = ?1",
            params![workspace.as_str()],
        )?;
        Ok(())
    }

    /// Archived thread roots (`reply_count > 0`, undeleted), newest first, optionally scoped
    /// to a single `channel`.
    pub fn recent_threads(
        &self,
        workspace: &TeamId,
        channel: Option<&ChannelId>,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        match channel {
            Some(ch) => {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT channel_id, json, deleted FROM message_current
                     WHERE workspace_id = ?1 AND channel_id = ?2 AND deleted = 0
                       AND COALESCE(reply_count, 0) > 0
                     ORDER BY ts_key DESC LIMIT ?3",
                )?;
                let rows = stmt.query_map(
                    params![workspace.as_str(), ch.as_str(), limit as i64],
                    raw_row,
                )?;
                collect(rows)
            }
            None => {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT channel_id, json, deleted FROM message_current
                     WHERE workspace_id = ?1 AND deleted = 0
                       AND COALESCE(reply_count, 0) > 0
                     ORDER BY ts_key DESC LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![workspace.as_str(), limit as i64], raw_row)?;
                collect(rows)
            }
        }
    }
}

/// Appends ` AND (clause(v1) OR clause(v2) ...)`; `?1` in `clause` is the value placeholder.
fn any_of(sql: &mut String, args: &mut Vec<SqlValue>, values: &[String], clause: &str) {
    if values.is_empty() {
        return;
    }
    let parts: Vec<String> = values
        .iter()
        .map(|v| {
            let occurrences = clause.matches("?1").count();
            args.extend((0..occurrences).map(|_| SqlValue::Text(v.clone())));
            format!("({})", clause.replace("?1", "?"))
        })
        .collect();
    sql.push_str(&format!(" AND ({})", parts.join(" OR ")));
}

/// Appends ` AND NOT (clause(v1) OR clause(v2) ...)`; `?1` in `clause` is the value placeholder.
fn none_of(sql: &mut String, args: &mut Vec<SqlValue>, values: &[String], clause: &str) {
    if values.is_empty() {
        return;
    }
    let parts: Vec<String> = values
        .iter()
        .map(|v| {
            let occurrences = clause.matches("?1").count();
            args.extend((0..occurrences).map(|_| SqlValue::Text(v.clone())));
            format!("({})", clause.replace("?1", "?"))
        })
        .collect();
    sql.push_str(&format!(" AND NOT ({})", parts.join(" OR ")));
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

fn is_ephemeral_message(message: &Message) -> bool {
    message
        .extra
        .get("is_ephemeral")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Compress JSON payloads >= 160 bytes with fast zlib when it saves space (unless `force_text`,
/// used for ephemeral messages so `json_extract(json, '$.is_ephemeral')` stays index-free).
fn encode_json_value(json: &str, force_text: bool) -> SqlValue {
    if !force_text && json.len() >= 160 {
        let mut enc = ZlibEncoder::new(Vec::with_capacity(json.len() / 2), Compression::fast());
        if enc.write_all(json.as_bytes()).is_ok() {
            if let Ok(compressed) = enc.finish() {
                if compressed.len() + 16 < json.len() {
                    return SqlValue::Blob(compressed);
                }
            }
        }
    }
    SqlValue::Text(json.to_owned())
}

/// Decode a `json` column stored as either legacy UTF-8 `TEXT` or zlib-compressed `BLOB`.
fn decode_json_value(val: ValueRef<'_>) -> rusqlite::Result<String> {
    match val {
        ValueRef::Text(bytes) => std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(rusqlite::Error::Utf8Error),
        ValueRef::Blob(bytes) => {
            let mut dec = ZlibDecoder::new(bytes);
            let mut out = String::new();
            dec.read_to_string(&mut out).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Blob,
                    Box::new(e),
                )
            })?;
            Ok(out)
        }
        other => Err(rusqlite::Error::InvalidColumnType(
            0,
            "json".to_owned(),
            other.data_type(),
        )),
    }
}

/// Snapshot the existing `message_current` row into `message_revision` before an edit or deletion
/// replaces it, preserving the original content verbatim in the append-only log.
fn archive_current_as_revision(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    channel: &ChannelId,
    ts: &Ts,
) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO message_revision
             (workspace_id, channel_id, ts, edit_ts, user_id, thread_ts, subtype, text, deleted,
              json, created_at, recorded_at)
         SELECT workspace_id, channel_id, ts, edit_ts, user_id, thread_ts, subtype, text, deleted,
                json, created_at, updated_at
         FROM message_current
         WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
        params![workspace.as_str(), channel.as_str(), ts.as_str()],
    )?;
    Ok(())
}

fn insert_revision(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    channel: &ChannelId,
    message: &Message,
    edit_ts: &str,
    deleted: bool,
    json: &SqlValue,
) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO message_revision
             (workspace_id, channel_id, ts, edit_ts, user_id, thread_ts, subtype, text, deleted,
              json, created_at, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            workspace.as_str(),
            channel.as_str(),
            message.ts.as_str(),
            edit_ts,
            message.user.as_ref().map(|u| u.as_str()),
            message.thread_ts.as_ref().map(|t| t.as_str()),
            message.subtype.as_ref().map(|s| s.as_str()),
            message.text,
            deleted,
            json,
            message.ts.secs(),
            now_secs(),
        ],
    )?;
    Ok(())
}

fn write_current(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    channel: &ChannelId,
    message: &Message,
    edit_ts: &str,
    deleted: bool,
    json: &SqlValue,
) -> Result<()> {
    let file_names = message
        .files
        .iter()
        .flat_map(|f| [f.name.as_deref(), f.title.as_deref()])
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let has_link = message.text.contains("http://")
        || message.text.contains("https://")
        || message
            .attachments
            .iter()
            .any(|a| a.from_url.is_some() || a.title_link.is_some());
    tx.execute(
        "INSERT INTO message_current
             (workspace_id, channel_id, ts, ts_key, edit_ts, user_id, thread_ts, subtype, text,
              file_names, deleted, has_link, has_file, is_pinned, reply_count, json, created_at,
              updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
         ON CONFLICT (workspace_id, channel_id, ts) DO UPDATE SET
             ts_key = excluded.ts_key, edit_ts = excluded.edit_ts, user_id = excluded.user_id,
             thread_ts = excluded.thread_ts, subtype = excluded.subtype, text = excluded.text,
             file_names = excluded.file_names, deleted = excluded.deleted,
             has_link = excluded.has_link, has_file = excluded.has_file,
             is_pinned = excluded.is_pinned, reply_count = excluded.reply_count,
             json = excluded.json, created_at = excluded.created_at,
             updated_at = excluded.updated_at",
        params![
            workspace.as_str(),
            channel.as_str(),
            message.ts.as_str(),
            ts_key(&message.ts),
            edit_ts,
            message.user.as_ref().map(|u| u.as_str()),
            message.thread_ts.as_ref().map(|t| t.as_str()),
            message.subtype.as_ref().map(|s| s.as_str()),
            message.text,
            file_names,
            deleted,
            has_link,
            !message.files.is_empty(),
            !message.pinned_to.is_empty(),
            message.reply_count,
            json,
            message.ts.secs(),
            now_secs(),
        ],
    )?;
    tx.execute(
        "DELETE FROM reaction WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
        params![workspace.as_str(), channel.as_str(), message.ts.as_str()],
    )?;
    for reaction in &message.reactions {
        tx.execute(
            "INSERT OR REPLACE INTO reaction (workspace_id, channel_id, ts, name, count, users)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                workspace.as_str(),
                channel.as_str(),
                message.ts.as_str(),
                reaction.name,
                reaction.count,
                serde_json::to_string(&reaction.users)?,
            ],
        )?;
    }
    Ok(())
}

/// Maximum number of bind parameters per SQL `IN (?, ...)` chunk (well below SQLite's limit).
const SQL_IN_CHUNK: usize = 500;

fn upsert_message_in_tx(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    channel: &ChannelId,
    message: &Message,
) -> Result<()> {
    let edit_ts = message.edited.as_ref().map_or("", |e| e.ts.as_str());
    let json = serde_json::to_string(message)?;
    let ephemeral = is_ephemeral_message(message);
    let stored_json = encode_json_value(&json, ephemeral);
    let current: Option<(String, bool)> = tx
        .query_row(
            "SELECT edit_ts, deleted FROM message_current
             WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
            params![workspace.as_str(), channel.as_str(), message.ts.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let newest = match &current {
        None => true,
        Some((_, true)) => false,
        Some((current_edit, false)) => Ts::from(edit_ts) >= Ts::from(current_edit.as_str()),
    };
    // Avoid duplicating untouched originals in `message_revision` (cuts archive size ~65%):
    // whenever a message is edited, tombstoned, or re-observed with a different revision key,
    // archive the existing `message_current` row first and append the incoming revision.
    let needs_revision = match &current {
        None => false,
        Some((_, true)) => true,
        Some((current_edit, false)) => current_edit.as_str() != edit_ts,
    };
    if needs_revision {
        if current.is_some() {
            archive_current_as_revision(tx, workspace, channel, &message.ts)?;
        }
        insert_revision(
            tx,
            workspace,
            channel,
            message,
            edit_ts,
            false,
            &stored_json,
        )?;
    }
    if newest {
        write_current(
            tx,
            workspace,
            channel,
            message,
            edit_ts,
            false,
            &stored_json,
        )?;
    }
    Ok(())
}

type RawRow = (String, String, bool);

/// Runs one membership `UPDATE ... RETURNING id`, collecting the flipped ids.
fn flipping(tx: &Transaction<'_>, sql: &str, args: Vec<SqlValue>) -> Result<Vec<ChannelId>> {
    let mut stmt = tx.prepare(sql)?;
    let rows = stmt.query_map(params_from_iter(args), |r| r.get::<_, String>(0))?;
    Ok(rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(ChannelId::from)
        .collect())
}

fn raw_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawRow> {
    Ok((r.get(0)?, decode_json_value(r.get_ref(1)?)?, r.get(2)?))
}

fn collect(rows: impl Iterator<Item = rusqlite::Result<RawRow>>) -> Result<Vec<StoredMessage>> {
    let mut out = Vec::new();
    for row in rows {
        let (channel, json, deleted) = row?;
        let channel = ChannelId::from(channel);
        let mut message: Message = serde_json::from_str(&json)?;
        message.channel.get_or_insert_with(|| channel.clone());
        out.push(StoredMessage {
            channel,
            message,
            deleted,
        });
    }
    Ok(out)
}

/// Numeric sort key: microseconds since the epoch (`"1700000000.000100"` -> 1700000000000100).
fn ts_key(ts: &Ts) -> i64 {
    let (secs, frac) = ts.as_str().split_once('.').unwrap_or((ts.as_str(), ""));
    let secs: i64 = secs.parse().unwrap_or(0);
    let micros: i64 = format!("{:0<6}", &frac[..frac.len().min(6)])
        .parse()
        .unwrap_or(0);
    secs.saturating_mul(1_000_000).saturating_add(micros)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn now_ts() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:06}", d.as_secs(), d.subsec_micros())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::SearchFilters;
    use crate::types::{Edited, File, FileId, Reaction, SectionKind, UserId};

    fn ws() -> TeamId {
        TeamId::from("T1")
    }

    #[test]
    fn lists_conversations_and_users_per_workspace() {
        let mut store = Store::open_in_memory().unwrap();
        let conv = |id: &str, name: &str| Conversation {
            id: ChannelId::from(id),
            name: Some(name.into()),
            ..Default::default()
        };
        store
            .upsert_conversation(&ws(), &conv("C2", "random"))
            .unwrap();
        store
            .upsert_conversation(&ws(), &conv("C1", "general"))
            .unwrap();
        store
            .upsert_conversation(&TeamId::from("T2"), &conv("C9", "other"))
            .unwrap();
        let ana = User {
            id: UserId::from("U1"),
            ..Default::default()
        };
        store.upsert_user(&ws(), &ana).unwrap();

        let ids: Vec<_> = store
            .conversations(&ws())
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, [ChannelId::from("C1"), ChannelId::from("C2")]);
        let users: Vec<_> = store
            .users(&ws())
            .unwrap()
            .into_iter()
            .map(|u| u.id)
            .collect();
        assert_eq!(users, [UserId::from("U1")]);
        assert!(store.users(&TeamId::from("T2")).unwrap().is_empty());
    }

    fn general() -> ChannelId {
        ChannelId::from("C1")
    }

    fn msg(ts: &str, user: &str, text: &str) -> Message {
        Message {
            ts: Ts::from(ts),
            user: Some(UserId::from(user)),
            text: text.into(),
            ..Default::default()
        }
    }

    #[test]
    fn local_state_round_trips_per_workspace() {
        let mut store = Store::open_in_memory().unwrap();
        let (a, b) = (TeamId::from("T1"), TeamId::from("T2"));
        assert_eq!(store.local(&a, "last_conversation").unwrap(), None);
        store.set_local(&a, "last_conversation", "C1").unwrap();
        store.set_local(&a, "last_conversation", "C2").unwrap();
        assert_eq!(
            store.local(&a, "last_conversation").unwrap().as_deref(),
            Some("C2")
        );
        assert_eq!(store.local(&b, "last_conversation").unwrap(), None);
    }

    #[test]
    fn migrate_is_idempotent() {
        let mut store = Store::open_in_memory().unwrap();
        assert_eq!(store.schema_version().unwrap(), schema::SCHEMA_VERSION);
        store.migrate().unwrap();
        store.migrate().unwrap();
        assert_eq!(store.schema_version().unwrap(), schema::SCHEMA_VERSION);
        let tables: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name IN
                 ('message_revision','message_current','message_fts','conversation','user',
                  'reaction','team','channel_section','custom_emoji','prefs')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 10);
    }

    #[test]
    fn sections_follow_the_next_id_chain_and_append_leftovers() {
        let mut store = Store::open_in_memory().unwrap();
        let section = |id: &str, next: Option<&str>| ChannelSection {
            id: id.to_owned(),
            name: format!("section {id}"),
            kind: SectionKind::Standard,
            emoji: "rocket".to_owned(),
            next_id: next.map(str::to_owned),
            channel_ids: vec![ChannelId::from("C1"), ChannelId::from("D1")],
        };
        // Stored order c, b, a, d: a points at b and d at c, so a is the only section nothing
        // points at. Display: the chain a -> b, then leftovers c, d in stored order.
        store
            .replace_sections(
                &ws(),
                &[
                    section("c", None),
                    section("b", None),
                    section("a", Some("b")),
                    section("d", Some("c")),
                ],
            )
            .unwrap();

        let sections = store.sections(&ws()).unwrap();
        let ids: Vec<_> = sections.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c", "d"]);
        assert_eq!(sections[0].name, "section a");
        assert_eq!(sections[0].emoji, "rocket");
        assert_eq!(
            sections[0].channel_ids,
            [ChannelId::from("C1"), ChannelId::from("D1")]
        );

        // A second replace swaps the whole set: stale sections disappear by omission.
        store
            .replace_sections(&ws(), &[section("e", None)])
            .unwrap();
        let ids: Vec<_> = store
            .sections(&ws())
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, ["e".to_owned()]);
        assert!(store.sections(&TeamId::from("T2")).unwrap().is_empty());
    }

    #[test]
    fn custom_emoji_resolves_aliases_and_drops_unresolvable() {
        let mut store = Store::open_in_memory().unwrap();
        let url = "https://emoji.example/party.png";
        store
            .replace_custom_emoji(
                &ws(),
                &[
                    ("yay".to_owned(), "alias:party".to_owned()),
                    ("party".to_owned(), url.to_owned()),
                    ("meh".to_owned(), "alias:gone".to_owned()),
                    ("loop".to_owned(), "alias:loop".to_owned()),
                    ("chain".to_owned(), "alias:yay".to_owned()),
                ],
            )
            .unwrap();

        // Name order; `chain` resolves through `yay` to `party`; dangling and cyclic aliases
        // are dropped.
        assert_eq!(
            store.custom_emoji(&ws()).unwrap(),
            vec![
                ("chain".to_owned(), url.to_owned()),
                ("party".to_owned(), url.to_owned()),
                ("yay".to_owned(), url.to_owned()),
            ]
        );
        assert!(store.custom_emoji(&TeamId::from("T2")).unwrap().is_empty());
    }

    #[test]
    fn team_and_prefs_round_trip() {
        let mut store = Store::open_in_memory().unwrap();
        assert!(store.team(&ws()).unwrap().is_none());
        assert!(store.prefs(&ws()).unwrap().is_none());

        let team = |name: &str| Team {
            id: TeamId::from("T1"),
            name: name.to_owned(),
            domain: "acme".to_owned(),
            ..Default::default()
        };
        store.upsert_team(&ws(), &team("Acme")).unwrap();
        store.upsert_team(&ws(), &team("Acme Inc")).unwrap();
        assert_eq!(store.team(&ws()).unwrap().unwrap().name, "Acme Inc");

        let prefs = serde_json::json!({"display_real_names": true});
        store.replace_prefs(&ws(), &prefs).unwrap();
        assert_eq!(store.prefs(&ws()).unwrap(), Some(prefs));
    }

    #[test]
    fn connected_teams_list_behind_the_own_team() {
        let mut store = Store::open_in_memory().unwrap();
        assert!(store.teams(&ws()).unwrap().is_empty());

        let team = |id: &str, name: &str| Team {
            id: TeamId::from(id),
            name: name.to_owned(),
            domain: name.to_lowercase(),
            ..Default::default()
        };
        // The external org sorts before the own id, so a plain scan would shadow it.
        store.upsert_team(&ws(), &team("T0AEXT", "Zeta")).unwrap();
        store
            .upsert_team(&ws(), &team(ws().as_str(), "Acme"))
            .unwrap();
        store.upsert_team(&ws(), &team("T0BEXT", "Beta")).unwrap();

        assert_eq!(
            store.team(&ws()).unwrap().unwrap().name,
            "Acme",
            "the own record stays answerable with connected teams stored beside it"
        );
        let names: Vec<_> = store
            .teams(&ws())
            .unwrap()
            .into_iter()
            .map(|team| team.name)
            .collect();
        assert_eq!(names, ["Acme", "Beta", "Zeta"], "own first, then by name");
    }

    #[test]
    fn member_team_ids_lists_only_external_distinct_teams() {
        let mut store = Store::open_in_memory().unwrap();
        let user = |id: &str, team: Option<&str>| User {
            id: UserId::from(id),
            team_id: team.map(TeamId::from),
            ..Default::default()
        };
        store.upsert_user(&ws(), &user("U1", Some("T1"))).unwrap();
        store
            .upsert_user(&ws(), &user("U2", Some("T0EXT")))
            .unwrap();
        store
            .upsert_user(&ws(), &user("U3", Some("T0EXT")))
            .unwrap();
        store.upsert_user(&ws(), &user("U4", None)).unwrap();

        let members = ["U1", "U2", "U3", "U4", "U9", "USLACKBOT"]
            .map(UserId::from)
            .to_vec();
        assert_eq!(
            store.member_team_ids(&ws(), &members).unwrap(),
            vec![TeamId::from("T0EXT")],
            "own team, duplicates, missing profiles and USLACKBOT never list"
        );
    }

    #[test]
    fn upsert_round_trips_through_channel_history() {
        let mut store = Store::open_in_memory().unwrap();
        let first = msg("1700000000.000100", "U1", "first");
        let second = msg("1700000001.000200", "U2", "second");
        let mut reply = msg("1700000002.000300", "U1", "in thread");
        reply.thread_ts = Some(first.ts.clone());
        for m in [&second, &first, &reply] {
            store.upsert_message(&ws(), &general(), m).unwrap();
        }
        // Re-observing a message is a no-op for the log.
        store.upsert_message(&ws(), &general(), &first).unwrap();

        let history = store.channel_history(&ws(), &general(), None, 50).unwrap();
        let texts: Vec<_> = history.iter().map(|m| m.message.text.as_str()).collect();
        assert_eq!(texts, ["first", "second"]);
        assert_eq!(history[0].message.channel, Some(general()));
        assert!(!history[0].deleted);
        assert_eq!(history[0].message.ts, first.ts);

        let older = store
            .channel_history(&ws(), &general(), Some(&second.ts), 50)
            .unwrap();
        assert_eq!(older.len(), 1);
        let thread = store.thread_replies(&ws(), &general(), &first.ts).unwrap();
        assert_eq!(thread.len(), 2);
        assert_eq!(
            store.revisions(&ws(), &general(), &first.ts).unwrap().len(),
            1
        );
    }

    #[test]
    fn latest_messages_picks_each_channels_newest_top_level() {
        let mut store = Store::open_in_memory().unwrap();
        let (dm1, dm2) = (ChannelId::from("D1"), ChannelId::from("D2"));
        let older = msg("1700000000.000100", "U1", "older");
        let newest = msg("1700000002.000200", "U2", "newest");
        let mut reply = msg("1700000003.000300", "U1", "in thread");
        reply.thread_ts = Some(newest.ts.clone());
        for m in [&older, &newest, &reply] {
            store.upsert_message(&ws(), &dm1, m).unwrap();
        }
        store
            .upsert_message(&ws(), &dm2, &msg("1700000001.000100", "U2", "other dm"))
            .unwrap();
        // Another workspace's message must not leak into the batch.
        store
            .upsert_message(
                &TeamId::from("T2"),
                &dm1,
                &msg("1800000000.000000", "U1", "elsewhere"),
            )
            .unwrap();

        let latest = store
            .latest_messages(&ws(), &[dm1.clone(), dm2.clone(), ChannelId::from("D3")])
            .unwrap();
        assert_eq!(
            latest.len(),
            2,
            "only the asked channels; empty D3 stays absent"
        );
        assert_eq!(
            latest[&dm1].message.text, "newest",
            "a reply is not top-level"
        );
        assert!(!latest[&dm1].deleted);
        assert_eq!(latest[&dm1].message.channel, Some(dm1.clone()));
        assert_eq!(latest[&dm2].message.text, "other dm");

        // A deleted message is never the latest: dm2's only message gone, it has none.
        store
            .record_deletion(&ws(), &dm2, &Ts::from("1700000001.000100"), None, None)
            .unwrap();
        let latest = store
            .latest_messages(&ws(), &[dm1.clone(), dm2.clone()])
            .unwrap();
        assert!(!latest.contains_key(&dm2));
        assert!(latest.contains_key(&dm1));

        assert!(store.latest_messages(&ws(), &[]).unwrap().is_empty());
    }

    #[test]
    fn member_counts_split_humans_apps_and_drop_the_gone() {
        let mut store = Store::open_in_memory().unwrap();
        let user = |id: &str, bot: bool, deleted: bool| User {
            id: UserId::from(id),
            is_bot: bot,
            deleted,
            ..Default::default()
        };
        store.upsert_user(&ws(), &user("U1", false, false)).unwrap();
        store
            .upsert_user(&ws(), &user("UBOT", true, false))
            .unwrap();
        store
            .upsert_user(&ws(), &user("UDEADBOT", true, true))
            .unwrap();
        store
            .upsert_user(&ws(), &user("UGONE", false, true))
            .unwrap();

        let members = [
            "U1",
            "UBOT",
            "UDEADBOT",
            "UGONE",
            // No stored profile: an unknown person is still a person.
            "UEXT",
            // The workspace bot is neither a human nor an app.
            "USLACKBOT",
        ]
        .map(UserId::from)
        .to_vec();

        assert_eq!(
            store.member_counts(&ws(), &members).unwrap(),
            MemberCounts { humans: 2, apps: 1 }
        );
        // Another workspace's profiles never leak in: everyone is an unknown human there.
        assert_eq!(
            store.member_counts(&TeamId::from("T2"), &members).unwrap(),
            MemberCounts { humans: 5, apps: 0 }
        );
        assert_eq!(
            store
                .member_counts(&ws(), &[UserId::from("USLACKBOT")])
                .unwrap(),
            MemberCounts::default()
        );
    }

    #[test]
    fn channels_with_messages_counts_deleted_ones_as_stored() {
        let mut store = Store::open_in_memory().unwrap();
        let dm1 = ChannelId::from("D1");
        store
            .upsert_message(&ws(), &dm1, &msg("1700000000.000100", "U1", "only one"))
            .unwrap();
        let dm2 = ChannelId::from("D2");
        store
            .upsert_message(&ws(), &dm2, &msg("1700000001.000100", "U1", "soon gone"))
            .unwrap();
        store
            .record_deletion(&ws(), &dm2, &Ts::from("1700000001.000100"), None, None)
            .unwrap();

        let asked = [dm1.clone(), dm2.clone(), ChannelId::from("D3")];
        assert_eq!(
            store.channels_with_messages(&ws(), &asked).unwrap(),
            [dm1, dm2].into_iter().collect(),
            "D2's deleted message still means its history was fetched once"
        );
        assert!(store.channels_with_messages(&ws(), &[]).unwrap().is_empty());
    }

    #[test]
    fn set_channel_memberships_flips_channel_rows_only_and_only_once() {
        let mut store = Store::open_in_memory().unwrap();
        let channel = |id: &str, member: bool, private: bool| Conversation {
            id: ChannelId::from(id),
            is_channel: !private,
            is_private: private,
            is_member: member,
            ..Default::default()
        };
        store
            .upsert_conversation(&ws(), &channel("C1", false, false))
            .unwrap();
        store
            .upsert_conversation(&ws(), &channel("C2", true, false))
            .unwrap();
        store
            .upsert_conversation(&ws(), &channel("C3", true, true))
            .unwrap();
        store
            .upsert_conversation(
                &ws(),
                &Conversation {
                    id: ChannelId::from("M1"),
                    is_mpim: true,
                    is_member: true,
                    ..Default::default()
                },
            )
            .unwrap();

        let mut flipped = store
            .set_channel_memberships(&ws(), &[ChannelId::from("C1")])
            .unwrap();
        flipped.sort();
        assert_eq!(
            flipped,
            vec![
                ChannelId::from("C1"),
                ChannelId::from("C2"),
                ChannelId::from("C3"),
            ],
            "joiner up, both leavers down"
        );
        let read = |store: &Store, id: &str| {
            store
                .conversation(&ws(), &ChannelId::from(id))
                .unwrap()
                .unwrap_or_else(|| panic!("{id} stored"))
        };
        assert!(
            read(&store, "C1").is_member,
            "the json copy follows the column"
        );
        assert!(!read(&store, "C2").is_member);
        assert!(!read(&store, "C3").is_member);
        assert!(read(&store, "M1").is_member, "group DM rows are untouched");

        // An unchanged pass rewrites nothing: no flips, not even updated_at.
        let stamps = |store: &Store| -> Vec<(String, i64)> {
            let mut stmt = store
                .conn
                .prepare("SELECT id, updated_at FROM conversation ORDER BY id")
                .unwrap();
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                .unwrap();
            rows.collect::<rusqlite::Result<_>>().unwrap()
        };
        let before = stamps(&store);
        assert!(
            store
                .set_channel_memberships(&ws(), &[ChannelId::from("C1")])
                .unwrap()
                .is_empty()
        );
        assert_eq!(stamps(&store), before, "no churn when nothing flips");

        // An empty membership list resets every channel row.
        let flipped = store.set_channel_memberships(&ws(), &[]).unwrap();
        assert_eq!(flipped, vec![ChannelId::from("C1")]);
        assert!(!read(&store, "C1").is_member);
        assert!(read(&store, "M1").is_member);
    }

    #[test]
    fn edit_appends_revision_and_updates_current() {
        let mut store = Store::open_in_memory().unwrap();
        let original = msg("1700000000.000100", "U1", "teh original");
        store.upsert_message(&ws(), &general(), &original).unwrap();
        let mut edited = msg("1700000000.000100", "U1", "the edited");
        edited.edited = Some(Edited {
            user: Some(UserId::from("U1")),
            ts: Ts::from("1700000050.000000"),
        });
        store.upsert_message(&ws(), &general(), &edited).unwrap();
        // A stale observation of the original must not roll the current state back.
        store.upsert_message(&ws(), &general(), &original).unwrap();

        let history = store.channel_history(&ws(), &general(), None, 10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].message.text, "the edited");

        let revisions = store.revisions(&ws(), &general(), &original.ts).unwrap();
        let texts: Vec<_> = revisions.iter().map(|r| r.message.text.as_str()).collect();
        assert_eq!(texts, ["teh original", "the edited"]);
        assert_eq!(revisions[1].edit_ts, "1700000050.000000");

        // Search follows the current text only.
        assert!(
            store
                .search(&ws(), &SearchQuery::parse("teh"), 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .search(&ws(), &SearchQuery::parse("edited"), 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn deletion_keeps_original_content_in_log() {
        let mut store = Store::open_in_memory().unwrap();
        let original = msg("1700000000.000100", "U1", "secret plans");
        store.upsert_message(&ws(), &general(), &original).unwrap();
        store
            .record_deletion(
                &ws(),
                &general(),
                &original.ts,
                Some(&Ts::from("1700000099.000000")),
                None,
            )
            .unwrap();
        // Late history replay does not resurrect it.
        store.upsert_message(&ws(), &general(), &original).unwrap();

        let history = store.channel_history(&ws(), &general(), None, 10).unwrap();
        assert_eq!(history.len(), 1);
        assert!(history[0].deleted);
        assert_eq!(history[0].message.text, "secret plans");

        let revisions = store.revisions(&ws(), &general(), &original.ts).unwrap();
        assert_eq!(revisions.len(), 2);
        assert!(!revisions[0].deleted);
        assert_eq!(revisions[0].message.text, "secret plans");
        assert!(revisions[1].deleted);
        assert_eq!(revisions[1].message.text, "secret plans");

        // The log refuses mutation.
        assert!(
            store
                .conn
                .execute("DELETE FROM message_revision", [])
                .is_err()
        );
        assert!(
            store
                .search(&ws(), &SearchQuery::parse("secret"), 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn fts_search_with_modifiers() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .upsert_conversation(
                &ws(),
                &Conversation {
                    id: general(),
                    name: Some("general".into()),
                    is_channel: true,
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .upsert_user(
                &ws(),
                &User {
                    id: UserId::from("U2"),
                    name: "ana".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let mut deploy = msg(
            "1704153600.000100",
            "U2",
            "Deploying the rocket today https://x.io",
        );
        deploy.reactions = vec![Reaction {
            name: "rocket".into(),
            count: 1,
            users: vec![UserId::from("U1")],
        }];
        let mut report = msg("1704240000.000100", "U1", "rocket report attached");
        report.files = vec![File {
            id: FileId::from("F1"),
            name: Some("launch-metrics.pdf".into()),
            ..Default::default()
        }];
        store.upsert_message(&ws(), &general(), &deploy).unwrap();
        store.upsert_message(&ws(), &general(), &report).unwrap();
        store
            .upsert_message(
                &ws(),
                &ChannelId::from("C2"),
                &msg("1704240001.0", "U2", "rocket elsewhere"),
            )
            .unwrap();

        let texts = |q: &str| -> Vec<String> {
            store
                .search(&ws(), &SearchQuery::parse(q), 10)
                .unwrap()
                .into_iter()
                .map(|m| m.message.text)
                .collect()
        };
        assert_eq!(texts("deploying").len(), 1);
        assert_eq!(texts("rocket").len(), 3);
        assert_eq!(texts("rocket in:general").len(), 2);
        assert_eq!(
            texts("rocket in:#general from:@ana"),
            ["Deploying the rocket today https://x.io"]
        );
        assert_eq!(texts("rocket in:C2 from:U2"), ["rocket elsewhere"]);
        assert_eq!(texts("launch"), ["rocket report attached"]);
        assert_eq!(texts("has:file").len(), 1);
        assert_eq!(
            texts("has:link"),
            ["Deploying the rocket today https://x.io"]
        );
        assert_eq!(texts("has:reaction").len(), 1);
        assert_eq!(
            texts("rocket after:2024-01-02 in:general"),
            ["rocket report attached"]
        );
        assert_eq!(
            texts("rocket before:2024-01-03"),
            ["Deploying the rocket today https://x.io"]
        );
        assert!(texts("nonexistentword").is_empty());
        assert!(
            texts(r#"rocket" OR "x"#).is_empty(),
            "FTS syntax in input is literal"
        );
    }

    #[test]
    fn filter_chip_constraints() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .upsert_conversation(
                &ws(),
                &Conversation {
                    id: general(),
                    name: Some("general".into()),
                    is_channel: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let mut mention = msg("1704153600.000100", "U2", "ping <@U1> about the rocket");
        mention.thread_ts = Some(Ts::from("1704153600.000000"));
        let plain = msg("1704240000.000100", "U1", "plain rocket");
        let top = msg("1704240010.000100", "U2", "top-level rocket <@U2>");
        store.upsert_message(&ws(), &general(), &mention).unwrap();
        store.upsert_message(&ws(), &general(), &plain).unwrap();
        store.upsert_message(&ws(), &general(), &top).unwrap();

        let found = |filters: SearchFilters| -> Vec<String> {
            let mut query = SearchQuery::parse("rocket");
            query.filters = filters;
            store
                .search(&ws(), &query, 10)
                .unwrap()
                .into_iter()
                .map(|m| m.message.text)
                .collect()
        };
        let threads = SearchFilters {
            threads_only: true,
            ..Default::default()
        };
        assert_eq!(found(threads), ["ping <@U1> about the rocket"]);
        let mentions_me = SearchFilters {
            includes_user: Some("U1".into()),
            ..Default::default()
        };
        assert_eq!(found(mentions_me), ["ping <@U1> about the rocket"]);
        let both = SearchFilters {
            includes_user: Some("U1".into()),
            threads_only: true,
            ..Default::default()
        };
        assert_eq!(found(both).len(), 1);
        let nobody = SearchFilters {
            includes_user: Some("U9".into()),
            ..Default::default()
        };
        assert!(found(nobody).is_empty());
    }

    #[test]
    fn recent_messages_by_user_returns_newest_undeleted_for_user() {
        let mut store = Store::open_in_memory().unwrap();
        let m1 = msg("1700000001.000100", "U1", "first");
        let m2 = msg("1700000002.000100", "U2", "other user");
        let m3 = msg("1700000003.000100", "U1", "second");
        let m4 = msg("1700000004.000100", "U1", "deleted one");
        store.upsert_message(&ws(), &general(), &m1).unwrap();
        store.upsert_message(&ws(), &general(), &m2).unwrap();
        store
            .upsert_message(&ws(), &ChannelId::from("C2"), &m3)
            .unwrap();
        store.upsert_message(&ws(), &general(), &m4).unwrap();
        store
            .record_deletion(&ws(), &general(), &m4.ts, None, Some(&m4))
            .unwrap();

        let rows = store
            .recent_messages_by_user(&ws(), &UserId::from("U1"), 10)
            .unwrap();
        let texts: Vec<&str> = rows.iter().map(|r| r.message.text.as_str()).collect();
        assert_eq!(texts, ["second", "first"]);
    }

    #[test]
    fn ts_key_is_numeric() {
        assert_eq!(
            ts_key(&Ts::from("1700000000.000100")),
            1_700_000_000_000_100
        );
        assert_eq!(ts_key(&Ts::from("1700000000.1")), 1_700_000_000_100_000);
        assert_eq!(ts_key(&Ts::from("1700000000")), 1_700_000_000_000_000);
    }

    #[test]
    fn stats_vacuum_and_offline_queue_round_trip() {
        let mut store = Store::open_in_memory().unwrap();
        let m1 = msg("1700000001.000100", "U1", "hello storage");
        store.upsert_message(&ws(), &general(), &m1).unwrap();
        let id1 = store.enqueue_offline(&ws(), "{\"a\":1}").unwrap();
        let id2 = store.enqueue_offline(&ws(), "{\"b\":2}").unwrap();
        assert!(id2 > id1);

        let stats = store.stats(&ws()).unwrap();
        assert_eq!(stats.messages, 1);
        assert_eq!(stats.revisions, 1);
        assert_eq!(stats.offline_queued, 2);
        assert!(stats.db_bytes() > 0);
        let breakdown = store.storage_breakdown(&ws(), 5).unwrap();
        assert_eq!(breakdown.top_channels.len(), 1);
        assert_eq!(breakdown.top_channels[0].0, "C1");
        assert_eq!(breakdown.top_channels[0].2, 1);

        assert_eq!(
            store.list_offline(&ws()).unwrap(),
            vec![(id1, "{\"a\":1}".to_owned()), (id2, "{\"b\":2}".to_owned())]
        );
        store.delete_offline(id1).unwrap();
        assert_eq!(
            store.list_offline(&ws()).unwrap(),
            vec![(id2, "{\"b\":2}".to_owned())]
        );
        store.clear_offline(&ws()).unwrap();
        assert!(store.list_offline(&ws()).unwrap().is_empty());

        let mut thread_root = msg("1700000002.000100", "U1", "thread starter");
        thread_root.reply_count = Some(3);
        store
            .upsert_message(&ws(), &general(), &thread_root)
            .unwrap();
        let threads = store.recent_threads(&ws(), None, 10).unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].message.text, "thread starter");
        assert!(
            store
                .recent_threads(&ws(), Some(&ChannelId::from("C9")), 10)
                .unwrap()
                .is_empty()
        );

        store.vacuum().unwrap();
    }

    #[test]
    fn all_local_and_read_only_wal_reader_see_committed_writes() {
        let dir =
            std::env::temp_dir().join(format!("slackless-store-ro-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.sqlite");
        let _ = std::fs::remove_file(&path);

        let mut writer = Store::open(&path).unwrap();
        let reader = Store::open_read_only(&path).unwrap();
        writer.set_local(&ws(), "font", "Lato").unwrap();
        writer.set_local(&ws(), "zoom", "3").unwrap();
        let all = reader.all_local(&ws()).unwrap();
        assert_eq!(all.get("font").map(String::as_str), Some("Lato"));
        assert_eq!(all.get("zoom").map(String::as_str), Some("3"));

        drop(reader);
        drop(writer);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compressed_json_round_trips_and_vacuum_compacts_legacy_text() {
        let mut store = Store::open_in_memory().unwrap();
        let long_text = "Slackless archive payload compression test ".repeat(24);
        let m1 = msg("1700000100.000100", "U1", &long_text);
        store.upsert_message(&ws(), &general(), &m1).unwrap();

        // Untouched original lives in `message_current` as a compressed BLOB without duplicating
        // into `message_revision`, yet `revisions()` still returns the single original revision.
        let kind: String = store
            .conn
            .query_row(
                "SELECT typeof(json) FROM message_current WHERE ts = '1700000100.000100'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kind, "blob");
        let rev_count: i64 = store
            .conn
            .query_row("SELECT count(*) FROM message_revision", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rev_count, 0);
        let revs = store.revisions(&ws(), &general(), &m1.ts).unwrap();
        assert_eq!(revs.len(), 1);
        assert_eq!(revs[0].message.text, long_text);

        // Simulate a legacy uncompressed TEXT row and verify `vacuum()` compresses it in place.
        let legacy_json = serde_json::to_string(&m1).unwrap();
        store
            .conn
            .execute(
                "UPDATE message_current SET json = ?1 WHERE ts = '1700000100.000100'",
                params![legacy_json],
            )
            .unwrap();
        store.vacuum().unwrap();
        let kind_after: String = store
            .conn
            .query_row(
                "SELECT typeof(json) FROM message_current WHERE ts = '1700000100.000100'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kind_after, "blob");
        let hist = store.channel_history(&ws(), &general(), None, 10).unwrap();
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0].message.text, long_text);
    }
}
