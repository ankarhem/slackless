//! Read-only archive directory, lookup, and offline digest queries.

use super::*;

/// Conversation `kind` column values, as the store writes them.
pub const ALL_CONVERSATION_KINDS: &[&str] = &["public_channel", "private_channel", "im", "mpim"];

/// Summary metadata for one stored conversation (channel, private group, DM, or group DM).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSummary {
    pub id: ChannelId,
    pub name: Option<String>,
    pub kind: String,
    pub is_archived: bool,
    pub is_member: bool,
    /// Peer user for direct messages (`im`).
    pub user: Option<UserId>,
    pub topic: Option<String>,
    /// Latest known message time (unix seconds), when present in the stored snapshot.
    pub latest_secs: Option<i64>,
}

impl ConversationSummary {
    /// `#name`-style label, falling back to the channel id.
    pub fn label(&self) -> String {
        self.name
            .as_ref()
            .map_or_else(|| self.id.to_string(), |name| format!("#{name}"))
    }
}

/// Lightweight workspace user record for directory listings and lookups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserSummary {
    pub id: UserId,
    pub name: String,
    pub display_name: Option<String>,
    pub real_name: Option<String>,
    pub is_bot: bool,
    pub deleted: bool,
}

/// One conversation's activity summary in a time-windowed offline digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestChannel {
    pub channel: ChannelId,
    pub label: String,
    pub message_count: u64,
    pub author_count: u64,
    pub latest_ts: Ts,
    pub latest_author: Option<String>,
    pub latest_text: String,
}

impl Store {
    /// Public and private channels for `workspace`, sorted case-insensitively by name
    /// with unnamed channels last.
    pub fn list_channels(&self, workspace: &TeamId) -> Result<Vec<ConversationSummary>> {
        let mut rows =
            self.list_conversations_by_kinds(workspace, &["public_channel", "private_channel"])?;
        rows.sort_by_key(channel_name_sort_key);
        Ok(rows)
    }

    /// Direct messages and multi-person DMs for `workspace`, most recently active first.
    pub fn list_dms(&self, workspace: &TeamId) -> Result<Vec<ConversationSummary>> {
        let mut rows = self.list_conversations_by_kinds(workspace, &["im", "mpim"])?;
        rows.sort_by_key(|row| std::cmp::Reverse(row.latest_secs));
        Ok(rows)
    }

