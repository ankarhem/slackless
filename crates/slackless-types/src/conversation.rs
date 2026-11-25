//! Conversations: public/private channels, DMs (IMs) and group DMs (MPIMs).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::de::{lenient_i64, null_default, ok_or_none};
use crate::ids::{ChannelId, FileId, TeamId, Ts, UserId};
use crate::macros::string_enum;
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
    /// The conversation's `properties` object: tab configuration and the channel canvas.
    #[serde(default)]
    pub properties: Option<ConversationProperties>,
    /// Fields this crate does not model (`shared_team_ids`, ...), preserved verbatim.
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
            properties,
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

/// `properties`: per-conversation settings. `tabs` drives the conversation tab row and
/// `canvas` identifies the channel canvas; other keys (`posting_restricted_to`, the legacy
/// `tabz`, ...) are preserved verbatim.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConversationProperties {
    /// The tab row's configuration, in display order after the implicit Messages tab.
    #[serde(default, deserialize_with = "null_default")]
    pub tabs: Vec<ConversationTabSpec>,
    /// Present once the conversation has a canvas (even one disabled as a tab).
    #[serde(default)]
    pub canvas: Option<ChannelCanvas>,
    /// Keys this crate does not model, preserved verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

string_enum! {
    /// `properties.tabs[].type`.
    pub enum ConversationTabKind {
        Files = "files",
        /// The channel's bookmarks entry; Slack renders it as the "Pins" tab.
        Bookmarks = "bookmarks",
        ChannelCanvas = "channel_canvas",
        /// A folder of bookmarks, shown under its own label.
        Folder = "folder",
        /// A Slack List tab (not supported by the UI yet).
        List = "list",
    }
}

