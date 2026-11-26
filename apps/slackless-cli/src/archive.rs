//! Access to the local message archive via [`slackless_core::store::Store`].
//!
//! Missing archive files return `Ok(None)` so offline commands can report empty results cleanly,
//! while genuine SQLite/corruption errors propagate explicitly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
pub use slackless_core::store::ConversationSummary as ConversationRow;
use slackless_core::store::{self, DigestChannel, Store, UserSummary};
use slackless_core::types::{ChannelId, TeamId, UserId};

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

impl From<UserSummary> for UserRow {
    fn from(user: UserSummary) -> Self {
        Self {
            id: user.id.as_str().to_owned(),
            name: user.name,
            display_name: user.display_name,
            real_name: user.real_name,
            is_bot: user.is_bot,
            deleted: user.deleted,
        }
    }
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

impl From<DigestChannel> for DigestChannelRow {
    fn from(row: DigestChannel) -> Self {
        Self {
            channel: row.channel.as_str().to_owned(),
            label: row.label,
            message_count: row.message_count,
            author_count: row.author_count,
            latest_ts: row.latest_ts.as_str().to_owned(),
            latest_author: row.latest_author,
            latest_text: row.latest_text,
        }
    }
}

/// The archive database path for one workspace.
pub fn store_path(team_id: &TeamId) -> Option<PathBuf> {
    slackless_core::paths::archive_store_path(team_id.as_str())
}

/// Team ids with an archive on disk, sorted.
pub fn stored_team_ids() -> Vec<TeamId> {
    let Some(dir) = slackless_core::paths::app_data_dir() else {
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
        .filter_map(|path| {
            let stem = path.file_stem()?.to_str()?;
            if stem.is_empty() || stem == slackless_core::paths::SEMANTIC_INDEX_STEM {
                return None;
            }
            Some(stem.to_owned())
        })
        .map(TeamId::from)
        .collect();
    ids.sort();
    ids
}

/// Format a raw channel name consistently with a single leading `#`.
pub(crate) fn format_channel_name(name: &str) -> String {
    if name.starts_with('#') {
        name.to_owned()
    } else {
        format!("#{name}")
    }
}

/// One opened SQLite archive handle (`Store`), avoiding repeated opens per CLI invocation.
pub struct Archive {
    store: Store,
}

impl Archive {
    /// Opens an existing workspace archive read-only; returns `Ok(None)` when no file exists yet
    /// and propagates genuine SQLite/corruption errors.
    pub fn open(team_id: &TeamId) -> store::Result<Option<Self>> {
        let Some(path) = store_path(team_id) else {
            return Ok(None);
        };
        Self::open_path(&path)
    }

    pub(crate) fn open_path(path: &Path) -> store::Result<Option<Self>> {
        if !path.is_file() {
            return Ok(None);
        }
        // A WAL database without its shm file cannot be opened read-only; fall back once.
        let store = match Store::open_read_only(path) {
            Ok(store) => store,
            Err(_) => Store::open(path)?,
        };
        Ok(Some(Self { store }))
    }

    /// Opens an existing workspace archive read-write; returns `Ok(None)` when no file exists yet.
    pub fn open_rw(team_id: &TeamId) -> store::Result<Option<Self>> {
        let Some(path) = store_path(team_id) else {
            return Ok(None);
        };
        Self::open_rw_path(&path)
    }

    pub(crate) fn open_rw_path(path: &Path) -> store::Result<Option<Self>> {
        if !path.is_file() {
            return Ok(None);
        }
        Ok(Some(Self {
            store: Store::open(path)?,
        }))
    }

    /// [`Archive::open_rw`] for a live command that mirrors its change into the archive: the
    /// command goes on without one, but an archive that fails to open is reported on stderr
    /// instead of passing for "no archive yet".
    pub(crate) fn open_rw_or_report(team_id: &TeamId) -> Option<Self> {
        Self::open_rw(team_id).unwrap_or_else(|err| {
            eprintln!(
                "slackless: could not open the local archive of {team_id} ({err}); continuing without it"
            );
            None
        })
    }

    /// Opens a workspace archive read-write, creating its parent directory and database if needed.
    pub fn open_or_create_rw(team_id: &TeamId) -> Result<Self> {
        let path = store_path(team_id).context("resolving the local archive path")?;
        Self::open_rw_or_create_path(&path)
    }

    /// [`Archive::open_or_create_rw`] for the archive file at `path`.
    pub(crate) fn open_rw_or_create_path(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating archive directory {}", parent.display()))?;
        }
        let store = Store::open(path)
            .with_context(|| format!("opening local archive {}", path.display()))?;
        Ok(Self { store })
    }

    /// Direct read access to the underlying [`Store`].
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Direct mutable access to the underlying [`Store`].
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    #[cfg(test)]
    pub(crate) fn from_store(store: Store) -> Self {
        Self { store }
    }

    /// Public and private channels, alphabetical by name.
    pub fn channels(&self, workspace: &TeamId) -> store::Result<Vec<ConversationRow>> {
        self.store.list_channels(workspace)
    }

    /// Direct messages and multi-person DMs, most recently active first.
    pub fn dms(&self, workspace: &TeamId) -> store::Result<Vec<ConversationRow>> {
        self.store.list_dms(workspace)
    }

    /// Channel-id to `#name` map for human result lines.
    pub fn channel_names(&self, workspace: &TeamId) -> store::Result<HashMap<String, String>> {
        let raw = self.store.channel_names(workspace)?;
        Ok(raw
            .into_iter()
            .map(|(id, name)| (id, format_channel_name(&name)))
            .collect())
    }

    /// Resolves `#name`, `name`, `@handle`, or an exact `C…`/`D…`/`G…` id to a conversation.
    pub fn resolve_channel(
        &self,
        workspace: &TeamId,
        arg: &str,
    ) -> store::Result<Option<ConversationRow>> {
        self.store.resolve_channel(workspace, arg)
    }

    /// The conversation with this exact id, if the archive knows it.
    pub fn conversation_by_id(
        &self,
        workspace: &TeamId,
        id: &ChannelId,
    ) -> store::Result<Option<ConversationRow>> {
        self.store.conversation_summary(workspace, id)
    }

    /// The best display name for a user id, from the archive's user table.
    #[cfg(test)]
    pub fn user_display(&self, workspace: &TeamId, user: &UserId) -> store::Result<Option<String>> {
        self.store.user_display(workspace, user)
    }

    /// Every user's best display name (`display_name` -> `real_name` -> `name`) for
    /// `workspace` in one query, preventing N+1 lookups across message/DM/pin/transcript rows.
    pub fn user_display_map(&self, workspace: &TeamId) -> store::Result<HashMap<UserId, String>> {
        slackless_core::export::user_display_map(&self.store, workspace)
    }

    /// Number of stored current-state messages.
    pub fn message_count(&self, workspace: &TeamId) -> store::Result<u64> {
        self.store.message_count(workspace)
    }

    /// Per-channel notification overrides (e.g. muted channels) parsed from cached Slack `prefs`.
    pub fn notify_levels(
        &self,
        workspace: &TeamId,
    ) -> store::Result<HashMap<ChannelId, slackless_core::NotifyLevel>> {
        Ok(self
            .store
            .prefs(workspace)?
            .map(|prefs| slackless_core::notify_levels(&prefs))
            .unwrap_or_default())
    }

    /// List archived workspace users, optionally filtered by a case-insensitive substring on
    /// `id`, `name`, `display_name`, or `real_name`.
    pub fn users(
        &self,
        workspace: &TeamId,
        query: Option<&str>,
        include_deleted: bool,
    ) -> store::Result<Vec<UserRow>> {
        Ok(self
            .store
            .list_users(workspace, query, include_deleted)?
            .into_iter()
            .map(UserRow::from)
            .collect())
    }

    /// Newest stored non-deleted message `ts` in `channel`, if any.
    pub fn latest_ts(
        &self,
        workspace: &TeamId,
        channel: &ChannelId,
    ) -> store::Result<Option<String>> {
        Ok(self
            .store
            .latest_ts(workspace, channel)?
            .map(|ts| ts.as_str().to_owned()))
    }

    /// Activity digest grouped by channel for messages at or after `cutoff_secs` (unix seconds).
    pub fn digest_since(
        &self,
        workspace: &TeamId,
        cutoff_secs: i64,
        limit: usize,
    ) -> store::Result<Vec<DigestChannelRow>> {
        Ok(self
            .store
            .digest_since(workspace, cutoff_secs, limit)?
            .into_iter()
            .map(DigestChannelRow::from)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    use slackless_core::NotifyLevel;
    use slackless_core::types::{Conversation, Latest, Message, Profile, TopicPurpose, Ts, User};

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
    fn open_path_returns_none_when_missing_and_err_when_corrupt() {
        let dir = TempDir::new("missing-or-corrupt");
        assert!(
            Archive::open_path(&dir.path().join("nope.sqlite"))
                .expect("missing file is Ok(None)")
                .is_none()
        );

        let corrupt = dir.path().join("corrupt.sqlite");
        std::fs::write(&corrupt, b"not a valid sqlite database file!").unwrap();
        assert!(
            Archive::open_path(&corrupt).is_err(),
            "corrupt database must propagate an error instead of returning None"
        );
    }

    #[test]
    fn channels_filters_and_sorts_case_insensitively_with_unnamed_last() {
        let dir = TempDir::new("channels");
        let (db_path, ws) = populated_archive(&dir);
        let archive = Archive::open_path(&db_path)
            .expect("open result")
            .expect("open archive");

        let channels = archive.channels(&ws).expect("channels");
        let ids: Vec<&str> = channels.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["C_ALPHA", "C_BOB", "C_ENG", "C_UNNAMED"]);

        assert_eq!(channels[0].label(), "#alpha");
        assert!(channels[0].is_archived);
        assert!(!channels[0].is_member);
        assert_eq!(channels[2].topic.as_deref(), Some("Build things"));
        assert_eq!(channels[3].label(), "C_UNNAMED");

        let names = archive.channel_names(&ws).expect("channel_names");
        assert_eq!(names.get("C_ENG").map(String::as_str), Some("#Engineering"));
        assert_eq!(names.get("C_ALPHA").map(String::as_str), Some("#alpha"));
        assert!(!names.contains_key("C_UNNAMED"));
    }

    #[test]
    fn dms_filters_and_sorts_by_latest_secs_desc_with_none_last() {
        let dir = TempDir::new("dms");
        let (db_path, ws) = populated_archive(&dir);
        let archive = Archive::open_path(&db_path)
            .expect("open result")
            .expect("open archive");

        let dms = archive.dms(&ws).expect("dms");
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
        let archive = Archive::open_path(&db_path)
            .expect("open result")
            .expect("open archive");

        // Exact ID match.
        assert_eq!(
            archive
                .resolve_channel(&ws, "C_ENG")
                .expect("resolve")
                .map(|r| r.id),
            Some(ChannelId::from("C_ENG"))
        );
        // Case-insensitive `#name` and bare `name`.
        assert_eq!(
            archive
                .resolve_channel(&ws, "#engineering")
                .expect("resolve")
                .map(|r| r.id),
            Some(ChannelId::from("C_ENG"))
        );
        assert_eq!(
            archive
                .resolve_channel(&ws, "ENGINEERING")
                .expect("resolve")
                .map(|r| r.id),
            Some(ChannelId::from("C_ENG"))
        );
        // Channel name takes precedence over IM by user handle ("bob" is both #bob and @bob).
        assert_eq!(
            archive
                .resolve_channel(&ws, "bob")
                .expect("resolve")
                .map(|r| r.id),
            Some(ChannelId::from("C_BOB"))
        );
        // `@handle` and display/real name resolve to IM when no channel shares the name.
        assert_eq!(
            archive
                .resolve_channel(&ws, "@alice")
                .expect("resolve")
                .map(|r| r.id),
            Some(ChannelId::from("D_ALICE"))
        );
        assert_eq!(
            archive
                .resolve_channel(&ws, "Alice Display")
                .expect("resolve")
                .map(|r| r.id),
            Some(ChannelId::from("D_ALICE"))
        );
        assert_eq!(
            archive
                .resolve_channel(&ws, "Bob Real")
                .expect("resolve")
                .map(|r| r.id),
            Some(ChannelId::from("D_BOB"))
        );
        // Unknown returns None.
        assert!(
            archive
                .resolve_channel(&ws, "#does-not-exist")
                .expect("resolve")
                .is_none()
        );
    }

    #[test]
    fn user_display_and_user_display_map_fall_back_skipping_empty_strings() {
        let dir = TempDir::new("user-display");
        let (db_path, ws) = populated_archive(&dir);
        let archive = Archive::open_path(&db_path)
            .expect("open result")
            .expect("open archive");

        assert_eq!(
            archive
                .user_display(&ws, &UserId::from("U1"))
                .expect("u1")
                .as_deref(),
            Some("Alice Display")
        );
        assert_eq!(
            archive
                .user_display(&ws, &UserId::from("U2"))
                .expect("u2")
                .as_deref(),
            Some("Bob Real")
        );
        assert_eq!(
            archive
                .user_display(&ws, &UserId::from("U3"))
                .expect("u3")
                .as_deref(),
            Some("carol")
        );
        assert_eq!(
            archive
                .user_display(&ws, &UserId::from("U_MISSING"))
                .expect("missing"),
            None
        );

        let map = archive.user_display_map(&ws).expect("user_display_map");
        assert_eq!(
            map.get(&UserId::from("U1")).map(String::as_str),
            Some("Alice Display")
        );
        assert_eq!(
            map.get(&UserId::from("U2")).map(String::as_str),
            Some("Bob Real")
        );
        assert_eq!(
            map.get(&UserId::from("U3")).map(String::as_str),
            Some("carol")
        );
        assert_eq!(
            map.get(&UserId::from("U5")).map(String::as_str),
            Some("Former Member")
        );
        assert!(!map.contains_key(&UserId::from("U_MISSING")));
    }

    #[test]
    fn users_filters_deleted_and_substrings_and_orders_case_insensitively() {
        let dir = TempDir::new("users");
        let (db_path, ws) = populated_archive(&dir);
        let archive = Archive::open_path(&db_path)
            .expect("open result")
            .expect("open archive");

        // Active users only, sorted by effective display name.
        let active = archive.users(&ws, None, false).expect("active");
        let active_ids: Vec<&str> = active.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(active_ids, vec!["U1", "U2", "U3", "U4"]);

        // Include deleted brings in U5 ("Former Member", between carol and Zeta Bot).
        let all = archive.users(&ws, None, true).expect("all");
        let all_ids: Vec<&str> = all.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(all_ids, vec!["U1", "U2", "U3", "U5", "U4"]);
        assert!(all.iter().find(|u| u.id == "U4").is_some_and(|u| u.is_bot));
        assert!(all.iter().find(|u| u.id == "U5").is_some_and(|u| u.deleted));

        // `@`-stripped case-insensitive substring across handle, display, real_name, id.
        assert_eq!(
            archive
                .users(&ws, Some("  @ALICE "), false)
                .expect("alice")
                .into_iter()
                .map(|u| u.id)
                .collect::<Vec<_>>(),
            vec!["U1"]
        );
        assert_eq!(
            archive
                .users(&ws, Some("real"), false)
                .expect("real")
                .into_iter()
                .map(|u| u.id)
                .collect::<Vec<_>>(),
            vec!["U1", "U2"]
        );
        assert_eq!(
            archive
                .users(&ws, Some("u3"), false)
                .expect("u3")
                .into_iter()
                .map(|u| u.id)
                .collect::<Vec<_>>(),
            vec!["U3"]
        );
        assert_eq!(
            archive
                .users(&ws, Some("zeta"), false)
                .expect("zeta")
                .into_iter()
                .map(|u| u.id)
                .collect::<Vec<_>>(),
            vec!["U4"]
        );
        // Empty/whitespace query matches all active users.
        assert_eq!(
            archive
                .users(&ws, Some("  @  "), false)
                .expect("blank")
                .len(),
            4
        );
    }

    #[test]
    fn digest_since_enforces_cutoff_excludes_deleted_breaks_ties_and_truncates_utf8() {
        let dir = TempDir::new("digest");
        let (db_path, ws) = populated_archive(&dir);
        let archive = Archive::open_path(&db_path)
            .expect("open result")
            .expect("open archive");

        let rows = archive
            .digest_since(&ws, 1_700_000_100, 10)
            .expect("digest");
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
        let limited = archive
            .digest_since(&ws, 1_700_000_100, 1)
            .expect("limited");
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].channel, "D_ALICE");
    }

