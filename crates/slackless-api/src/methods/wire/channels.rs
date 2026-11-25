//! Conversation listing, DM tabs, channel sections, pins, bookmarks, and role assignments.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::client::Params;
use crate::de::{lenient_vec, null_default, or_default};
use crate::types::{
    ChannelId, ChannelSection, Conversation, ConversationKind, File, Latest, Message, SectionKind,
    Ts, User, UserId,
};

/// Filters for `conversations.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListOpts {
    /// Kinds to list; empty means Slack's default (public channels).
    pub kinds: Vec<ConversationKind>,
    pub exclude_archived: bool,
    /// Page size (Slack max 999, default 100).
    pub limit: Option<u32>,
}

/// `conversations.view`: channel + first page of history + side-loaded users/emoji.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ConversationView {
    #[serde(default, deserialize_with = "or_default")]
    pub channel: Option<Conversation>,
    #[serde(default, deserialize_with = "null_default")]
    pub messages: Vec<Message>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub users: Vec<User>,
    /// Custom emoji name -> URL or `alias:<name>`.
    #[serde(default, deserialize_with = "or_default")]
    pub emojis: HashMap<String, String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `users.channelSections.create`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SectionCreated {
    #[serde(default)]
    pub channel_section_id: String,
}

/// `pins.list`: a conversation's pinned messages and files, oldest pin first.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct PinsList {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub items: Vec<PinnedItem>,
}

/// One `pins.list` item: a pinned message or file.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct PinnedItem {
    /// `message` or `file`.
    #[serde(rename = "type", default, deserialize_with = "or_default")]
    pub kind: Option<String>,
    /// When it was pinned; unix seconds.
    #[serde(default, deserialize_with = "or_default")]
    pub created: Option<i64>,
    #[serde(default)]
    pub created_by: Option<UserId>,
    #[serde(default, deserialize_with = "or_default")]
    pub message: Option<Message>,
    #[serde(default, deserialize_with = "or_default")]
    pub file: Option<File>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `bookmarks.list`: a conversation's bookmarks (links, folders, shortcuts), bar order.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct BookmarksList {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub bookmarks: Vec<ChannelBookmark>,
}

/// One channel bookmark. Folders carry no `link`; entries inside a folder carry its id in
/// `parent_id` (the empty string at the top level).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ChannelBookmark {
    #[serde(default, deserialize_with = "or_default")]
    pub id: String,
    #[serde(default)]
    pub channel_id: ChannelId,
    #[serde(default, deserialize_with = "or_default")]
    pub title: String,
    #[serde(default)]
    pub link: Option<String>,
    /// Bookmark icon tag, e.g. `:clap:`.
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default)]
    pub icon_url: Option<String>,
    /// `link`, `folder`, ...
    #[serde(rename = "type", default, deserialize_with = "or_default")]
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "or_default")]
    pub parent_id: Option<String>,
    #[serde(default, deserialize_with = "or_default")]
    pub entity_id: Option<String>,
    /// Unix seconds.
    #[serde(default, deserialize_with = "or_default")]
    pub date_created: Option<i64>,
    #[serde(default)]
    pub last_updated_by_user_id: Option<UserId>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Deserialize)]
pub(in crate::methods) struct ChannelWire {
    pub(in crate::methods) channel: Conversation,
}

#[derive(Deserialize)]
pub(in crate::methods) struct ChannelsWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) channels: Vec<Conversation>,
}

/// `conversations.members` returns bare member ids.
#[derive(Deserialize)]
pub(in crate::methods) struct MemberIdsWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) members: Vec<UserId>,
}

/// `admin.roles.entity.listAssignments`: who holds which role on one entity.
#[derive(Deserialize)]
pub(in crate::methods) struct RoleAssignmentsWire {
    #[serde(default)]
    pub(in crate::methods) role_assignments: Vec<RoleAssignmentWire>,
}

#[derive(Deserialize)]
pub(in crate::methods) struct RoleAssignmentWire {
    pub(in crate::methods) role_id: String,
    #[serde(default)]
    pub(in crate::methods) users: Vec<UserId>,
}