    /// Every stored conversation of `workspace` whose `kind` is in `kinds`.
    pub fn list_conversations_by_kinds(
        &self,
        workspace: &TeamId,
        kinds: &[&str],
    ) -> Result<Vec<ConversationSummary>> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, kind, is_archived, is_member, json
             FROM conversation
             WHERE workspace_id = ?1",
        )?;
        let rows = stmt.query_map(params![workspace.as_str()], parse_conversation_summary)?;
        let mut out = Vec::new();
        for row in rows {
            let summary = row?;
            if kinds.contains(&summary.kind.as_str()) {
                out.push(summary);
            }
        }
        Ok(out)
    }

    /// Channel-id to channel-name map for `workspace` (unnamed conversations omitted).
    pub fn channel_names(&self, workspace: &TeamId) -> Result<HashMap<String, String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name FROM conversation
             WHERE workspace_id = ?1 AND name IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![workspace.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, name) = row?;
            out.insert(id, name);
        }
        Ok(out)
    }

    /// Summary for one conversation by exact id (single primary-key lookup).
    pub fn conversation_summary(
        &self,
        workspace: &TeamId,
        id: &ChannelId,
    ) -> Result<Option<ConversationSummary>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, kind, is_archived, is_member, json
             FROM conversation
             WHERE workspace_id = ?1 AND id = ?2",
        )?;
        Ok(stmt
            .query_row(
                params![workspace.as_str(), id.as_str()],
                parse_conversation_summary,
            )
            .optional()?)
    }

    /// Resolves `#name`, `name`, `@handle`, or an exact `C…`/`D…`/`G…` id to a conversation.
    ///
    /// Precedence:
    /// 1. Exact conversation id match.
    /// 2. Case-insensitive conversation name match.
    /// 3. Direct message (`im`) with a user matching handle, display name, or real name.
    pub fn resolve_channel(
        &self,
        workspace: &TeamId,
        arg: &str,
    ) -> Result<Option<ConversationSummary>> {
        let bare = arg
            .strip_prefix('#')
            .or_else(|| arg.strip_prefix('@'))
            .unwrap_or(arg);

        if let Some(by_id) = self.conversation_summary(workspace, &ChannelId::from(bare))? {
            return Ok(Some(by_id));
        }

        let by_name = self
            .conn
            .prepare_cached(
                "SELECT id, name, kind, is_archived, is_member, json
                 FROM conversation
                 WHERE workspace_id = ?1 AND name = ?2 COLLATE NOCASE
                 LIMIT 1",
            )?
            .query_row(
                params![workspace.as_str(), bare],
                parse_conversation_summary,
            )
            .optional()?;
        if by_name.is_some() {
            return Ok(by_name);
        }

        let Some(user_id) = self.resolve_user(workspace, bare)? else {
            return Ok(None);
        };
        let ims = self.list_conversations_by_kinds(workspace, &["im"])?;
        Ok(ims
            .into_iter()
            .find(|row| row.user.as_ref() == Some(&user_id)))
    }

    /// Look up a user id by `name`, `display_name`, or `real_name` (case-insensitive).
    pub fn resolve_user(&self, workspace: &TeamId, handle: &str) -> Result<Option<UserId>> {
        let mut stmt = self.conn.prepare_cached(
            r#"SELECT id FROM "user"
               WHERE workspace_id = ?1
                 AND (name = ?2 COLLATE NOCASE
                      OR display_name = ?2 COLLATE NOCASE
                      OR real_name = ?2 COLLATE NOCASE)
               LIMIT 1"#,
        )?;
        Ok(stmt
            .query_row(params![workspace.as_str(), handle], |row| {
                row.get::<_, String>(0)
            })
            .optional()?
            .map(UserId::from))
    }

    /// Best human display name for `user` (`display_name` -> `real_name` -> `name`, skipping empty strings).
    pub fn user_display(&self, workspace: &TeamId, user: &UserId) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare_cached(
            r#"SELECT display_name, real_name, name FROM "user"
               WHERE workspace_id = ?1 AND id = ?2"#,
        )?;
        let row = stmt
            .query_row(params![workspace.as_str(), user.as_str()], |row| {
                let display: Option<String> = row.get(0)?;
                let real: Option<String> = row.get(1)?;
                let name: Option<String> = row.get(2)?;
                Ok(display
                    .filter(|v| !v.is_empty())
                    .or_else(|| real.filter(|v| !v.is_empty()))
                    .or_else(|| name.filter(|v| !v.is_empty())))
            })
            .optional()?;
        Ok(row.flatten())
    }

    /// List archived workspace users, optionally filtered by a case-insensitive substring on
    /// `id`, `name`, `display_name`, or `real_name`.
    pub fn list_users(
        &self,
        workspace: &TeamId,
        query: Option<&str>,
        include_deleted: bool,
    ) -> Result<Vec<UserSummary>> {
        let mut stmt = self.conn.prepare_cached(
            r#"SELECT id, name, display_name, real_name, is_bot, deleted
               FROM "user"
               WHERE workspace_id = ?1
               ORDER BY COALESCE(NULLIF(display_name, ''), NULLIF(real_name, ''), name) COLLATE NOCASE ASC"#,
        )?;
        let needle = query
            .map(|q| q.trim().trim_start_matches('@').to_ascii_lowercase())
            .filter(|q| !q.is_empty());
        let rows = stmt.query_map(params![workspace.as_str()], |row| {
            Ok(UserSummary {
                id: UserId::from(row.get::<_, String>(0)?),
                name: row.get(1)?,
                display_name: row.get::<_, Option<String>>(2)?.filter(|s| !s.is_empty()),
                real_name: row.get::<_, Option<String>>(3)?.filter(|s| !s.is_empty()),
                is_bot: row.get::<_, i64>(4)? != 0,
                deleted: row.get::<_, i64>(5)? != 0,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            let user = row?;
            if !include_deleted && user.deleted {
                continue;
            }
            if let Some(q) = &needle {
                let matches = user.id.as_str().to_ascii_lowercase().contains(q)
                    || user.name.to_ascii_lowercase().contains(q)
                    || user
                        .display_name
                        .as_deref()
                        .is_some_and(|s| s.to_ascii_lowercase().contains(q))
                    || user
                        .real_name
                        .as_deref()
                        .is_some_and(|s| s.to_ascii_lowercase().contains(q));
                if !matches {
                    continue;
                }
            }
            out.push(user);
        }
        Ok(out)
    }

    /// Number of stored current-state messages for `workspace` (including tombstones).
    pub fn message_count(&self, workspace: &TeamId) -> Result<u64> {
        let count: i64 = self.conn.query_row(
            "SELECT count(*) FROM message_current WHERE workspace_id = ?1",
            params![workspace.as_str()],
            |row| row.get(0),
        )?;
        Ok(u64::try_from(count.max(0)).unwrap_or(0))
    }

    /// Newest stored non-deleted message `ts` in `channel`, if any.
    pub fn latest_ts(&self, workspace: &TeamId, channel: &ChannelId) -> Result<Option<Ts>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT ts FROM message_current
             WHERE workspace_id = ?1 AND channel_id = ?2 AND deleted = 0
             ORDER BY ts_key DESC LIMIT 1",
        )?;
        Ok(stmt
            .query_row(params![workspace.as_str(), channel.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .optional()?
            .map(Ts::from))
    }

    /// Activity digest grouped by channel for non-deleted messages at or after `cutoff_secs`
    /// (unix seconds), ordered by message count descending then latest timestamp descending.
    pub fn digest_since(
        &self,
        workspace: &TeamId,
        cutoff_secs: i64,
        limit: usize,
    ) -> Result<Vec<DigestChannel>> {
        let cutoff_key = cutoff_secs.saturating_mul(1_000_000);
        let mut stmt = self.conn.prepare_cached(
            "SELECT channel_id, COUNT(*), COUNT(DISTINCT COALESCE(user_id, '')), MAX(ts_key)
             FROM message_current
             WHERE workspace_id = ?1 AND ts_key >= ?2 AND deleted = 0
             GROUP BY channel_id
             ORDER BY COUNT(*) DESC, MAX(ts_key) DESC
             LIMIT ?3",
        )?;
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        let grouped: Vec<(String, i64, i64, i64)> = stmt
            .query_map(params![workspace.as_str(), cutoff_key, limit_i64], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        if grouped.is_empty() {
            return Ok(Vec::new());
        }

        let names = self.channel_names(workspace)?;
        let dm_labels: HashMap<String, String> = self
            .list_dms(workspace)?
            .into_iter()
            .filter_map(|dm| {
                let label = dm
                    .user
                    .as_ref()
                    .and_then(|u| self.user_display(workspace, u).ok().flatten())
                    .map(|n| format!("@{n}"))
                    .or_else(|| dm.name.map(|n| format!("#{n}")))?;
                Some((dm.id.as_str().to_owned(), label))
            })
            .collect();

        let mut latest_stmt = self.conn.prepare_cached(
            "SELECT ts, user_id, text FROM message_current
             WHERE workspace_id = ?1 AND channel_id = ?2 AND ts_key = ?3 AND deleted = 0
             LIMIT 1",
        )?;

        let mut out = Vec::with_capacity(grouped.len());
        for (channel_id, msg_count, author_count, max_ts_key) in grouped {
            let latest = latest_stmt
                .query_row(
                    params![workspace.as_str(), &channel_id, max_ts_key],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?;
            let (latest_ts, latest_user, latest_text) = latest.unwrap_or_default();
            let latest_author = latest_user.as_deref().map(|uid| {
                self.user_display(workspace, &UserId::from(uid))
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| uid.to_owned())
            });
            let label = names
                .get(&channel_id)
                .map(|n| format!("#{n}"))
                .or_else(|| dm_labels.get(&channel_id).cloned())
                .unwrap_or_else(|| channel_id.clone());
            let snippet = latest_text.split_whitespace().collect::<Vec<_>>().join(" ");
            let truncated = if snippet.chars().count() > 120 {
                let mut s: String = snippet.chars().take(119).collect();
                s.push('…');
                s
            } else {
                snippet
            };
            out.push(DigestChannel {
                channel: ChannelId::from(channel_id),
                label,
                message_count: u64::try_from(msg_count.max(0)).unwrap_or(0),
                author_count: u64::try_from(author_count.max(0)).unwrap_or(0),
                latest_ts: Ts::from(latest_ts),
                latest_author,
                latest_text: truncated,
            });
        }
        Ok(out)
    }
}

fn parse_conversation_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationSummary> {
    let id: String = row.get(0)?;
    let name: Option<String> = row.get(1)?;
    let kind: String = row.get(2)?;
    let is_archived: bool = row.get(3)?;
    let is_member: bool = row.get(4)?;
    let json: String = row.get(5)?;
    let conv: Option<Conversation> = serde_json::from_str(&json).ok();
    Ok(ConversationSummary {
        id: ChannelId::from(id),
        name,
        kind,
        is_archived,
        is_member,
        user: conv.as_ref().and_then(|c| c.user.clone()),
        topic: conv
            .as_ref()
            .and_then(|c| c.topic.as_ref().map(|t| t.value.clone())),
        latest_secs: conv
            .as_ref()
            .and_then(|c| c.latest.as_ref().and_then(|l| l.ts().secs())),
    })
}

/// Case-insensitive name sort key; unnamed conversations sort last.
fn channel_name_sort_key(row: &ConversationSummary) -> (u8, String, String) {
    let unnamed = u8::from(row.name.is_none());
    (
        unnamed,
        row.name.as_deref().unwrap_or("").to_lowercase(),
        row.id.as_str().to_owned(),
    )
}
