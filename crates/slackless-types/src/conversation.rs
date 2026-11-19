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

    /// Overlays a fresher copy from Slack on this stored one. Endpoints return partial
    /// conversation objects (`client.userBoot` IMs carry unread counts that
    /// `conversations.info` omits; only `conversations.info` carries `num_members`), so a
    /// field moves only when the fresh copy actually carries it: `Option`s when `Some`,
    /// `members` when non-empty, flags when `true` (an omitted flag deserializes as `false`
    /// and must not erase a stored `true`).
    pub fn merge(&mut self, fresh: &Conversation) {
        macro_rules! carried {
            ($($field:ident),* $(,)?) => {$(
                if fresh.$field.is_some() {
                    self.$field = fresh.$field.clone();
                }
            )*};
        }
        carried!(
            name,
            name_normalized,
            topic,
            purpose,
            user,
            latest,
            last_read,
            unread_count,
            unread_count_display,
            num_members,
            creator,
            created,
            updated,
            context_team_id,
            priority,
        );
        macro_rules! flagged {
            ($($field:ident),* $(,)?) => {$(
                if fresh.$field {
                    self.$field = true;
                }
            )*};
        }
        flagged!(
            is_channel,
            is_group,
            is_im,
            is_mpim,
            is_private,
            is_archived,
            is_general,
            is_member,
            is_open,
            is_shared,
            is_ext_shared,
            is_org_shared,
        );
        if !fresh.members.is_empty() {
            self.members = fresh.members.clone();
        }
        for (key, value) in &fresh.extra {
            self.extra.insert(key.clone(), value.clone());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::UserId;

    #[test]
    fn merge_takes_carried_fields_and_keeps_absent_ones() {
        let stored = Conversation {
            id: ChannelId::from("C1"),
            name: Some("general".into()),
            is_channel: true,
            is_member: true,
            unread_count_display: Some(7),
            priority: Some(0.5),
            topic: Some(TopicPurpose {
                value: "kept".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let fresh = Conversation {
            id: ChannelId::from("C1"),
            num_members: Some(42),
            is_channel: true,
            // Omitted by the endpoint that produced `fresh`:
            name: None,
            is_member: false,
            unread_count_display: None,
            priority: None,
            topic: None,
            ..Default::default()
        };

        let mut merged = stored;
        merged.merge(&fresh);

        assert_eq!(merged.num_members, Some(42), "newly carried fields land");
        assert_eq!(merged.name.as_deref(), Some("general"));
        assert_eq!(merged.unread_count_display, Some(7));
        assert_eq!(merged.priority, Some(0.5));
        assert_eq!(
            merged.topic.as_ref().map(|t| t.value.as_str()),
            Some("kept")
        );
        assert!(
            merged.is_member,
            "an omitted flag must not erase a stored true"
        );
    }

    #[test]
    fn merge_replaces_carried_values_and_flags_turning_true() {
        let mut merged = Conversation {
            id: ChannelId::from("C1"),
            name: Some("old".into()),
            is_archived: false,
            members: Vec::new(),
            ..Default::default()
        };
        merged.merge(&Conversation {
            id: ChannelId::from("C1"),
            name: Some("renamed".into()),
            is_archived: true,
            members: vec![UserId::from("U1"), UserId::from("U2")],
            ..Default::default()
        });

        assert_eq!(merged.name.as_deref(), Some("renamed"));
        assert!(merged.is_archived);
        assert_eq!(merged.members.len(), 2);
    }
}
