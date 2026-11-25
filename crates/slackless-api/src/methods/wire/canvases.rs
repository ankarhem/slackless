//! Slack Canvas creation, metadata, access, comments, content, and edit-operation wire types.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::de::{or_default, value_as_lenient_bool};
use crate::types::{ChannelId, File, Message, MessageSubtype, Ts, UserId};

/// Access level for `canvases.access.set`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CanvasAccessLevel {
    Read,
    Write,
    Owner,
}

impl CanvasAccessLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Owner => "owner",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "owner" => Some(Self::Owner),
            _ => None,
        }
    }
}

/// Parsed metadata for a Slack Canvas (`files.info` canvas fields + derived comment channel).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasMeta {
    pub file_id: String,
    pub title: String,
    pub permalink: Option<String>,
    pub editable: bool,
    pub access: Option<String>,
    pub org_or_workspace_access: Option<String>,
    pub canvas_printing_enabled: bool,
    pub canvas_creator_id: Option<UserId>,
    pub editors: Vec<UserId>,
    pub quip_thread_id: Option<String>,
    pub comment_channel_id: ChannelId,
}

impl Default for CanvasMeta {
    fn default() -> Self {
        Self {
            file_id: String::new(),
            title: String::new(),
            permalink: None,
            editable: false,
            access: None,
            org_or_workspace_access: None,
            canvas_printing_enabled: true,
            canvas_creator_id: None,
            editors: Vec::new(),
            quip_thread_id: None,
            comment_channel_id: ChannelId::default(),
        }
    }
}

/// One reply inside a Canvas comment thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CanvasCommentReply {
    pub ts: Ts,
    pub author: Option<UserId>,
    pub text: String,
}

/// A Canvas comment thread rooted in the canvas's file comment channel (`C…`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CanvasCommentThread {
    pub root_ts: Ts,
    pub author: Option<UserId>,
    pub text: String,
    pub reply_count: usize,
    pub replies: Vec<CanvasCommentReply>,
    pub resolved: bool,
}

/// Derives a canvas's comment file-channel id by replacing a leading `'F'` with `'C'`.
pub fn canvas_comment_channel_id(file_id: &str) -> ChannelId {
    if let Some(rest) = file_id.strip_prefix('F') {
        ChannelId::from(format!("C{rest}"))
    } else {
        ChannelId::from(file_id)
    }
}

fn lookup_canvas_field<'a>(
    file: &'a File,
    raw_file: Option<&'a Value>,
    key: &str,
) -> Option<&'a Value> {
    file.extra
        .get(key)
        .filter(|v| !v.is_null())
        .or_else(|| raw_file.and_then(|v| v.get(key)).filter(|v| !v.is_null()))
}