/// Slack's channel-manager role, as its client reads it from
/// `admin.roles.entity.listAssignments`.
pub(in crate::methods) const CHANNEL_MANAGER_ROLE: &str = "Rl0A";

/// `client.dms` answers one DMs-tab row per conversation: the `channel` object with the
/// row's bare `latest` duplicated beside it.
#[derive(Deserialize)]
pub(in crate::methods) struct DmWire {
    #[serde(default)]
    pub(in crate::methods) id: ChannelId,
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) latest: Option<Ts>,
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) channel: Option<Conversation>,
}

impl DmWire {
    /// The row's conversation: the nested object when Slack sent one, else the bare minimum
    /// (`id` + `latest`) the row carries on its own.
    pub(in crate::methods) fn into_conversation(self) -> Conversation {
        let latest = self.latest.map(Latest::Ts);
        match self.channel {
            Some(mut conversation) => {
                if conversation.id.as_str().is_empty() {
                    conversation.id = self.id;
                }
                if conversation.latest.is_none() {
                    conversation.latest = latest;
                }
                conversation
            }
            None => Conversation {
                id: self.id,
                latest,
                ..Default::default()
            },
        }
    }
}

#[derive(Deserialize)]
pub(in crate::methods) struct DmsWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) ims: Vec<DmWire>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) mpims: Vec<DmWire>,
}

#[derive(Deserialize)]
pub(in crate::methods) struct ChannelSectionsWire {
    /// Pagination cursor, top-level on this method; empty string on the last page.
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) cursor: Option<String>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) channel_sections: Vec<SectionWire>,
}

#[derive(Deserialize)]
pub(in crate::methods) struct SectionWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) channel_section_id: String,
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) name: String,
    #[serde(rename = "type", default, deserialize_with = "or_default")]
    pub(in crate::methods) kind: Option<SectionKind>,
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) emoji: String,
    /// The next section in display order; empty string when this is the last one.
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) next_channel_section_id: String,
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) channel_ids_page: IdsPageWire,
}

#[derive(Default, Deserialize)]
pub(in crate::methods) struct IdsPageWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) channel_ids: Vec<ChannelId>,
}

impl From<SectionWire> for ChannelSection {
    fn from(wire: SectionWire) -> Self {
        Self {
            id: wire.channel_section_id,
            name: wire.name,
            kind: wire.kind.unwrap_or(SectionKind::Standard),
            emoji: wire.emoji,
            next_id: (!wire.next_channel_section_id.is_empty())
                .then_some(wire.next_channel_section_id),
            channel_ids: wire.channel_ids_page.channel_ids,
        }
    }
}

/// Add one page's `section` to `sections`. Later pages repeat a section to carry more of its
/// channel ids, so a known id gains the ids it lacks instead of a second entry.
pub(in crate::methods) fn merge_section(
    sections: &mut Vec<ChannelSection>,
    section: ChannelSection,
) {
    match sections.iter_mut().find(|known| known.id == section.id) {
        Some(known) => {
            for id in section.channel_ids {
                if !known.channel_ids.contains(&id) {
                    known.channel_ids.push(id);
                }
            }
        }
        None => sections.push(section),
    }
}

pub(in crate::methods) fn kind_wire(kind: ConversationKind) -> &'static str {
    match kind {
        ConversationKind::PublicChannel => "public_channel",
        ConversationKind::PrivateChannel => "private_channel",
        ConversationKind::Im => "im",
        ConversationKind::Mpim => "mpim",
    }
}

/// The shared `types`/`exclude_archived`/`limit`/`cursor` params of the conversation listing
/// methods (`conversations.list`, `users.conversations`).
pub(in crate::methods) fn listing_params(opts: &ListOpts, cursor: Option<String>) -> Params {
    let mut params = Vec::new();
    if !opts.kinds.is_empty() {
        let kinds: Vec<_> = opts.kinds.iter().copied().map(kind_wire).collect();
        params.push(("types", kinds.join(",")));
    }
    if opts.exclude_archived {
        params.push(("exclude_archived", "true".to_owned()));
    }
    if let Some(limit) = opts.limit {
        params.push(("limit", limit.to_string()));
    }
    if let Some(cursor) = cursor {
        params.push(("cursor", cursor));
    }
    params
}
