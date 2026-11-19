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
}

/// Case-insensitive name sort key; unnamed conversations sort last by id.
fn name_key(row: &ConversationRow) -> String {
    let unnamed = if row.name.is_none() { "1" } else { "0" };
    format!(
        "{unnamed}{}",
        row.name.as_deref().unwrap_or("").to_lowercase()
    )
}