fn non_empty_str(val: Option<&Value>) -> Option<String> {
    val.and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Parses [`CanvasMeta`] from a [`File`] (and optional raw `files.info` or `file` JSON value).
pub fn parse_canvas_meta_from_file(file: &File, raw: Option<&Value>) -> CanvasMeta {
    let raw_file = raw.and_then(|v| v.get("file")).or(raw);

    let file_id = if !file.id.as_str().is_empty() {
        file.id.as_str().to_owned()
    } else {
        non_empty_str(raw_file.and_then(|v| v.get("id"))).unwrap_or_default()
    };

    let title = file
        .title
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| file.name.as_deref().filter(|s| !s.is_empty()))
        .or_else(|| {
            raw_file
                .and_then(|v| v.get("title"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            raw_file
                .and_then(|v| v.get("name"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("Canvas")
        .to_owned();

    let permalink = file
        .permalink
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| non_empty_str(raw_file.and_then(|v| v.get("permalink"))));

    let editable = lookup_canvas_field(file, raw_file, "editable")
        .and_then(value_as_lenient_bool)
        .unwrap_or(false);

    let access = non_empty_str(lookup_canvas_field(file, raw_file, "access"));
    let org_or_workspace_access = non_empty_str(lookup_canvas_field(
        file,
        raw_file,
        "org_or_workspace_access",
    ));
    let canvas_printing_enabled = lookup_canvas_field(file, raw_file, "canvas_printing_enabled")
        .and_then(value_as_lenient_bool)
        .unwrap_or(true);

    let canvas_creator_id = non_empty_str(lookup_canvas_field(file, raw_file, "canvas_creator_id"))
        .map(UserId::from)
        .or_else(|| {
            file.user
                .as_ref()
                .filter(|u| !u.as_str().is_empty())
                .cloned()
        });

    let editors = lookup_canvas_field(file, raw_file, "editors")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(UserId::from)
                .collect()
        })
        .unwrap_or_default();

    let quip_thread_id = non_empty_str(lookup_canvas_field(file, raw_file, "quip_thread_id"));
    let comment_channel_id = canvas_comment_channel_id(&file_id);

    CanvasMeta {
        file_id,
        title,
        permalink,
        editable,
        access,
        org_or_workspace_access,
        canvas_printing_enabled,
        canvas_creator_id,
        editors,
        quip_thread_id,
        comment_channel_id,
    }
}

/// Translates known Slack server-enforced Canvas error codes into clear user-facing messages.
pub fn format_canvas_error(err: &str) -> String {
    let normalized = err
        .trim()
        .strip_prefix("slack api error:")
        .map(str::trim)
        .unwrap_or_else(|| err.trim());

    match normalized {
        "free_teams_cannot_create_standalone_canvases" => {
            "Free Slack workspaces cannot create standalone canvases. Create a channel canvas inside a channel instead.".to_owned()
        }
        "free_team_canvas_tab_already_exists" => {
            "This channel already has a canvas tab (free workspaces allow one canvas per channel).".to_owned()
        }
        "channel_canvas_already_exists" => {
            "This channel already has a canvas.".to_owned()
        }
        "restricted_action" => {
            "You do not have permission to perform this action on this canvas.".to_owned()
        }
        "canvas_printing_disabled" => {
            "Printing and exporting is disabled for this canvas by workspace policy.".to_owned()
        }
        "not_allowed_token_type" => {
            "This canvas action is not allowed with the current Slack session token.".to_owned()
        }
        "canvas_not_found" => {
            "Canvas not found or you no longer have access to it.".to_owned()
        }
        "invalid_parameters" => {
            "Invalid canvas request parameters.".to_owned()
        }
        "missing_scope" => {
            "Your Slack session is missing the required scope for this canvas action.".to_owned()
        }
        "cant_delete_file" => {
            "This canvas cannot be deleted (you may not be the owner or it is protected).".to_owned()
        }
        _ => err.to_owned(),
    }
}

fn is_comment_message_resolved(msg: &Message) -> bool {
    if msg
        .extra
        .get("is_archived")
        .and_then(value_as_lenient_bool)
        .unwrap_or(false)
    {
        return true;
    }
    if msg
        .extra
        .get("quip_thread_archived")
        .and_then(value_as_lenient_bool)
        .unwrap_or(false)
    {
        return true;
    }
    if msg
        .extra
        .get("document_comment")
        .and_then(|dc| dc.get("is_archived"))
        .and_then(value_as_lenient_bool)
        .unwrap_or(false)
    {
        return true;
    }
    match &msg.subtype {
        Some(MessageSubtype::ChannelArchive | MessageSubtype::GroupArchive) => true,
        Some(MessageSubtype::Other(s)) => s.to_ascii_lowercase().contains("archived"),
        _ => false,
    }
}

fn comment_author(msg: &Message) -> Option<UserId> {
    msg.user
        .as_ref()
        .filter(|u| !u.as_str().is_empty())
        .cloned()
        .or_else(|| {
            msg.extra
                .get("document_comment")
                .and_then(|dc| dc.get("authors"))
                .and_then(Value::as_array)
                .and_then(|arr| arr.iter().find_map(Value::as_str))
                .filter(|s| !s.is_empty())
                .map(UserId::from)
        })
}

/// Maps comment-channel history roots and fetched replies into [`CanvasCommentThread`]s.
pub fn parse_canvas_comment_threads(
    history: &[Message],
    replies_by_root: &HashMap<Ts, Vec<Message>>,
) -> Vec<CanvasCommentThread> {
    let mut threads = Vec::new();
    for root in history {
        if root.ts.as_str().is_empty() || root.is_thread_reply() {
            continue;
        }
        let mut resolved = is_comment_message_resolved(root);
        let mut seen_reply_ts = HashSet::new();
        let mut replies = Vec::new();

        if let Some(thread_msgs) = replies_by_root.get(&root.ts) {
            for msg in thread_msgs {
                if msg.ts == root.ts {
                    if is_comment_message_resolved(msg) {
                        resolved = true;
                    }
                    continue;
                }
                if msg.ts.as_str().is_empty() || !seen_reply_ts.insert(msg.ts.clone()) {
                    continue;
                }
                replies.push(CanvasCommentReply {
                    ts: msg.ts.clone(),
                    author: msg
                        .user
                        .as_ref()
                        .filter(|u| !u.as_str().is_empty())
                        .cloned(),
                    text: msg.text.clone(),
                });
            }
        }

        let reply_count = replies.len().max(root.reply_count.unwrap_or(0) as usize);

        threads.push(CanvasCommentThread {
            root_ts: root.ts.clone(),
            author: comment_author(root),
            text: root.text.clone(),
            reply_count,
            replies,
            resolved,
        });
    }
    threads
}

/// `canvases.create` / `conversations.canvases.create` / `files.createCanvas`: created canvas ID.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CanvasCreated {
    #[serde(default, alias = "file_id", deserialize_with = "or_default")]
    pub canvas_id: String,
}

/// One section-level edit operation for `canvases.edit` (`F-COLLAB-132`, `F-SHELL-264`, `F-SHELL-277`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanvasEditOp {
    InsertAtStart {
        markdown: String,
    },
    InsertAtEnd {
        markdown: String,
    },
    InsertAfter {
        section_id: String,
        markdown: String,
    },
    InsertBefore {
        section_id: String,
        markdown: String,
    },
    /// Replace `section_id`, or the entire canvas when `section_id` is `None`.
    Replace {
        section_id: Option<String>,
        markdown: String,
    },
    Delete {
        section_id: String,
    },
    Rename {
        title: String,
    },
}

impl CanvasEditOp {
    pub(in crate::methods) fn to_change_json(&self) -> Value {
        let doc = |md: &str| serde_json::json!({ "type": "markdown", "markdown": md });
        match self {
            Self::InsertAtStart { markdown } => serde_json::json!({
                "operation": "insert_at_start",
                "document_content": doc(markdown),
            }),
            Self::InsertAtEnd { markdown } => serde_json::json!({
                "operation": "insert_at_end",
                "document_content": doc(markdown),
            }),
            Self::InsertAfter {
                section_id,
                markdown,
            } => serde_json::json!({
                "operation": "insert_after",
                "section_id": section_id,
                "document_content": doc(markdown),
            }),
            Self::InsertBefore {
                section_id,
                markdown,
            } => serde_json::json!({
                "operation": "insert_before",
                "section_id": section_id,
                "document_content": doc(markdown),
            }),
            Self::Replace {
                section_id,
                markdown,
            } => {
                let mut obj = serde_json::Map::new();
                obj.insert("operation".to_owned(), Value::String("replace".to_owned()));
                if let Some(id) = section_id.as_deref().filter(|s| !s.is_empty()) {
                    obj.insert("section_id".to_owned(), Value::String(id.to_owned()));
                }
                obj.insert("document_content".to_owned(), doc(markdown));
                Value::Object(obj)
            }
            Self::Delete { section_id } => serde_json::json!({
                "operation": "delete",
                "section_id": section_id,
            }),
            Self::Rename { title } => serde_json::json!({
                "operation": "rename",
                "title_content": doc(title),
            }),
        }
    }
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct CanvasContentWire {
    #[serde(default)]
    pub(in crate::methods) content: String,
}
