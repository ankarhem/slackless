//! Users, profile updates, custom emoji, `search.modules.*`, and Edge search.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::de::{lenient_vec, or_default};
use crate::types::{ChannelId, Conversation, File, Message, User};

/// `sort` for `search.modules.*`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchSort {
    #[default]
    Score,
    Timestamp,
}

#[derive(Deserialize)]
pub(in crate::methods) struct UserWire {
    pub(in crate::methods) user: User,
}

#[derive(Deserialize)]
pub(in crate::methods) struct MembersWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) members: Vec<User>,
}

#[derive(Deserialize)]
pub(in crate::methods) struct EmojiWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) emoji: HashMap<String, String>,
}

/// One message hit returned by `search.modules.messages`.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteMessageHit {
    pub channel: ChannelId,
    pub channel_name: Option<String>,
    pub message: Message,
}

/// One page of remote message search hits.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RemoteMessagePage {
    pub hits: Vec<RemoteMessageHit>,
    pub total: usize,
    pub page: usize,
    pub page_count: usize,
}

/// One page of remote file search hits.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RemoteFilePage {
    pub files: Vec<File>,
    pub total: usize,
    pub page: usize,
    pub page_count: usize,
}

/// Sparse payload for `users.profile.set`: only `Some` fields are serialized.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ProfileUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_emoji: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_expiration: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub real_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Value>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct ProfileSetWire {
    #[serde(default)]
    pub(in crate::methods) profile: slackless_types::Profile,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct ModulePaginationWire {
    #[serde(default)]
    pub(in crate::methods) total_count: usize,
    #[serde(default)]
    pub(in crate::methods) page: usize,
    #[serde(default)]
    pub(in crate::methods) page_count: usize,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct SearchModuleChannelWire {
    #[serde(default)]
    pub(in crate::methods) id: ChannelId,
    #[serde(default)]
    pub(in crate::methods) name: Option<String>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct SearchModuleMessageGroupWire {
    #[serde(default)]
    pub(in crate::methods) channel: SearchModuleChannelWire,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) messages: Vec<Message>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct SearchModulesMessagesWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) items: Vec<SearchModuleMessageGroupWire>,
    #[serde(default)]
    pub(in crate::methods) pagination: ModulePaginationWire,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct SearchModulesFilesWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) items: Vec<File>,
    #[serde(default)]
    pub(in crate::methods) pagination: ModulePaginationWire,
}

pub(in crate::methods) fn strip_highlight_markers(s: &str) -> String {
    s.chars()
        .filter(|&c| c != '\u{e000}' && c != '\u{e001}')
        .collect()
}

pub(in crate::methods) fn parse_search_modules_messages(
    wire: SearchModulesMessagesWire,
) -> RemoteMessagePage {
    let mut hits = Vec::new();
    for group in wire.items {
        if group.channel.id.as_str().is_empty() {
            continue;
        }
        let channel_name = group.channel.name.filter(|s| !s.is_empty());
        for mut message in group.messages {
            if message.text.contains('\u{e000}') || message.text.contains('\u{e001}') {
                message.text = strip_highlight_markers(&message.text);
            }
            hits.push(RemoteMessageHit {
                channel: group.channel.id.clone(),
                channel_name: channel_name.clone(),
                message,
            });
        }
    }
    RemoteMessagePage {
        hits,
        total: wire.pagination.total_count,
        page: wire.pagination.page.max(1),
        page_count: wire.pagination.page_count,
    }
}

pub(in crate::methods) fn parse_search_modules_files(
    wire: SearchModulesFilesWire,
) -> RemoteFilePage {
    RemoteFilePage {
        files: wire.items,
        total: wire.pagination.total_count,
        page: wire.pagination.page.max(1),
        page_count: wire.pagination.page_count,
    }
}

/// Combined remote results for the Cmd+K quick switcher (`edgeapi` `users/search` + `channels/search`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EdgeSearchResults {
    pub users: Vec<User>,
    pub channels: Vec<Conversation>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct EdgeUsersWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) results: Vec<User>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct EdgeChannelsWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) results: Vec<Conversation>,
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) member_channels: Vec<ChannelId>,
}
