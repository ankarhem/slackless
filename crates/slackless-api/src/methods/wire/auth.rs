//! Auth, workspace boot, unread counts, and team wire payloads.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::de::{lenient_bool, lenient_u32, lenient_vec, or_default};
use crate::types::{BotId, ChannelId, Conversation, Team, TeamId, Ts, User, UserId};

/// `auth.test`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct AuthTest {
    /// Workspace URL, e.g. `https://acme.slack.com/`.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub team: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub team_id: Option<TeamId>,
    #[serde(default)]
    pub user_id: Option<UserId>,
    #[serde(default)]
    pub enterprise_id: Option<TeamId>,
    #[serde(default)]
    pub bot_id: Option<BotId>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `client.userBoot`: the startup hydrate payload.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct UserBoot {
    #[serde(rename = "self", default, deserialize_with = "or_default")]
    pub self_user: Option<User>,
    #[serde(default, deserialize_with = "or_default")]
    pub team: Option<Team>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub channels: Vec<Conversation>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub ims: Vec<Conversation>,
    /// Open conversation ids (sidebar).
    #[serde(default, deserialize_with = "or_default")]
    pub is_open: Vec<ChannelId>,
    #[serde(default, deserialize_with = "or_default")]
    pub starred: Vec<ChannelId>,
    /// Channel id -> 0..1 relevance score for sidebar ordering.
    #[serde(default, deserialize_with = "or_default")]
    pub channels_priority: HashMap<String, f64>,
    #[serde(default)]
    pub prefs: Option<Value>,
    #[serde(default)]
    pub dnd: Option<Value>,
    #[serde(default)]
    pub slack_route: Option<String>,
    #[serde(default)]
    pub emoji_cache_ts: Option<Value>,
    /// Warm boots only: channels whose cached data is still valid.
    #[serde(default, deserialize_with = "or_default")]
    pub unchanged_channel_ids: Vec<ChannelId>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `client.counts`: unread/mention state.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ClientCounts {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub channels: Vec<ConversationCounts>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub ims: Vec<ConversationCounts>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub mpims: Vec<ConversationCounts>,
    #[serde(default, deserialize_with = "or_default")]
    pub threads: ThreadCounts,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One conversation's entry in [`ClientCounts`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ConversationCounts {
    pub id: ChannelId,
    #[serde(default, deserialize_with = "or_default")]
    pub last_read: Option<Ts>,
    #[serde(default, deserialize_with = "or_default")]
    pub latest: Option<Ts>,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub mention_count: u32,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub dm_count: u32,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub has_unreads: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ConversationCounts {
    /// Mention or DM unread count (`client.counts` `ims` entries may report `dm_count`).
    pub fn effective_mentions(&self) -> u32 {
        self.mention_count.max(self.dm_count)
    }

    /// Whether this conversation currently has any unread messages.
    pub fn is_unread(&self) -> bool {
        self.has_unreads || self.mention_count > 0 || self.dm_count > 0
    }

    /// Clear all unread flags and mention/DM counters.
    pub fn clear_unreads(&mut self) {
        self.has_unreads = false;
        self.mention_count = 0;
        self.dm_count = 0;
    }
}

/// `client.counts` `threads`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ThreadCounts {
    #[serde(default, deserialize_with = "lenient_bool")]
    pub has_unreads: bool,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub mention_count: u32,
}

#[derive(Deserialize)]
pub(in crate::methods) struct TeamWire {
    pub(in crate::methods) team: Team,
}
