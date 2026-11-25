//! Machine (`--json`) and human output shaping for every command.

use serde::Serialize;
use slackless_core::store::StoredMessage;
use slackless_core::types::TeamId;

use crate::archive::{Archive, ConversationRow};
use crate::session::StatusEntry;

/// Prints `value` as one JSON line.
pub fn emit_json(value: &impl Serialize) {
    println!(
        "{}",
        serde_json::to_string(value).expect("view structs are plain data")
    );
}

/// Prints `value` as JSON for `--json`, otherwise the assembled human text.
pub fn emit(json: bool, value: &impl Serialize, human: &str) {
    if json {
        emit_json(value);
    } else {
        print!("{human}");
    }
}

/// One emoji reaction aggregate.
#[derive(Serialize)]
pub struct ReactionRow {
    pub name: String,
    pub count: u32,
}

/// One message as the archive currently renders it.
#[derive(Serialize)]
pub struct MessageRow {
    pub channel: String,
    /// The message's permanent id; copy it into `reply`/`react`.
    pub ts: String,
    pub user: Option<String>,
    pub user_name: Option<String>,
    pub bot_id: Option<String>,
    pub text: String,
    pub thread_ts: Option<String>,
    pub subtype: Option<String>,
    pub deleted: bool,
    pub edited: bool,
    pub reply_count: Option<u32>,
    pub reactions: Vec<ReactionRow>,
    pub files: Vec<String>,
    pub permalink: Option<String>,
}

impl MessageRow {
    /// One message as the archive renders it, with the author's name resolved when stored.
    pub fn from_stored(
        message: &StoredMessage,
        archive: Option<&Archive>,
        workspace: &TeamId,
    ) -> Self {
        let inner = &message.message;
        Self {
            channel: message.channel.as_str().to_owned(),
            ts: inner.ts.as_str().to_owned(),
            user: inner.user.as_ref().map(|user| user.as_str().to_owned()),
            user_name: inner
                .user
                .as_ref()
                .and_then(|user| archive.and_then(|archive| archive.user_display(workspace, user))),
            bot_id: inner.bot_id.as_ref().map(|bot| bot.as_str().to_owned()),
            text: inner.text.clone(),
            thread_ts: inner.thread_ts.as_ref().map(|ts| ts.as_str().to_owned()),
            subtype: inner
                .subtype
                .as_ref()
                .map(|subtype| subtype.as_str().to_owned()),
            deleted: message.deleted,
            edited: inner.edited.is_some(),
            reply_count: inner.reply_count,
            reactions: inner
                .reactions
                .iter()
                .map(|reaction| ReactionRow {
                    name: reaction.name.clone(),
                    count: reaction.count,
                })
                .collect(),
            files: inner
                .files
                .iter()
                .filter_map(|file| file.name.clone().or_else(|| file.title.clone()))
                .collect(),
            permalink: inner.permalink.clone(),
        }
    }

    /// One tab-separated line: `channel\tts\tauthor: text`.
    pub fn human_line(&self, channel: Option<&str>) -> String {
        let who = self
            .user_name
            .as_deref()
            .or(self.user.as_deref())
            .or(self.bot_id.as_deref())
            .unwrap_or("unknown");
        let channel = channel.unwrap_or(&self.channel);
        let deleted = if self.deleted { " (deleted)" } else { "" };
        format!("{channel}\t{}\t{who}:{deleted} {}\n", self.ts, self.text)
    }
}

/// One channel or private group.
#[derive(Serialize)]
pub struct ChannelView {
    pub id: String,
    pub name: Option<String>,
    pub kind: String,
    pub is_archived: bool,
    pub is_member: bool,
    pub topic: Option<String>,
}

impl ChannelView {
    pub fn from_row(row: &ConversationRow) -> Self {
        Self {
            id: row.id.as_str().to_owned(),
            name: row.name.clone(),
            kind: row.kind.clone(),
            is_archived: row.is_archived,
            is_member: row.is_member,
            topic: row.topic.clone(),
        }
    }
}

