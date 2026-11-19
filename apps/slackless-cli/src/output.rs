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