    #[test]
    fn latest_ts_message_count_and_notify_levels() {
        let dir = TempDir::new("meta");
        let (db_path, ws) = populated_archive(&dir);
        let archive = Archive::open_path(&db_path)
            .expect("open result")
            .expect("open archive");

        // Newest in C_ENG (1700000250) was deleted, so latest_ts skips it.
        assert_eq!(
            archive
                .latest_ts(&ws, &ChannelId::from("C_ENG"))
                .expect("latest")
                .as_deref(),
            Some("1700000200.000000")
        );
        assert_eq!(
            archive
                .latest_ts(&ws, &ChannelId::from("C_ALPHA"))
                .expect("latest"),
            None
        );

        // message_count counts message_current rows for T1 only (4 in C_ENG + 2 in D_ALICE + 1 in C_ORPHAN).
        assert_eq!(archive.message_count(&ws).expect("count"), 7);
        assert_eq!(
            archive.message_count(&TeamId::from("T2")).expect("count"),
            1
        );
        assert_eq!(
            archive
                .message_count(&TeamId::from("T_NONE"))
                .expect("count"),
            0
        );

        // Valid notify_levels.
        let levels = archive.notify_levels(&ws).expect("notify_levels");
        assert_eq!(
            levels.get(&ChannelId::from("C_ENG")),
            Some(&NotifyLevel::Muted)
        );
        assert_eq!(
            levels.get(&ChannelId::from("C_ALPHA")),
            Some(&NotifyLevel::Mentions)
        );
        assert!(!levels.contains_key(&ChannelId::from("C_BOB")));
        assert!(
            archive
                .notify_levels(&TeamId::from("T_NONE"))
                .expect("notify")
                .is_empty()
        );

        // Malformed inner notification prefs JSON falls back to empty map.
        drop(archive);
        let mut store = Store::open(&db_path).expect("reopen rw");
        store
            .replace_prefs(
                &TeamId::from("T_BAD"),
                &serde_json::json!({ "all_notifications_prefs": "{not valid json" }),
            )
            .expect("replace malformed prefs");
        drop(store);

        let archive = Archive::open_path(&db_path)
            .expect("open result")
            .expect("reopen archive");
        assert!(
            archive
                .notify_levels(&TeamId::from("T_BAD"))
                .expect("notify")
                .is_empty()
        );
    }

    #[test]
    fn stored_team_ids_in_lists_sorted_sqlite_files_only() {
        let dir = TempDir::new("team-ids");
        std::fs::write(dir.path().join("T200.sqlite"), b"").unwrap();
        std::fs::write(dir.path().join("T100.sqlite"), b"").unwrap();
        std::fs::write(dir.path().join("semantic_index.sqlite"), b"").unwrap();
        std::fs::write(dir.path().join("T100.sqlite-wal"), b"").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"").unwrap();
        std::fs::create_dir(dir.path().join("TDIR.sqlite")).unwrap();

        let ids = stored_team_ids_in(dir.path());
        assert_eq!(ids, vec![TeamId::from("T100"), TeamId::from("T200")]);
        assert!(stored_team_ids_in(&dir.path().join("missing")).is_empty());
    }
}
