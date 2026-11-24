//! Read-only access to the local message archive.
//!
//! `slackless_store::Store` writes and reads messages but keeps its connection private (there
//! is no conversation/user listing API), so the listing and name-resolution commands query the
//! documented SQLite schema directly here. Everything else — history, search, the send echo —
//! goes through `Store`'s public API. Listing is best-effort: a missing or unreadable archive
//! yields empty results, never an error.

use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags, params};
use slackless_core::store::types::{ChannelId, Conversation, TeamId, UserId};

/// Conversation `kind` column values, as the store writes them.
pub const ALL_KINDS: &[&str] = &["public_channel", "private_channel", "im", "mpim"];

/// One conversation as stored, with the fields the CLI renders.
#[derive(Debug, Clone)]
pub struct ConversationRow {
    pub id: ChannelId,
    pub name: Option<String>,
    pub kind: String,
    pub is_archived: bool,
    pub is_member: bool,
    /// Peer user for DMs.
    pub user: Option<UserId>,
    pub topic: Option<String>,
    /// Latest known message time (unix seconds), when the snapshot has one.
    pub latest_secs: Option<i64>,
}

/// One workspace user row from the local archive.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UserRow {
    pub id: String,
    pub name: String,
    pub display_name: Option<String>,
    pub real_name: Option<String>,
    pub is_bot: bool,
    pub deleted: bool,
}

/// One conversation's activity summary in an offline `digest`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DigestChannelRow {
    pub channel: String,
    pub label: String,
    pub message_count: u64,
    pub author_count: u64,
    pub latest_ts: String,
    pub latest_author: Option<String>,
    pub latest_text: String,
}

impl ConversationRow {
    /// `#name`-style label, falling back to the id.
    pub fn label(&self) -> String {
        self.name
            .as_ref()
            .map_or_else(|| self.id.to_string(), |name| format!("#{name}"))
    }
}

/// The platform application-data directory holding one `<team_id>.sqlite` per workspace — the
/// same location the desktop app uses (`bridge.rs`'s `default_store_path`; the store location
/// stays a front-end decision).
fn archive_dir() -> Option<PathBuf> {
    let mut path = PathBuf::from(std::env::var_os("HOME")?);
    if cfg!(target_os = "macos") {
        path.push("Library/Application Support/slackless");
    } else {
        path.push(".local/share/slackless");
    }
    Some(path)
}

/// The archive database path for one workspace.
pub fn store_path(team_id: &TeamId) -> Option<PathBuf> {
    archive_dir().map(|dir| dir.join(format!("{}.sqlite", team_id.as_str())))
}

/// Team ids with an archive on disk, sorted.
pub fn stored_team_ids() -> Vec<TeamId> {
    let Some(dir) = archive_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut ids: Vec<TeamId> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.to_string_lossy() == "sqlite")
        })
        .filter_map(|path| path.file_stem()?.to_str().map(str::to_owned))
        .map(TeamId::from)
        .collect();
    ids.sort();
    ids
}

/// A read-only view of one workspace's archive; `None` when it does not exist on disk.
pub struct Archive {
    conn: Connection,
}

impl Archive {
    pub fn open(team_id: &TeamId) -> Option<Self> {
        let path = store_path(team_id)?;
        if !path.is_file() {
            return None;
        }
        // A WAL database without its shm file cannot be opened read-only; fall back.
        let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .or_else(|_| Connection::open(&path))
            .ok()?;
        Some(Self { conn })
    }

    /// Public and private channels, alphabetical by name.
    pub fn channels(&self, workspace: &TeamId) -> Vec<ConversationRow> {
        let mut rows = self.conversations(workspace, &["public_channel", "private_channel"]);
        rows.sort_by_key(name_key);
        rows
    }

    /// Direct messages and multi-person DMs, most recently active first.
    pub fn dms(&self, workspace: &TeamId) -> Vec<ConversationRow> {
        let mut rows = self.conversations(workspace, &["im", "mpim"]);
        rows.sort_by_key(|row| std::cmp::Reverse(row.latest_secs));
        rows
    }

    /// Channel-id to `#name` map for human result lines.
    pub fn channel_names(&self, workspace: &TeamId) -> std::collections::HashMap<String, String> {
        self.conversations(workspace, ALL_KINDS)
            .into_iter()
            .filter_map(|row| {
                row.name
                    .clone()
                    .map(|name| (row.id.as_str().to_owned(), name))
            })
            .collect()
    }

