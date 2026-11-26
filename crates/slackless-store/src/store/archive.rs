//! Read-only archive directory, lookup, and offline digest queries.

use super::*;

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
        let mut out = Vec::new();
        query_in_chunks(
            &self.conn,
            workspace,
            kinds,
            |k| k,
            "SELECT id, name, kind, is_archived, is_member, json
             FROM conversation
             WHERE workspace_id = ?1 AND kind",
            parse_conversation_summary,
            |summary| out.push(summary),
        )?;
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
                Ok(pick_user_display(row.get(0)?, row.get(1)?, row.get(2)?))
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

    /// Distinct workspace IDs present anywhere in this database (`team`, `conversation`,
    /// `message_current`, `file_transcript`, `huddle_transcript`), sorted.
    pub fn list_workspaces(&self) -> Result<Vec<TeamId>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT workspace_id FROM team
             UNION SELECT workspace_id FROM conversation
             UNION SELECT workspace_id FROM message_current
             UNION SELECT workspace_id FROM file_transcript
             UNION SELECT workspace_id FROM huddle_transcript
             ORDER BY workspace_id ASC",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            let id = row?;
            if !id.is_empty() {
                out.push(TeamId::from(id));
            }
        }
        Ok(out)
    }

    /// Visit every non-deleted current message in `workspace` (oldest first) for incremental
    /// sidecar indexing, with its attached file names & transcripts. Rows are decoded one at a
    /// time, so a whole workspace is never held in memory at once.
    pub fn for_each_message_for_indexing(
        &self,
        workspace: &TeamId,
        mut visit: impl FnMut(ChannelId, Message, String, String),
    ) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT channel_id, json, file_names, file_transcripts
             FROM message_current
             WHERE workspace_id = ?1 AND deleted = 0
             ORDER BY ts_key ASC",
        )?;
        let mut rows = stmt.query(params![workspace.as_str()])?;
        while let Some(row) = rows.next()? {
            let channel = ChannelId::from(row.get::<_, String>(0)?);
            let json = decode_json_value(row.get_ref(1)?)?;
            let Ok(mut message) = serde_json::from_str::<Message>(&json) else {
                continue;
            };
            message.channel.get_or_insert_with(|| channel.clone());
            visit(channel, message, row.get(2)?, row.get(3)?);
        }
        Ok(())
    }

    /// Every stored `file_transcript` row in `workspace` (with optional carrying message channel
    /// & ts from `message_file`), for incremental semantic indexing.
    pub fn list_file_transcripts_for_indexing(
        &self,
        workspace: &TeamId,
    ) -> Result<Vec<WorkspaceFileTranscript>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT ft.file_id, ft.source, ft.text, ft.updated_at,
                    (SELECT mc.channel_id
                     FROM message_file mf
                     JOIN message_current mc ON mc.id = mf.message_id
                     WHERE mf.workspace_id = ft.workspace_id AND mf.file_id = ft.file_id
                     ORDER BY mc.ts_key DESC LIMIT 1),
                    (SELECT mc.ts
                     FROM message_file mf
                     JOIN message_current mc ON mc.id = mf.message_id
                     WHERE mf.workspace_id = ft.workspace_id AND mf.file_id = ft.file_id
                     ORDER BY mc.ts_key DESC LIMIT 1)
             FROM file_transcript ft
             WHERE ft.workspace_id = ?1
             ORDER BY ft.updated_at DESC, ft.file_id ASC",
        )?;
        let rows = stmt.query_map(params![workspace.as_str()], |row| {
            let file_id: String = row.get(0)?;
            let source: String = row.get(1)?;
            let text: String = row.get(2)?;
            let updated_at: i64 = row.get(3)?;
            let channel: Option<String> = row.get(4)?;
            let message_ts: Option<String> = row.get(5)?;
            Ok(WorkspaceFileTranscript {
                workspace: workspace.clone(),
                transcript: StoredFileTranscript {
                    file_id: crate::types::FileId::from(file_id),
                    source,
                    text,
                    cues: Vec::new(),
                    updated_at,
                },
                channel: channel.map(ChannelId::from),
                message_ts: message_ts.map(Ts::from),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Every stored `huddle_transcript` segment in `workspace`, oldest first, for semantic indexing.
    pub fn list_huddle_transcripts_for_indexing(
        &self,
        workspace: &TeamId,
    ) -> Result<Vec<HuddleTranscriptSegment>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, channel_id, call_id, thread_root_ts, speaker_id, lang, source, text, at_ms
             FROM huddle_transcript
             WHERE workspace_id = ?1
             ORDER BY at_ms ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![workspace.as_str()], |row| {
            let channel_s: String = row.get(1)?;
            let thread_s: Option<String> = row.get(3)?;
            let speaker_s: Option<String> = row.get(4)?;
            Ok(HuddleTranscriptSegment {
                id: row.get(0)?,
                channel: ChannelId::from(channel_s),
                call_id: row.get(2)?,
                thread_root_ts: thread_s.filter(|s| !s.is_empty()).map(Ts::from),
                speaker_id: speaker_s.filter(|s| !s.is_empty()).map(UserId::from),
                lang: row.get(5)?,
                source: row.get(6)?,
                text: row.get(7)?,
                at_ms: row.get(8)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Fetch a single `huddle_transcript` segment by `(workspace, id)` (for semantic hydration).
    pub fn huddle_transcript_segment(
        &self,
        workspace: &TeamId,
        segment_id: i64,
    ) -> Result<Option<HuddleTranscriptSegment>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, channel_id, call_id, thread_root_ts, speaker_id, lang, source, text, at_ms
             FROM huddle_transcript
             WHERE workspace_id = ?1 AND id = ?2",
        )?;
        Ok(stmt
            .query_row(params![workspace.as_str(), segment_id], |row| {
                let channel_s: String = row.get(1)?;
                let thread_s: Option<String> = row.get(3)?;
                let speaker_s: Option<String> = row.get(4)?;
                Ok(HuddleTranscriptSegment {
                    id: row.get(0)?,
                    channel: ChannelId::from(channel_s),
                    call_id: row.get(2)?,
                    thread_root_ts: thread_s.filter(|s| !s.is_empty()).map(Ts::from),
                    speaker_id: speaker_s.filter(|s| !s.is_empty()).map(UserId::from),
                    lang: row.get(5)?,
                    source: row.get(6)?,
                    text: row.get(7)?,
                    at_ms: row.get(8)?,
                })
            })
            .optional()?)
    }

    /// Per-channel stored message counts for `workspace` (including tombstones), in one query.
    pub fn channel_message_counts(&self, workspace: &TeamId) -> Result<HashMap<ChannelId, u64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT channel_id, COUNT(*) FROM message_current
             WHERE workspace_id = ?1
             GROUP BY channel_id",
        )?;
        let rows = stmt.query_map(params![workspace.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (ch, count) = row?;
            out.insert(
                ChannelId::from(ch),
                u64::try_from(count.max(0)).unwrap_or(0),
            );
        }
        Ok(out)
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

    /// Up to `limit` newest undeleted messages of `subtype` across all of `workspace`'s
    /// conversations, newest first.
    pub fn recent_messages_by_subtype(
        &self,
        workspace: &TeamId,
        subtype: &MessageSubtype,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT channel_id, json, deleted FROM message_current
             WHERE workspace_id = ?1 AND subtype = ?2 AND deleted = 0
             ORDER BY ts_key DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![
                workspace.as_str(),
                subtype.as_str(),
                i64::try_from(limit).unwrap_or(i64::MAX),
            ],
            raw_row,
        )?;
        collect(rows)
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
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        self.with_read_tx(|tx| {
            let mut stmt = tx.prepare_cached(
                "SELECT channel_id, COUNT(*), COUNT(DISTINCT COALESCE(user_id, '')), MAX(ts_key)
                 FROM message_current
                 WHERE workspace_id = ?1 AND ts_key >= ?2 AND deleted = 0
                 GROUP BY channel_id
                 ORDER BY COUNT(*) DESC, MAX(ts_key) DESC
                 LIMIT ?3",
            )?;
            let grouped: Vec<(String, i64, i64, i64)> = stmt
                .query_map(params![workspace.as_str(), cutoff_key, limit_i64], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            if grouped.is_empty() {
                return Ok(Vec::new());
            }

            // Fetch metadata ONLY for the conversations actually present in `grouped`
            // (eliminating full-workspace channel/DM scans).
            let channel_ids: Vec<&str> = grouped.iter().map(|(id, ..)| id.as_str()).collect();
            let mut convs: HashMap<String, ConversationSummary> =
                HashMap::with_capacity(channel_ids.len());
            query_in_chunks(
                tx,
                workspace,
                &channel_ids,
                |id| id,
                "SELECT id, name, kind, is_archived, is_member, json
                 FROM conversation
                 WHERE workspace_id = ?1 AND id",
                parse_conversation_summary,
                |summary| {
                    convs.insert(summary.id.as_str().to_owned(), summary);
                },
            )?;

            let mut latest_stmt = tx.prepare_cached(
                "SELECT ts, user_id, text FROM message_current
                 WHERE workspace_id = ?1 AND channel_id = ?2 AND ts_key = ?3 AND deleted = 0
                 LIMIT 1",
            )?;
            let mut latest_rows: Vec<(String, Option<String>, String)> =
                Vec::with_capacity(grouped.len());
            let mut user_ids_seen: HashSet<String> = HashSet::new();
            for (channel_id, _, _, max_ts_key) in &grouped {
                let latest = latest_stmt
                    .query_row(params![workspace.as_str(), channel_id, max_ts_key], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })
                    .optional()?
                    .unwrap_or_default();
                if let Some(uid) = &latest.1 {
                    user_ids_seen.insert(uid.clone());
                }
                if let Some(conv) = convs.get(channel_id)
                    && (conv.kind == "im" || conv.kind == "mpim")
                    && let Some(peer) = &conv.user
                {
                    user_ids_seen.insert(peer.as_str().to_owned());
                }
                latest_rows.push(latest);
            }

            // Batch-fetch display names for all referenced DM peers and latest authors at once.
            let mut user_displays: HashMap<String, String> =
                HashMap::with_capacity(user_ids_seen.len());
            if !user_ids_seen.is_empty() {
                let ask: Vec<String> = user_ids_seen.into_iter().collect();
                query_in_chunks(
                    tx,
                    workspace,
                    &ask,
                    String::as_str,
                    r#"SELECT id, display_name, real_name, name FROM "user"
                       WHERE workspace_id = ?1 AND id"#,
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            pick_user_display(row.get(1)?, row.get(2)?, row.get(3)?),
                        ))
                    },
                    |(id, display)| {
                        if let Some(name) = display {
                            user_displays.insert(id, name);
                        }
                    },
                )?;
            }

            let mut out = Vec::with_capacity(grouped.len());
            for ((channel_id, msg_count, author_count, _), (latest_ts, latest_user, latest_text)) in
                grouped.into_iter().zip(latest_rows)
            {
                let latest_author = latest_user.as_deref().map(|uid| {
                    user_displays
                        .get(uid)
                        .cloned()
                        .unwrap_or_else(|| uid.to_owned())
                });
                let label = match convs.get(&channel_id) {
                    Some(conv) if conv.name.is_some() => {
                        format!("#{}", conv.name.as_deref().unwrap_or_default())
                    }
                    Some(conv) if conv.kind == "im" || conv.kind == "mpim" => conv
                        .user
                        .as_ref()
                        .and_then(|u| user_displays.get(u.as_str()))
                        .map(|n| format!("@{n}"))
                        .unwrap_or_else(|| channel_id.clone()),
                    _ => channel_id.clone(),
                };
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
        })
    }
}

fn pick_user_display(
    display: Option<String>,
    real: Option<String>,
    name: Option<String>,
) -> Option<String> {
    display
        .filter(|v| !v.is_empty())
        .or_else(|| real.filter(|v| !v.is_empty()))
        .or_else(|| name.filter(|v| !v.is_empty()))
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
