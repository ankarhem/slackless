//! The blocking [`Store`]. Drive it from a dedicated thread or `spawn_blocking`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Transaction, params, params_from_iter};

use crate::Result;
use crate::query::{Has, SearchQuery};
use crate::schema;
use crate::types::{
    ChannelId, ChannelSection, Conversation, ConversationKind, Message, MessageSubtype, Team,
    TeamId, Ts, User,
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

/// One entry of a message's append-only revision log.
#[derive(Debug, Clone, PartialEq)]
pub struct Revision {
    /// `""` for the original post, `edited.ts` for an edit, the deletion event ts for a tombstone.
    pub edit_ts: String,
    pub deleted: bool,
    pub message: Message,
}

/// Durable local-first message store; the rendering source of truth.
pub struct Store {
    pub(crate) conn: Connection,
}

impl Store {
    /// Opens (creating if needed) the database at `path` and migrates it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        Self::init(conn)
    }

    /// A fresh in-memory store (tests, ephemeral sessions).
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
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
        let edit_ts = message.edited.as_ref().map_or("", |e| e.ts.as_str());
        let json = serde_json::to_string(message)?;
        let tx = self.conn.transaction()?;
        insert_revision(&tx, workspace, channel, message, edit_ts, false, &json)?;
        let current: Option<(String, bool)> = tx
            .query_row(
                "SELECT edit_ts, deleted FROM message_current
                 WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
                params![workspace.as_str(), channel.as_str(), message.ts.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let newest = match current {
            None => true,
            Some((_, true)) => false,
            Some((current_edit, false)) => Ts::from(edit_ts) >= Ts::from(current_edit),
        };
        if newest {
            write_current(&tx, workspace, channel, message, edit_ts, false, &json)?;
        }
        tx.commit()?;
        Ok(())
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
        let mut prior = match previous {
            Some(m) => m.clone(),
            None => {
                let stored: Option<String> = tx
                    .query_row(
                        "SELECT json FROM message_current
                         WHERE workspace_id = ?1 AND channel_id = ?2 AND ts = ?3",
                        params![workspace.as_str(), channel.as_str(), deleted_ts.as_str()],
                        |r| r.get(0),
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
        insert_revision(&tx, workspace, channel, &prior, &edit_ts, true, &json)?;
        write_current(&tx, workspace, channel, &prior, &edit_ts, true, &json)?;
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_conversation(&mut self, workspace: &TeamId, conv: &Conversation) -> Result<()> {
        let kind = match conv.kind() {
            ConversationKind::PublicChannel => "public_channel",
            ConversationKind::PrivateChannel => "private_channel",
            ConversationKind::Im => "im",
            ConversationKind::Mpim => "mpim",
        };
        self.conn.execute(
            "INSERT INTO conversation
                 (workspace_id, id, name, kind, is_archived, is_member, json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (workspace_id, id) DO UPDATE SET
                 name = excluded.name, kind = excluded.kind, is_archived = excluded.is_archived,
                 is_member = excluded.is_member, json = excluded.json,
                 updated_at = excluded.updated_at",
            params![
                workspace.as_str(),
                conv.id.as_str(),
                conv.name,
                kind,
                conv.is_archived,
                conv.is_member,
                serde_json::to_string(conv)?,
                now_secs(),
            ],
        )?;
        Ok(())
    }

    pub fn upsert_user(&mut self, workspace: &TeamId, user: &User) -> Result<()> {
        self.conn.execute(
            r#"INSERT INTO "user"
                   (workspace_id, id, name, real_name, display_name, is_bot, deleted, json,
                    updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
               ON CONFLICT (workspace_id, id) DO UPDATE SET
                   name = excluded.name, real_name = excluded.real_name,
                   display_name = excluded.display_name, is_bot = excluded.is_bot,
                   deleted = excluded.deleted, json = excluded.json,
                   updated_at = excluded.updated_at"#,
            params![
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
                now_secs(),
            ],
        )?;
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

    /// The workspace's team record (`client.userBoot.team`), if hydrated.
    pub fn team(&self, workspace: &TeamId) -> Result<Option<Team>> {
        let json = self
            .conn
            .query_row(
                "SELECT json FROM team WHERE workspace_id = ?1",
                params![workspace.as_str()],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(json.map(|json| serde_json::from_str(&json)).transpose()?)
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
                    r.get::<_, String>(2)?,
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
        let f = &query.filters;
        let mut sql = String::from(
            "SELECT m.channel_id, m.json, m.deleted FROM message_current m
             WHERE m.workspace_id = ? AND m.deleted = 0",
        );
        let mut args = vec![SqlValue::Text(workspace.as_str().to_owned())];
        if let Some(fts) = query.fts_match() {
            sql.push_str(" AND m.id IN (SELECT rowid FROM message_fts WHERE message_fts MATCH ?)");
            args.push(SqlValue::Text(fts));
        }
        any_of(
            &mut sql,
            &mut args,
            &f.in_channels,
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
        sql.push_str(" ORDER BY m.ts_key DESC LIMIT ?");
        args.push(SqlValue::Integer(limit as i64));

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), raw_row)?;
        collect(rows)
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

fn insert_revision(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    channel: &ChannelId,
    message: &Message,
    edit_ts: &str,
    deleted: bool,
    json: &str,
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
    json: &str,
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

type RawRow = (String, String, bool);

fn raw_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawRow> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
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
    fn ts_key_is_numeric() {
        assert_eq!(
            ts_key(&Ts::from("1700000000.000100")),
            1_700_000_000_000_100
        );
        assert_eq!(ts_key(&Ts::from("1700000000.1")), 1_700_000_000_100_000);
        assert_eq!(ts_key(&Ts::from("1700000000")), 1_700_000_000_000_000);
    }
}
