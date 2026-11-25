//! Read-only access to the local message archive.
//!
//! `slackless_store::Store` writes and reads messages but keeps its connection private (there
//! is no conversation/user listing API), so the listing and name-resolution commands query the
//! documented SQLite schema directly here. Everything else — history, search, the send echo —
//! goes through `Store`'s public API. Listing is best-effort: a missing or unreadable archive
//! yields empty results, never an error.

use std::path::{Path, PathBuf};

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
    stored_team_ids_in(&dir)
}

/// Team ids with a `<team_id>.sqlite` archive file in `dir`, sorted.
pub(crate) fn stored_team_ids_in(dir: &Path) -> Vec<TeamId> {
    let Ok(entries) = std::fs::read_dir(dir) else {
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
        Self::open_path(&path)
    }

    pub(crate) fn open_path(path: &Path) -> Option<Self> {
        if !path.is_file() {
            return None;
        }
        // A WAL database without its shm file cannot be opened read-only; fall back.
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .or_else(|_| Connection::open(path))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    use slackless_core::NotifyLevel;
    use slackless_core::store::Store;
    use slackless_core::types::{Latest, Message, Profile, TopicPurpose, Ts, User};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "slackless-cli-archive-test-{}-{}-{n}",
                label,
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn make_user(
        id: &str,
        name: &str,
        display_name: Option<&str>,
        real_name: Option<&str>,
        is_bot: bool,
        deleted: bool,
    ) -> User {
        User {
            id: UserId::from(id),
            name: name.to_owned(),
            real_name: real_name.map(str::to_owned),
            profile: Profile {
                display_name: display_name.map(str::to_owned),
                real_name: real_name.map(str::to_owned),
                ..Profile::default()
            },
            is_bot,
            deleted,
            ..User::default()
        }
    }

    fn make_message(ts: &str, user: Option<&str>, text: &str) -> Message {
        Message {
            ts: Ts::from(ts),
            user: user.map(UserId::from),
            text: text.to_owned(),
            ..Message::default()
        }
    }

    fn populated_archive(dir: &TempDir) -> (PathBuf, TeamId) {
        let ws = TeamId::from("T1");
        let db_path = dir.path().join("T1.sqlite");
        let mut store = Store::open(&db_path).expect("open store");

        // Users exercising display_name -> real_name -> name fallbacks, empty strings, bot, deleted.
        let users = vec![
            make_user(
                "U1",
                "alice",
                Some("Alice Display"),
                Some("Alice Real"),
                false,
                false,
            ),
            make_user("U2", "bob", Some(""), Some("Bob Real"), false, false),
            make_user("U3", "carol", Some(""), Some(""), false, false),
            make_user("U4", "zbot", Some("Zeta Bot"), None, true, false),
            make_user("U5", "olduser", Some("Former Member"), None, false, true),
        ];
        store.upsert_users(&ws, &users).expect("upsert users");

        // Conversations across all four kinds + unnamed + archived/non-member.
        let convs = vec![
            Conversation {
                id: ChannelId::from("C_ENG"),
                name: Some("Engineering".into()),
                is_channel: true,
                is_member: true,
                topic: Some(TopicPurpose {
                    value: "Build things".into(),
                    ..TopicPurpose::default()
                }),
                ..Conversation::default()
            },
            Conversation {
                id: ChannelId::from("C_ALPHA"),
                name: Some("alpha".into()),
                is_channel: true,
                is_private: true,
                is_archived: true,
                is_member: false,
                ..Conversation::default()
            },
            Conversation {
                id: ChannelId::from("C_UNNAMED"),
                name: None,
                is_channel: true,
                is_member: true,
                ..Conversation::default()
            },
            // Public channel whose name matches a user handle to test resolve_channel precedence.
            Conversation {
                id: ChannelId::from("C_BOB"),
                name: Some("bob".into()),
                is_channel: true,
                is_member: true,
                ..Conversation::default()
            },
            Conversation {
                id: ChannelId::from("D_ALICE"),
                is_im: true,
                user: Some(UserId::from("U1")),
                latest: Some(Latest::Ts(Ts::from("1700000300.000000"))),
                ..Conversation::default()
            },
            Conversation {
                id: ChannelId::from("D_BOB"),
                is_im: true,
                user: Some(UserId::from("U2")),
                latest: None,
                ..Conversation::default()
            },
            Conversation {
                id: ChannelId::from("G_MPIM"),
                name: Some("mpdm-alice--bob-1".into()),
                is_mpim: true,
                latest: Some(Latest::Ts(Ts::from("1700000500.000000"))),
                ..Conversation::default()
            },
        ];
        store
            .upsert_conversations(&ws, &convs)
            .expect("upsert convs");

        // Messages in C_ENG (including old message, deleted message, and multi-author activity).
        store
            .upsert_messages(
                &ws,
                &ChannelId::from("C_ENG"),
                &[
                    make_message("1700000050.000000", Some("U1"), "before cutoff"),
                    make_message("1700000100.000000", Some("U1"), "at cutoff"),
                    make_message("1700000200.000000", Some("U2"), "second author"),
                    make_message("1700000250.000000", Some("U1"), "newest will be deleted"),
                ],
            )
            .expect("upsert C_ENG");
        store
            .record_deletion(
                &ws,
                &ChannelId::from("C_ENG"),
                &Ts::from("1700000250.000000"),
                Some(&Ts::from("1700000251.000000")),
                None,
            )
            .expect("record deletion");

        // Messages in D_ALICE with same count (2) as C_ENG post-cutoff, but newer max ts (tie-breaker).
        let long_utf8 = format!("hello\n\tworld   {}", "é".repeat(130));
        store
            .upsert_messages(
                &ws,
                &ChannelId::from("D_ALICE"),
                &[
                    make_message("1700000150.000000", Some("U1"), "dm one"),
                    make_message("1700000300.000000", Some("U1"), &long_utf8),
                ],
            )
            .expect("upsert D_ALICE");

        // Message in an unknown channel (not in `conversation` table) -> raw ID fallback.
        store
            .upsert_message(
                &ws,
                &ChannelId::from("C_ORPHAN"),
                &make_message("1700000120.000000", None, "orphan   msg\nline2"),
            )
            .expect("upsert C_ORPHAN");

        // Notification prefs: C_ENG muted, C_ALPHA mentions, C_BOB everything (omitted).
        let prefs = serde_json::json!({
            "all_notifications_prefs": serde_json::json!({
                "channels": {
                    "C_ENG": { "muted": true, "desktop": "everything" },
                    "C_ALPHA": { "muted": false, "desktop": "mention" },
                    "C_BOB": { "muted": false, "desktop": "everything" }
                }
            }).to_string()
        });
        store.replace_prefs(&ws, &prefs).expect("replace prefs");

        // Another workspace row to verify workspace isolation.
        let other_ws = TeamId::from("T2");
        store
            .upsert_message(
                &other_ws,
                &ChannelId::from("C_OTHER"),
                &make_message("1700000999.000000", Some("U1"), "other ws"),
            )
            .expect("upsert T2");

        (db_path, ws)
    }

    #[test]
    fn open_path_returns_none_when_missing() {
        let dir = TempDir::new("missing");
        assert!(Archive::open_path(&dir.path().join("nope.sqlite")).is_none());
    }

    #[test]
    fn channels_filters_and_sorts_case_insensitively_with_unnamed_last() {
        let dir = TempDir::new("channels");
        let (db_path, ws) = populated_archive(&dir);
        let archive = Archive::open_path(&db_path).expect("open archive");

        let channels = archive.channels(&ws);
        let ids: Vec<&str> = channels.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["C_ALPHA", "C_BOB", "C_ENG", "C_UNNAMED"]);

        assert_eq!(channels[0].label(), "#alpha");
        assert!(channels[0].is_archived);
        assert!(!channels[0].is_member);
        assert_eq!(channels[2].topic.as_deref(), Some("Build things"));
        assert_eq!(channels[3].label(), "C_UNNAMED");
    }

    #[test]
    fn dms_filters_and_sorts_by_latest_secs_desc_with_none_last() {
        let dir = TempDir::new("dms");
        let (db_path, ws) = populated_archive(&dir);
        let archive = Archive::open_path(&db_path).expect("open archive");

        let dms = archive.dms(&ws);
        let ids: Vec<&str> = dms.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["G_MPIM", "D_ALICE", "D_BOB"]);
        assert_eq!(dms[0].latest_secs, Some(1_700_000_500));
        assert_eq!(dms[1].latest_secs, Some(1_700_000_300));
        assert_eq!(dms[2].latest_secs, None);
    }

    #[test]
    fn resolve_channel_respects_precedence_and_prefix_stripping() {
        let dir = TempDir::new("resolve");
        let (db_path, ws) = populated_archive(&dir);
        let archive = Archive::open_path(&db_path).expect("open archive");

        // Exact ID match.
        assert_eq!(
            archive.resolve_channel(&ws, "C_ENG").map(|r| r.id),
            Some(ChannelId::from("C_ENG"))
        );
        // Case-insensitive `#name` and bare `name`.
        assert_eq!(
            archive.resolve_channel(&ws, "#engineering").map(|r| r.id),
            Some(ChannelId::from("C_ENG"))
        );
        assert_eq!(
            archive.resolve_channel(&ws, "ENGINEERING").map(|r| r.id),
            Some(ChannelId::from("C_ENG"))
        );
        // Channel name takes precedence over IM by user handle ("bob" is both #bob and @bob).
        assert_eq!(
            archive.resolve_channel(&ws, "bob").map(|r| r.id),
            Some(ChannelId::from("C_BOB"))
        );
        // `@handle` and display/real name resolve to IM when no channel shares the name.
        assert_eq!(
            archive.resolve_channel(&ws, "@alice").map(|r| r.id),
            Some(ChannelId::from("D_ALICE"))
        );
        assert_eq!(
            archive.resolve_channel(&ws, "Alice Display").map(|r| r.id),
            Some(ChannelId::from("D_ALICE"))
        );
        assert_eq!(
            archive.resolve_channel(&ws, "Bob Real").map(|r| r.id),
            Some(ChannelId::from("D_BOB"))
        );
        // Unknown returns None.
        assert!(archive.resolve_channel(&ws, "#does-not-exist").is_none());
    }

    #[test]
    fn user_display_falls_back_skipping_empty_strings() {
        let dir = TempDir::new("user-display");
        let (db_path, ws) = populated_archive(&dir);
        let archive = Archive::open_path(&db_path).expect("open archive");

        assert_eq!(
            archive.user_display(&ws, &UserId::from("U1")).as_deref(),
            Some("Alice Display")
        );
        assert_eq!(
            archive.user_display(&ws, &UserId::from("U2")).as_deref(),
            Some("Bob Real")
        );
        assert_eq!(
            archive.user_display(&ws, &UserId::from("U3")).as_deref(),
            Some("carol")
        );
        assert_eq!(archive.user_display(&ws, &UserId::from("U_MISSING")), None);
    }

    #[test]
    fn users_filters_deleted_and_substrings_and_orders_case_insensitively() {
        let dir = TempDir::new("users");
        let (db_path, ws) = populated_archive(&dir);
        let archive = Archive::open_path(&db_path).expect("open archive");

        // Active users only, sorted by effective display name.
        let active = archive.users(&ws, None, false);
        let active_ids: Vec<&str> = active.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(active_ids, vec!["U1", "U2", "U3", "U4"]);

        // Include deleted brings in U5 ("Former Member", between carol and Zeta Bot).
        let all = archive.users(&ws, None, true);
        let all_ids: Vec<&str> = all.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(all_ids, vec!["U1", "U2", "U3", "U5", "U4"]);
        assert!(all.iter().find(|u| u.id == "U4").is_some_and(|u| u.is_bot));
        assert!(all.iter().find(|u| u.id == "U5").is_some_and(|u| u.deleted));

        // `@`-stripped case-insensitive substring across handle, display, real_name, id.
        assert_eq!(
            archive
                .users(&ws, Some("  @ALICE "), false)
                .into_iter()
                .map(|u| u.id)
                .collect::<Vec<_>>(),
            vec!["U1"]
        );
        assert_eq!(
            archive
                .users(&ws, Some("real"), false)
                .into_iter()
                .map(|u| u.id)
                .collect::<Vec<_>>(),
            vec!["U1", "U2"]
        );
        assert_eq!(
            archive
                .users(&ws, Some("u3"), false)
                .into_iter()
                .map(|u| u.id)
                .collect::<Vec<_>>(),
            vec!["U3"]
        );
        assert_eq!(
            archive
                .users(&ws, Some("zeta"), false)
                .into_iter()
                .map(|u| u.id)
                .collect::<Vec<_>>(),
            vec!["U4"]
        );
        // Empty/whitespace query matches all active users.
        assert_eq!(archive.users(&ws, Some("  @  "), false).len(), 4);
    }

    #[test]
    fn digest_since_enforces_cutoff_excludes_deleted_breaks_ties_and_truncates_utf8() {
        let dir = TempDir::new("digest");
        let (db_path, ws) = populated_archive(&dir);
        let archive = Archive::open_path(&db_path).expect("open archive");

        let rows = archive.digest_since(&ws, 1_700_000_100, 10);
        assert_eq!(rows.len(), 3);

        // D_ALICE and C_ENG both have 2 non-deleted messages >= cutoff, but D_ALICE has newer max ts.
        assert_eq!(rows[0].channel, "D_ALICE");
        assert_eq!(rows[0].label, "@Alice Display");
        assert_eq!(rows[0].message_count, 2);
        assert_eq!(rows[0].author_count, 1);
        assert_eq!(rows[0].latest_ts, "1700000300.000000");
        assert_eq!(rows[0].latest_author.as_deref(), Some("Alice Display"));
        assert!(rows[0].latest_text.starts_with("hello world "));
        assert!(rows[0].latest_text.ends_with('…'));
        assert_eq!(rows[0].latest_text.chars().count(), 120);

        assert_eq!(rows[1].channel, "C_ENG");
        assert_eq!(rows[1].label, "#Engineering");
        assert_eq!(rows[1].message_count, 2);
        assert_eq!(rows[1].author_count, 2);
        assert_eq!(rows[1].latest_ts, "1700000200.000000");
        assert_eq!(rows[1].latest_author.as_deref(), Some("Bob Real"));
        assert_eq!(rows[1].latest_text, "second author");

        // Orphan channel falls back to raw ID and collapses whitespace.
        assert_eq!(rows[2].channel, "C_ORPHAN");
        assert_eq!(rows[2].label, "C_ORPHAN");
        assert_eq!(rows[2].message_count, 1);
        assert_eq!(rows[2].latest_author, None);
        assert_eq!(rows[2].latest_text, "orphan msg line2");

        // Limit caps returned conversations.
        let limited = archive.digest_since(&ws, 1_700_000_100, 1);
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].channel, "D_ALICE");
    }

    #[test]
    fn latest_ts_message_count_and_notify_levels() {
        let dir = TempDir::new("meta");
        let (db_path, ws) = populated_archive(&dir);
        let archive = Archive::open_path(&db_path).expect("open archive");

        // Newest in C_ENG (1700000250) was deleted, so latest_ts skips it.
        assert_eq!(
            archive.latest_ts(&ws, &ChannelId::from("C_ENG")).as_deref(),
            Some("1700000200.000000")
        );
        assert_eq!(archive.latest_ts(&ws, &ChannelId::from("C_ALPHA")), None);

        // message_count counts message_current rows for T1 only (4 in C_ENG + 2 in D_ALICE + 1 in C_ORPHAN).
        assert_eq!(archive.message_count(&ws), 7);
        assert_eq!(archive.message_count(&TeamId::from("T2")), 1);
        assert_eq!(archive.message_count(&TeamId::from("T_NONE")), 0);

        // Valid notify_levels.
        let levels = archive.notify_levels(&ws);
        assert_eq!(
            levels.get(&ChannelId::from("C_ENG")),
            Some(&NotifyLevel::Muted)
        );
        assert_eq!(
            levels.get(&ChannelId::from("C_ALPHA")),
            Some(&NotifyLevel::Mentions)
        );
        assert!(!levels.contains_key(&ChannelId::from("C_BOB")));
        assert!(archive.notify_levels(&TeamId::from("T_NONE")).is_empty());

        // Malformed JSON in prefs falls back to empty map.
        drop(archive);
        let conn = Connection::open(&db_path).expect("reopen rw");
        conn.execute(
            "INSERT OR REPLACE INTO prefs (workspace_id, json, updated_at) VALUES (?1, ?2, 0)",
            params!["T_BAD", "{not valid json"],
        )
        .expect("insert malformed prefs");
        drop(conn);

        let archive = Archive::open_path(&db_path).expect("reopen archive");
        assert!(archive.notify_levels(&TeamId::from("T_BAD")).is_empty());
    }

    #[test]
    fn stored_team_ids_in_lists_sorted_sqlite_files_only() {
        let dir = TempDir::new("team-ids");
        std::fs::write(dir.path().join("T200.sqlite"), b"").unwrap();
        std::fs::write(dir.path().join("T100.sqlite"), b"").unwrap();
        std::fs::write(dir.path().join("T100.sqlite-wal"), b"").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"").unwrap();
        std::fs::create_dir(dir.path().join("TDIR.sqlite")).unwrap();

        let ids = stored_team_ids_in(dir.path());
        assert_eq!(ids, vec![TeamId::from("T100"), TeamId::from("T200")]);
        assert!(stored_team_ids_in(&dir.path().join("missing")).is_empty());
    }
}
