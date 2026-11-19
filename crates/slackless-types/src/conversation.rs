//! Conversations: public/private channels, DMs (IMs) and group DMs (MPIMs).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::de::{lenient_i64, null_default, ok_or_none};
use crate::ids::{ChannelId, TeamId, Ts, UserId};
use crate::message::Message;

/// A conversation object (`conversations.info/list/view`, `client.userBoot.channels/ims`,
/// `channel_created`, ...). Which flags are present depends on the endpoint and conversation
/// kind, so missing flags read as `false`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub id: ChannelId,
    /// Absent for IMs.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub name_normalized: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub is_channel: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_group: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_im: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_mpim: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_private: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_archived: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_general: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_member: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_open: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_shared: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_ext_shared: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_org_shared: bool,
    #[serde(default)]
    pub topic: Option<TopicPurpose>,
    #[serde(default)]
    pub purpose: Option<TopicPurpose>,
    /// The other party of an IM.
    #[serde(default)]
    pub user: Option<UserId>,
    #[serde(default, deserialize_with = "ok_or_none")]
    pub latest: Option<Latest>,
    #[serde(default)]
    pub last_read: Option<Ts>,
    #[serde(default)]
    pub unread_count: Option<u32>,
    #[serde(default)]
    pub unread_count_display: Option<u32>,
    #[serde(default)]
    pub num_members: Option<u32>,
    #[serde(default, deserialize_with = "null_default")]
    pub members: Vec<UserId>,
    #[serde(default)]
    pub creator: Option<UserId>,
    /// Unix seconds.
    #[serde(default, deserialize_with = "lenient_i64")]
    pub created: Option<i64>,
    /// Last metadata change; unix millis in most responses.
    #[serde(default, deserialize_with = "lenient_i64")]
    pub updated: Option<i64>,
    #[serde(default)]
    pub context_team_id: Option<TeamId>,
    /// IM sidebar priority.
    #[serde(default)]
    pub priority: Option<f64>,
    /// Fields this crate does not model (`properties`, `shared_team_ids`, ...), preserved verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The four conversation kinds the UI distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConversationKind {
    PublicChannel,
    PrivateChannel,
    Im,
    Mpim,
}

impl Conversation {
    pub fn kind(&self) -> ConversationKind {
        if self.is_im {
            ConversationKind::Im
        } else if self.is_mpim {
            ConversationKind::Mpim
        } else if self.is_private || self.is_group {
            ConversationKind::PrivateChannel
        } else {
            ConversationKind::PublicChannel
        }
    }
}

/// `topic` / `purpose`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TopicPurpose {
    #[serde(default, deserialize_with = "null_default")]
    pub value: String,
    /// Empty string when unset.
    #[serde(default)]
    pub creator: Option<UserId>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub last_set: Option<i64>,
}

/// `latest` is a bare ts in boot/counts payloads and a full message in `conversations.info/view`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Latest {
    Ts(Ts),
    Message(Box<Message>),
}

impl Latest {
    pub fn ts(&self) -> &Ts {
        match self {
            Latest::Ts(ts) => ts,
            Latest::Message(m) => &m.ts,
        }
    }
}