    /// Resolves `#name`, `name`, `@handle`, or an exact `C…`/`D…`/`G…` id to a conversation.
    pub fn resolve_channel(&self, workspace: &TeamId, arg: &str) -> Option<ConversationRow> {
        let bare = arg
            .strip_prefix('#')
            .or_else(|| arg.strip_prefix('@'))
            .unwrap_or(arg);
        let rows = self.conversations(workspace, ALL_KINDS);
        let by_id = |row: &ConversationRow| row.id.as_str() == bare;
        let by_name = |row: &ConversationRow| {
            row.name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(bare))
        };
        rows.iter().find(|row| by_id(row)).cloned().or_else(|| {
            rows.iter().find(|row| by_name(row)).cloned().or_else(|| {
                let user = self.resolve_user(workspace, bare)?;
                rows.iter()
                    .find(|row| row.kind == "im" && row.user.as_ref().is_some_and(|u| u == &user))
                    .cloned()
            })
        })
    }

    /// The conversation with this exact id, if the archive knows it.
    pub fn conversation_by_id(
        &self,
        workspace: &TeamId,
        id: &ChannelId,
    ) -> Option<ConversationRow> {
        self.conversations(workspace, ALL_KINDS)
            .into_iter()
            .find(|row| &row.id == id)
    }

    /// The best display name for a user id, from the archive's user table.
    pub fn user_display(&self, workspace: &TeamId, user: &UserId) -> Option<String> {
        self.conn
            .query_row(
                r#"SELECT display_name, real_name, name FROM "user"
                   WHERE workspace_id = ?1 AND id = ?2"#,
                params![workspace.as_str(), user.as_str()],
                |row| {
                    let display: Option<String> = row.get(0)?;
                    let real: Option<String> = row.get(1)?;
                    let name: Option<String> = row.get(2)?;
                    Ok(display
                        .filter(|value| !value.is_empty())
                        .or(real.filter(|value| !value.is_empty()))
                        .or(name.filter(|value| !value.is_empty())))
                },
            )
            .ok()
            .flatten()
    }

    /// Number of stored current-state messages; 0 when the archive is empty or unreadable.
    pub fn message_count(&self, workspace: &TeamId) -> u64 {
        self.conn
            .query_row(
                "SELECT count(*) FROM message_current WHERE workspace_id = ?1",
                params![workspace.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .map_or(0, |n| u64::try_from(n.max(0)).unwrap_or(0))
    }

    /// Per-channel notification overrides (e.g. muted channels) parsed from cached Slack `prefs`.
    pub fn notify_levels(
        &self,
        workspace: &TeamId,
    ) -> std::collections::HashMap<ChannelId, slackless_core::NotifyLevel> {
        let Ok(json) = self.conn.query_row(
            "SELECT json FROM prefs WHERE workspace_id = ?1",
            params![workspace.as_str()],
            |row| row.get::<_, String>(0),
        ) else {
            return std::collections::HashMap::new();
        };
        serde_json::from_str::<serde_json::Value>(&json)
            .map(|prefs| slackless_core::notify_levels(&prefs))
            .unwrap_or_default()
    }

    /// Every stored conversation of the given kinds. An unreadable archive or row is skipped:
    /// the tables exist from the first migration, so absence means "not synced yet".
    fn conversations(&self, workspace: &TeamId, kinds: &[&str]) -> Vec<ConversationRow> {
        let Ok(mut stmt) = self.conn.prepare(
            "SELECT id, name, kind, is_archived, is_member, json FROM conversation
             WHERE workspace_id = ?1",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(params![workspace.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, String>(5)?,
            ))
        }) else {
            return Vec::new();
        };
        rows.flatten()
            .filter(|(_, _, kind, _, _, _)| kinds.contains(&kind.as_str()))
            .map(|(id, name, kind, is_archived, is_member, json)| {
                let conv: Option<Conversation> = serde_json::from_str(&json).ok();
                ConversationRow {
                    id: ChannelId::from(id),
                    name,
                    kind,
                    is_archived,
                    is_member,
                    user: conv.as_ref().and_then(|conv| conv.user.clone()),
                    topic: conv
                        .as_ref()
                        .and_then(|conv| conv.topic.as_ref().map(|topic| topic.value.clone())),
                    latest_secs: conv.as_ref().and_then(|conv| {
                        conv.latest.as_ref().and_then(|latest| latest.ts().secs())
                    }),
                }
            })
            .collect()
    }

    /// A user id matched by name, display name or real name (case-insensitive).
    fn resolve_user(&self, workspace: &TeamId, handle: &str) -> Option<UserId> {
        self.conn
            .query_row(
                r#"SELECT id FROM "user" WHERE workspace_id = ?1
                   AND (name = ?2 COLLATE NOCASE OR display_name = ?2 COLLATE NOCASE
                        OR real_name = ?2 COLLATE NOCASE)"#,
                params![workspace.as_str(), handle],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .map(UserId::from)
    }

    /// List archived workspace users, optionally filtered by a case-insensitive substring on
    /// `id`, `name`, `display_name`, or `real_name`.
    pub fn users(
        &self,
        workspace: &TeamId,
        query: Option<&str>,
        include_deleted: bool,
    ) -> Vec<UserRow> {
        let Ok(mut stmt) = self.conn.prepare(
            r#"SELECT id, name, display_name, real_name, is_bot, deleted
               FROM "user"
               WHERE workspace_id = ?1
               ORDER BY COALESCE(NULLIF(display_name, ''), NULLIF(real_name, ''), name) COLLATE NOCASE ASC"#,
        ) else {
            return Vec::new();
        };
        let needle = query
            .map(|q| q.trim().trim_start_matches('@').to_ascii_lowercase())
            .filter(|q| !q.is_empty());
        let Ok(rows) = stmt.query_map(params![workspace.as_str()], |row| {
            Ok(UserRow {
                id: row.get(0)?,
                name: row.get(1)?,
                display_name: row.get::<_, Option<String>>(2)?.filter(|s| !s.is_empty()),
                real_name: row.get::<_, Option<String>>(3)?.filter(|s| !s.is_empty()),
                is_bot: row.get::<_, i64>(4)? != 0,
                deleted: row.get::<_, i64>(5)? != 0,
            })
        }) else {
            return Vec::new();
        };
        rows.flatten()
            .filter(|u| include_deleted || !u.deleted)
            .filter(|u| match &needle {
                None => true,
                Some(q) => {
                    u.id.to_ascii_lowercase().contains(q)
                        || u.name.to_ascii_lowercase().contains(q)
                        || u.display_name
                            .as_deref()
                            .is_some_and(|s| s.to_ascii_lowercase().contains(q))
                        || u.real_name
                            .as_deref()
                            .is_some_and(|s| s.to_ascii_lowercase().contains(q))
                }
            })
            .collect()
    }

    /// Newest stored non-deleted message `ts` in `channel`, if any.
    pub fn latest_ts(&self, workspace: &TeamId, channel: &ChannelId) -> Option<String> {
        self.conn
            .query_row(
                "SELECT ts FROM message_current
                 WHERE workspace_id = ?1 AND channel_id = ?2 AND deleted = 0
                 ORDER BY ts_key DESC LIMIT 1",
                params![workspace.as_str(), channel.as_str()],
                |row| row.get::<_, String>(0),
            )
            .ok()
    }

    /// Activity digest grouped by channel for messages at or after `cutoff_secs` (unix seconds).
    pub fn digest_since(
        &self,
        workspace: &TeamId,
        cutoff_secs: i64,
        limit: usize,
    ) -> Vec<DigestChannelRow> {
        let cutoff_key = cutoff_secs.saturating_mul(1_000_000);
        let Ok(mut stmt) = self.conn.prepare(
            "SELECT channel_id, COUNT(*), COUNT(DISTINCT COALESCE(user_id, '')), MAX(ts_key)
             FROM message_current
             WHERE workspace_id = ?1 AND ts_key >= ?2 AND deleted = 0
             GROUP BY channel_id
             ORDER BY COUNT(*) DESC, MAX(ts_key) DESC
             LIMIT ?3",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(
            params![workspace.as_str(), cutoff_key, limit as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        ) else {
            return Vec::new();
        };
        let names = self.channel_names(workspace);
        let dm_labels: std::collections::HashMap<String, String> = self
            .dms(workspace)
            .into_iter()
            .filter_map(|dm| {
                let label = dm
                    .user
                    .as_ref()
                    .and_then(|u| self.user_display(workspace, u))
                    .map(|n| format!("@{n}"))
                    .or_else(|| dm.name.map(|n| format!("#{n}")))?;
                Some((dm.id.as_str().to_owned(), label))
            })
            .collect();
        let mut out = Vec::new();
        for (channel_id, msg_count, author_count, max_ts_key) in rows.flatten() {
            let latest = self
                .conn
                .query_row(
                    "SELECT ts, user_id, text FROM message_current
                     WHERE workspace_id = ?1 AND channel_id = ?2 AND ts_key = ?3
                     LIMIT 1",
                    params![workspace.as_str(), &channel_id, max_ts_key],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .ok();
            let (latest_ts, latest_user, latest_text) = latest.unwrap_or_default();
            let latest_author = latest_user.as_deref().map(|uid| {
                self.user_display(workspace, &UserId::from(uid))
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
            out.push(DigestChannelRow {
                channel: channel_id,
                label,
                message_count: u64::try_from(msg_count.max(0)).unwrap_or(0),
                author_count: u64::try_from(author_count.max(0)).unwrap_or(0),
                latest_ts,
                latest_author,
                latest_text: truncated,
            });
        }
        out
    }
}

/// Case-insensitive name sort key; unnamed conversations sort last by id.
fn name_key(row: &ConversationRow) -> String {
    let unnamed = if row.name.is_none() { "1" } else { "0" };
    format!(
        "{unnamed}{}",
        row.name.as_deref().unwrap_or("").to_lowercase()
    )
}