/// Tab-separated channel lines: `id\t#name\tkind (flags)\ttopic`.
pub fn channels_human(rows: &[ChannelView]) -> String {
    let mut out = String::new();
    for row in rows {
        let name = row
            .name
            .as_deref()
            .map_or_else(|| row.id.clone(), |name| format!("#{name}"));
        let mut flags = String::new();
        if row.is_archived {
            flags.push_str(" (archived)");
        }
        if !row.is_member {
            flags.push_str(" (not a member)");
        }
        let topic = row.topic.as_deref().unwrap_or("");
        out.push_str(&format!(
            "{}\t{}{}\t{}\t{topic}\n",
            row.id, name, flags, row.kind
        ));
    }
    out
}

/// One DM or group DM.
#[derive(Serialize)]
pub struct DmView {
    pub id: String,
    /// Peer user id (`im`).
    pub user: Option<String>,
    pub user_name: Option<String>,
    pub kind: String,
    /// Group DM name, when Slack has one (`mpim`).
    pub name: Option<String>,
}

impl DmView {
    pub fn from_row(row: &ConversationRow, archive: &Archive, workspace: &TeamId) -> Self {
        let user_name = row
            .user
            .as_ref()
            .and_then(|user| archive.user_display(workspace, user));
        Self {
            id: row.id.as_str().to_owned(),
            user: row.user.as_ref().map(|user| user.as_str().to_owned()),
            user_name,
            kind: row.kind.clone(),
            name: row.name.clone(),
        }
    }
}

/// Tab-separated DM lines: `id\tpeer\tuser id\tkind`.
pub fn dms_human(rows: &[DmView]) -> String {
    let mut out = String::new();
    for row in rows {
        let peer = row
            .user_name
            .clone()
            .or_else(|| row.name.clone())
            .or_else(|| row.user.clone())
            .unwrap_or_else(|| row.id.clone());
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            row.id,
            peer,
            row.user.as_deref().unwrap_or("-"),
            row.kind
        ));
    }
    out
}

/// `auth status` for one stored session.
#[derive(Serialize)]
pub struct WorkspaceView {
    pub team_id: String,
    pub workspace_url: String,
    pub user_id: String,
    pub has_secrets: bool,
    pub archive_path: String,
    pub archive_exists: bool,
    pub messages: u64,
}

impl WorkspaceView {
    pub fn from_entry(entry: &StatusEntry) -> Self {
        Self {
            team_id: entry.team_id.as_str().to_owned(),
            workspace_url: entry.workspace_url.clone(),
            user_id: entry.user_id.as_str().to_owned(),
            has_secrets: entry.has_secrets,
            archive_path: entry.archive_path.clone(),
            archive_exists: entry.archive_exists,
            messages: entry.messages,
        }
    }
}

/// The whole `auth status` document.
#[derive(Serialize)]
pub struct AuthStatusView {
    pub signed_in: bool,
    pub workspaces: Vec<WorkspaceView>,
}

