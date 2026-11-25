//! Users, profile updates, custom emoji, `search.messages`, `search.modules.*`, and Edge search.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::de::{lenient_bool, lenient_u32, lenient_vec, or_default};
use crate::types::{
    Attachment, Block, ChannelId, Conversation, File, Message, TeamId, Ts, User, UserId,
};

/// `sort` for `search.messages`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchSort {
    #[default]
    Score,
    Timestamp,
}

/// Options for `search.messages`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchOpts {
    /// Results per page (max 100).
    pub count: Option<u32>,
    /// 1-based page (max 100). Ignored by Slack when `cursor` is set.
    pub page: Option<u32>,
    /// `*` for the first cursor page, then `pagination.next_cursor`.
    pub cursor: Option<String>,
    pub sort: SearchSort,
    /// Ascending instead of descending.
    pub ascending: bool,
    /// Wrap hits in U+E000 / U+E001.
    pub highlight: bool,
}

/// `search.messages` `messages` object.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct SearchMessages {
    #[serde(default, deserialize_with = "lenient_u32")]
    pub total: u32,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub matches: Vec<SearchMatch>,
    #[serde(default, deserialize_with = "or_default")]
    pub pagination: Option<SearchPagination>,
}

/// One `search.messages` hit. `channel` is an object here, unlike in [`Message`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct SearchMatch {
    #[serde(default, deserialize_with = "or_default")]
    pub channel: SearchChannel,
    #[serde(default)]
    pub ts: Ts,
    #[serde(default, deserialize_with = "or_default")]
    pub text: String,
    #[serde(default, deserialize_with = "or_default")]
    pub user: Option<UserId>,
    #[serde(default, deserialize_with = "or_default")]
    pub username: Option<String>,
    #[serde(default, deserialize_with = "or_default")]
    pub team: Option<TeamId>,
    #[serde(default, deserialize_with = "or_default")]
    pub permalink: Option<String>,
    #[serde(default, deserialize_with = "or_default")]
    pub iid: Option<String>,
    /// `message`, `im`, `group`, ...
    #[serde(rename = "type", default, deserialize_with = "or_default")]
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "or_default")]
    pub blocks: Vec<Block>,
    #[serde(default, deserialize_with = "or_default")]
    pub attachments: Vec<Attachment>,
    #[serde(default, deserialize_with = "or_default")]
    pub files: Vec<File>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `search.messages` match `channel`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct SearchChannel {
    #[serde(default)]
    pub id: ChannelId,
    /// For IM hits, the other user's id.
    #[serde(default, deserialize_with = "or_default")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub is_private: bool,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub is_mpim: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `search.messages` `pagination`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SearchPagination {
    #[serde(default, deserialize_with = "lenient_u32")]
    pub page: u32,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub page_count: u32,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub per_page: u32,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub total_count: u32,
    /// Empty/absent on the last cursor page.
    #[serde(default, deserialize_with = "or_default")]
    pub next_cursor: Option<String>,
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
pub(in crate::methods) struct SearchWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) messages: SearchMessages,
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