/// One entry of `properties.tabs`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConversationTabSpec {
    #[serde(rename = "type", default)]
    pub kind: Option<ConversationTabKind>,
    /// Tab label; the empty string means "the type's default label".
    #[serde(default)]
    pub label: Option<String>,
    /// Stable tab id (`"files"`, a bookmark id for folders, ...).
    #[serde(default)]
    pub id: Option<String>,
    /// A disabled `channel_canvas` tab hides the canvas without deleting it.
    #[serde(default, deserialize_with = "null_default")]
    pub is_disabled: bool,
    /// Type-specific payload (a folder tab's `folder_bookmark_id`).
    #[serde(default)]
    pub data: Option<TabData>,
    /// Fields this crate does not model, preserved verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `properties.tabs[].data`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TabData {
    /// The folder bookmark whose entries a `folder` tab lists.
    #[serde(default)]
    pub folder_bookmark_id: Option<String>,
    /// File id (`F…`) backing a `list` or `canvas` tab.
    #[serde(default)]
    pub file_id: Option<String>,
    /// Alternate list id key observed on some `list` tabs.
    #[serde(default)]
    pub list_id: Option<String>,
    /// Fields this crate does not model, preserved verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `properties.canvas`: the conversation's canvas, once one exists.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChannelCanvas {
    /// The canvas document's file id (`F...`), the id every `canvases.*` method takes.
    #[serde(default)]
    pub file_id: Option<FileId>,
    #[serde(default, deserialize_with = "null_default")]
    pub is_empty: bool,
    #[serde(default)]
    pub quip_thread_id: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub is_migrated: bool,
    /// Fields this crate does not model, preserved verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
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

    /// The recorded `properties` of live conversations: default tabs, a disabled canvas tab,
    /// a folder tab and a canvas reference must all parse typed while other keys survive.
    #[test]
    fn parses_recorded_properties_tabs_and_canvas() {
        let json = serde_json::json!({
            "id": "C1",
            "properties": {
                "tabs": [
                    {"type": "folder", "label": "Bookmarks", "id": "Ct1",
                     "data": {"folder_bookmark_id": "Bk1"}},
                    {"type": "bookmarks", "label": "", "id": "bookmarks"},
                    {"type": "files", "label": "", "id": "files"},
                    {"type": "channel_canvas", "label": "Canvas", "id": "channel_canvas",
                     "is_disabled": true},
                    {"type": "brand_new_2027", "label": "??"}
                ],
                "canvas": {"file_id": "F06M7CVHPJP", "is_empty": true,
                           "quip_thread_id": "Q1", "is_migrated": true},
                "tabz": [{"type": "files"}],
                "posting_restricted_to": {"type": ["ra"]}
            }
        });
        let conversation: Conversation = serde_json::from_value(json).expect("parses");
        let properties = conversation.properties.expect("properties");

        let tabs = &properties.tabs;
        assert_eq!(tabs.len(), 5, "every entry parses");
        assert_eq!(tabs[0].kind, Some(ConversationTabKind::Folder));
        assert_eq!(
            tabs[0]
                .data
                .as_ref()
                .and_then(|d| d.folder_bookmark_id.as_deref()),
            Some("Bk1"),
            "a folder tab carries its folder bookmark id"
        );
        assert_eq!(tabs[1].kind, Some(ConversationTabKind::Bookmarks));
        assert_eq!(tabs[2].kind, Some(ConversationTabKind::Files));
        let canvas_tab = &tabs[3];
        assert_eq!(canvas_tab.kind, Some(ConversationTabKind::ChannelCanvas));
        assert!(
            canvas_tab.is_disabled,
            "a disabled canvas tab stays parseable"
        );
        assert_eq!(
            tabs[4].kind,
            Some(ConversationTabKind::Other("brand_new_2027".into())),
            "an unknown tab type lands in the catch-all"
        );
        assert_eq!(
            properties.canvas.as_ref().and_then(|c| c.file_id.as_ref()),
            Some(&FileId::from("F06M7CVHPJP"))
        );
        assert!(
            properties.extra.contains_key("tabz")
                && properties.extra.contains_key("posting_restricted_to"),
            "unmodeled property keys survive verbatim"
        );
        assert!(!conversation.extra.contains_key("properties"));
    }

    #[test]
    fn merge_carries_properties_only_when_present() {
        let stored = Conversation {
            id: ChannelId::from("C1"),
            properties: Some(ConversationProperties {
                tabs: vec![ConversationTabSpec {
                    kind: Some(ConversationTabKind::Files),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        // A response without `properties` (e.g. `client.userBoot` IMs) must not erase it.
        let mut merged = stored.clone();
        merged.merge(&Conversation {
            id: ChannelId::from("C1"),
            ..Default::default()
        });
        assert_eq!(merged.properties, stored.properties);

        // A response with `properties` replaces the stored copy wholesale.
        let fresh = ConversationProperties {
            canvas: Some(ChannelCanvas {
                file_id: Some(FileId::from("F2")),
                ..Default::default()
            }),
            ..Default::default()
        };
        merged.merge(&Conversation {
            id: ChannelId::from("C1"),
            properties: Some(fresh.clone()),
            ..Default::default()
        });
        assert_eq!(merged.properties, Some(fresh));
    }

    #[test]
    fn kind_resolves_overlapping_flags_by_precedence() {
        let im = Conversation {
            is_im: true,
            is_mpim: true,
            is_private: true,
            is_group: true,
            is_channel: true,
            ..Default::default()
        };
        assert_eq!(im.kind(), ConversationKind::Im);

        let mpim = Conversation {
            is_mpim: true,
            is_private: true,
            is_group: true,
            is_channel: true,
            ..Default::default()
        };
        assert_eq!(mpim.kind(), ConversationKind::Mpim);

        let private_channel = Conversation {
            is_private: true,
            is_channel: true,
            ..Default::default()
        };
        assert_eq!(private_channel.kind(), ConversationKind::PrivateChannel);

        let legacy_group = Conversation {
            is_group: true,
            ..Default::default()
        };
        assert_eq!(legacy_group.kind(), ConversationKind::PrivateChannel);

        let public_channel = Conversation {
            is_channel: true,
            ..Default::default()
        };
        assert_eq!(public_channel.kind(), ConversationKind::PublicChannel);
        assert_eq!(
            Conversation::default().kind(),
            ConversationKind::PublicChannel
        );
    }

    #[test]
    fn merge_preserves_members_on_empty_fresh_and_overlays_extra_keys() {
        let mut stored = Conversation {
            id: ChannelId::from("C1"),
            members: vec![UserId::from("U1"), UserId::from("U2")],
            extra: Map::from_iter([
                ("kept".to_owned(), serde_json::json!(1)),
                ("overwritten".to_owned(), serde_json::json!("old")),
            ]),
            ..Default::default()
        };
        let fresh = Conversation {
            id: ChannelId::from("C1"),
            members: Vec::new(),
            extra: Map::from_iter([
                ("overwritten".to_owned(), serde_json::json!("new")),
                ("added".to_owned(), serde_json::json!(true)),
            ]),
            ..Default::default()
        };

        stored.merge(&fresh);
        assert_eq!(
            stored.members,
            vec![UserId::from("U1"), UserId::from("U2")],
            "empty fresh members must not erase stored members"
        );
        assert_eq!(stored.extra.get("kept"), Some(&serde_json::json!(1)));
        assert_eq!(
            stored.extra.get("overwritten"),
            Some(&serde_json::json!("new"))
        );
        assert_eq!(stored.extra.get("added"), Some(&serde_json::json!(true)));
    }

    #[test]
    fn latest_parses_bare_ts_numeric_ts_full_message_and_empty_object() {
        let bare: Conversation = serde_json::from_value(serde_json::json!({
            "id": "C1",
            "latest": "1700000000.000100"
        }))
        .unwrap();
        assert_eq!(bare.latest, Some(Latest::Ts(Ts::from("1700000000.000100"))));
        assert_eq!(
            bare.latest.as_ref().unwrap().ts().as_str(),
            "1700000000.000100"
        );

        let numeric: Conversation = serde_json::from_value(serde_json::json!({
            "id": "C1",
            "latest": 1700000000
        }))
        .unwrap();
        assert_eq!(numeric.latest, Some(Latest::Ts(Ts::from("1700000000"))));

        let full_msg: Conversation = serde_json::from_value(serde_json::json!({
            "id": "C1",
            "latest": {
                "type": "message",
                "ts": "1700000000.000200",
                "user": "U1",
                "text": "hello"
            }
        }))
        .unwrap();
        match full_msg.latest.as_ref().unwrap() {
            Latest::Message(m) => {
                assert_eq!(m.ts.as_str(), "1700000000.000200");
                assert_eq!(m.text, "hello");
            }
            other => panic!("expected Latest::Message, got {other:?}"),
        }
        assert_eq!(
            full_msg.latest.as_ref().unwrap().ts().as_str(),
            "1700000000.000200"
        );

        let empty_obj: Conversation = serde_json::from_value(serde_json::json!({
            "id": "C1",
            "latest": {}
        }))
        .unwrap();
        assert_eq!(empty_obj.latest, None);
    }
}