/// Human `auth status`: one block per workspace.
pub fn auth_human(view: &AuthStatusView) -> String {
    if view.workspaces.is_empty() {
        return "not signed in: no stored sessions; run `slackless auth login`\n".to_owned();
    }
    let mut out = String::new();
    if view.signed_in {
        out.push_str("signed in\n");
    } else {
        out.push_str("not signed in: stored sessions have no credentials\n");
    }
    for workspace in &view.workspaces {
        let secrets = if workspace.has_secrets {
            "stored"
        } else {
            "missing"
        };
        out.push_str(&format!(
            "{}\t{}\tuser {}\tsecrets: {}\n\tarchive: {} ({} messages)\n",
            workspace.team_id,
            workspace.workspace_url,
            workspace.user_id,
            secrets,
            workspace.archive_path,
            workspace.messages,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use slackless_core::store::Store;
    use slackless_core::types::{
        BotId, ChannelId, Edited, File, FileId, Message, MessageSubtype, Profile, Reaction, Ts,
        User, UserId,
    };

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_db_path(label: &str) -> PathBuf {
        let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "slackless-cli-output-test-{}-{}-{n}.sqlite",
            label,
            std::process::id()
        ))
    }

    #[test]
    fn message_row_from_stored_resolves_user_edited_deleted_files_and_reactions() {
        let path = temp_db_path("from-stored");
        let ws = TeamId::from("T1");
        let mut store = Store::open(&path).expect("open store");
        store
            .upsert_user(
                &ws,
                &User {
                    id: UserId::from("U1"),
                    name: "ana".into(),
                    profile: Profile {
                        display_name: Some("Ana Display".into()),
                        ..Profile::default()
                    },
                    ..User::default()
                },
            )
            .expect("upsert user");
        drop(store);

        let archive = Archive::open_path(&path).expect("open archive");
        let stored = StoredMessage {
            channel: ChannelId::from("C1"),
            deleted: true,
            message: Message {
                ts: Ts::from("1700000001.000100"),
                thread_ts: Some(Ts::from("1700000000.000001")),
                user: Some(UserId::from("U1")),
                bot_id: Some(BotId::from("B1")),
                text: "Updated body".into(),
                subtype: Some(MessageSubtype::ThreadBroadcast),
                edited: Some(Edited {
                    user: Some(UserId::from("U1")),
                    ts: Ts::from("1700000002.000000"),
                }),
                reply_count: Some(3),
                reactions: vec![
                    Reaction {
                        name: "tada".into(),
                        count: 2,
                        users: vec![],
                    },
                    Reaction {
                        name: "eyes".into(),
                        count: 1,
                        users: vec![],
                    },
                ],
                files: vec![
                    File {
                        id: FileId::from("F1"),
                        name: Some("report.pdf".into()),
                        title: Some("Q4 Report".into()),
                        ..File::default()
                    },
                    File {
                        id: FileId::from("F2"),
                        name: None,
                        title: Some("Only Title".into()),
                        ..File::default()
                    },
                    File {
                        id: FileId::from("F3"),
                        name: None,
                        title: None,
                        ..File::default()
                    },
                ],
                permalink: Some("https://acme.slack.com/archives/C1/p1".into()),
                ..Message::default()
            },
        };

        let row = MessageRow::from_stored(&stored, Some(&archive), &ws);
        assert_eq!(row.channel, "C1");
        assert_eq!(row.ts, "1700000001.000100");
        assert_eq!(row.user.as_deref(), Some("U1"));
        assert_eq!(row.user_name.as_deref(), Some("Ana Display"));
        assert_eq!(row.bot_id.as_deref(), Some("B1"));
        assert_eq!(row.thread_ts.as_deref(), Some("1700000000.000001"));
        assert_eq!(row.subtype.as_deref(), Some("thread_broadcast"));
        assert!(row.deleted);
        assert!(row.edited);
        assert_eq!(row.reply_count, Some(3));
        assert_eq!(row.reactions.len(), 2);
        assert_eq!(row.reactions[0].name, "tada");
        assert_eq!(row.reactions[0].count, 2);
        assert_eq!(row.files, vec!["report.pdf", "Only Title"]);
        assert_eq!(
            row.permalink.as_deref(),
            Some("https://acme.slack.com/archives/C1/p1")
        );

        // Without archive, user_name stays None.
        let row_no_archive = MessageRow::from_stored(&stored, None, &ws);
        assert_eq!(row_no_archive.user_name, None);

        drop(archive);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }

    #[test]
    fn human_line_author_fallback_channel_override_and_deleted_marker() {
        let base = MessageRow {
            channel: "C1".into(),
            ts: "1700000001.000100".into(),
            user: Some("U1".into()),
            user_name: Some("Ana".into()),
            bot_id: Some("B1".into()),
            text: "hi".into(),
            thread_ts: None,
            subtype: None,
            deleted: false,
            edited: false,
            reply_count: None,
            reactions: vec![],
            files: vec![],
            permalink: None,
        };

        // user_name + channel override.
        assert_eq!(
            base.human_line(Some("#eng")),
            "#eng\t1700000001.000100\tAna: hi\n"
        );

        // Fallback to user id + default channel + deleted marker.
        let deleted_user = MessageRow {
            user_name: None,
            deleted: true,
            ..base
        };
        assert_eq!(
            deleted_user.human_line(None),
            "C1\t1700000001.000100\tU1: (deleted) hi\n"
        );

        // Fallback to bot_id.
        let bot_only = MessageRow {
            user: None,
            deleted: false,
            ..deleted_user
        };
        assert_eq!(bot_only.human_line(None), "C1\t1700000001.000100\tB1: hi\n");

        // Fallback to "unknown".
        let unknown = MessageRow {
            bot_id: None,
            ..bot_only
        };
        assert_eq!(
            unknown.human_line(None),
            "C1\t1700000001.000100\tunknown: hi\n"
        );
    }

    #[test]
    fn channels_human_formats_names_flags_and_topics() {
        let rows = vec![
            ChannelView {
                id: "C1".into(),
                name: Some("eng".into()),
                kind: "public_channel".into(),
                is_archived: false,
                is_member: true,
                topic: Some("Ship it".into()),
            },
            ChannelView {
                id: "C2".into(),
                name: None,
                kind: "private_channel".into(),
                is_archived: true,
                is_member: false,
                topic: None,
            },
        ];
        let out = channels_human(&rows);
        assert_eq!(
            out,
            "C1\t#eng\tpublic_channel\tShip it\nC2\tC2 (archived) (not a member)\tprivate_channel\t\n"
        );
    }

    #[test]
    fn dms_human_peer_fallback_cascade_and_dash() {
        let rows = vec![
            DmView {
                id: "D1".into(),
                user: Some("U1".into()),
                user_name: Some("Ana".into()),
                kind: "im".into(),
                name: Some("ignored".into()),
            },
            DmView {
                id: "G1".into(),
                user: None,
                user_name: None,
                kind: "mpim".into(),
                name: Some("mpdm-a--b".into()),
            },
            DmView {
                id: "D2".into(),
                user: Some("U2".into()),
                user_name: None,
                kind: "im".into(),
                name: None,
            },
            DmView {
                id: "D3".into(),
                user: None,
                user_name: None,
                kind: "im".into(),
                name: None,
            },
        ];
        assert_eq!(
            dms_human(&rows),
            "D1\tAna\tU1\tim\nG1\tmpdm-a--b\t-\tmpim\nD2\tU2\tU2\tim\nD3\tD3\t-\tim\n"
        );
    }

    #[test]
    fn auth_human_handles_empty_signed_in_and_missing_secrets() {
        let empty = AuthStatusView {
            signed_in: false,
            workspaces: vec![],
        };
        assert_eq!(
            auth_human(&empty),
            "not signed in: no stored sessions; run `slackless auth login`\n"
        );

        let missing = AuthStatusView {
            signed_in: false,
            workspaces: vec![WorkspaceView {
                team_id: "T1".into(),
                workspace_url: "https://acme.slack.com/".into(),
                user_id: "U1".into(),
                has_secrets: false,
                archive_path: "/tmp/T1.sqlite".into(),
                archive_exists: false,
                messages: 0,
            }],
        };
        let out_missing = auth_human(&missing);
        assert!(out_missing.starts_with("not signed in: stored sessions have no credentials\n"));
        assert!(out_missing.contains("secrets: missing"));
        assert!(out_missing.contains("(0 messages)"));

        let signed = AuthStatusView {
            signed_in: true,
            workspaces: vec![WorkspaceView {
                team_id: "T2".into(),
                workspace_url: "https://beta.slack.com/".into(),
                user_id: "U2".into(),
                has_secrets: true,
                archive_path: "/tmp/T2.sqlite".into(),
                archive_exists: true,
                messages: 42,
            }],
        };
        let out_signed = auth_human(&signed);
        assert!(out_signed.starts_with("signed in\n"));
        assert!(out_signed.contains("secrets: stored"));
        assert!(out_signed.contains("archive: /tmp/T2.sqlite (42 messages)"));
    }
}
