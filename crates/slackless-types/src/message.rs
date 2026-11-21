//! The message object shared by history/replies responses, `chat.*` responses and websocket
//! `message` events.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::de::{lenient_i64, null_default, ok_or_none};
use crate::file::File;
use crate::ids::{BotId, ChannelId, TeamId, Ts, UserId};
use crate::macros::string_enum;
use crate::reaction::Reaction;
use crate::rich_text::Block;
use crate::user::Profile;

string_enum! {
    /// `message.subtype`. A plain user message has no subtype (`Message::subtype == None`);
    /// unrecognized subtypes land in `Other`.
    pub enum MessageSubtype {
        BotMessage = "bot_message",
        MeMessage = "me_message",
        MessageChanged = "message_changed",
        MessageDeleted = "message_deleted",
        MessageReplied = "message_replied",
        ThreadBroadcast = "thread_broadcast",
        /// Deprecated predecessor of `thread_broadcast`.
        ReplyBroadcast = "reply_broadcast",
        ChannelJoin = "channel_join",
        ChannelLeave = "channel_leave",
        ChannelTopic = "channel_topic",
        ChannelPurpose = "channel_purpose",
        ChannelName = "channel_name",
        ChannelArchive = "channel_archive",
        ChannelUnarchive = "channel_unarchive",
        GroupJoin = "group_join",
        GroupLeave = "group_leave",
        GroupTopic = "group_topic",
        GroupPurpose = "group_purpose",
        GroupName = "group_name",
        GroupArchive = "group_archive",
        GroupUnarchive = "group_unarchive",
        FileShare = "file_share",
        FileComment = "file_comment",
        FileMention = "file_mention",
        PinnedItem = "pinned_item",
        UnpinnedItem = "unpinned_item",
        BotAdd = "bot_add",
        BotRemove = "bot_remove",
        HuddleThread = "huddle_thread",
        EkmAccessDenied = "ekm_access_denied",
        ChannelPostingPermissions = "channel_posting_permissions",
        AssistantAppThread = "assistant_app_thread",
    }
}

/// A Slack message.
///
/// `channel` is absent in history/replies responses (the channel is the request's) and present in
/// websocket events. Everything except `ts` is optional; unmodeled fields are kept in `extra`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// The message id within its channel.
    pub ts: Ts,
    /// Thread root ts; equals `ts` on the root itself.
    #[serde(default)]
    pub thread_ts: Option<Ts>,
    #[serde(default)]
    pub channel: Option<ChannelId>,
    #[serde(default)]
    pub user: Option<UserId>,
    #[serde(default)]
    pub bot_id: Option<BotId>,
    #[serde(default)]
    pub team: Option<TeamId>,
    /// mrkdwn fallback text; render `blocks` when present.
    #[serde(default, deserialize_with = "null_default")]
    pub text: String,
    #[serde(default)]
    pub subtype: Option<MessageSubtype>,
    #[serde(default, deserialize_with = "null_default")]
    pub blocks: Vec<Block>,
    #[serde(default, deserialize_with = "null_default")]
    pub attachments: Vec<Attachment>,
    #[serde(default, deserialize_with = "null_default")]
    pub files: Vec<File>,
    #[serde(default, deserialize_with = "null_default")]
    pub reactions: Vec<Reaction>,
    #[serde(default)]
    pub edited: Option<Edited>,
    #[serde(default)]
    pub reply_count: Option<u32>,
    #[serde(default)]
    pub reply_users_count: Option<u32>,
    #[serde(default, deserialize_with = "null_default")]
    pub reply_users: Vec<UserId>,
    #[serde(default)]
    pub latest_reply: Option<Ts>,
    /// Author of the thread root, on replies.
    #[serde(default)]
    pub parent_user_id: Option<UserId>,
    /// Client-generated UUID; the dedup key between an optimistic send and its echo.
    #[serde(default)]
    pub client_msg_id: Option<String>,
    #[serde(default)]
    pub bot_profile: Option<BotProfile>,
    /// Trimmed author profile embedded in the message.
    #[serde(default)]
    pub user_profile: Option<Profile>,
    /// Display name override (bots, integrations).
    #[serde(default)]
    pub username: Option<String>,
    /// Metadata-only messages (`message_changed`, `message_deleted`, ...) that must not render.
    #[serde(default, deserialize_with = "null_default")]
    pub hidden: bool,
    #[serde(default)]
    pub deleted_ts: Option<Ts>,
    #[serde(default)]
    pub event_ts: Option<Ts>,
    /// Thread root summary on `thread_broadcast`.
    #[serde(default, deserialize_with = "ok_or_none")]
    pub root: Option<Box<Message>>,
    /// Thread-root read state.
    #[serde(default)]
    pub last_read: Option<Ts>,
    #[serde(default)]
    pub subscribed: Option<bool>,
    #[serde(default)]
    pub is_locked: Option<bool>,
    #[serde(default, deserialize_with = "null_default")]
    pub pinned_to: Vec<ChannelId>,
    #[serde(default)]
    pub permalink: Option<String>,
    /// Fields this crate does not model, preserved verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Message {
    /// True for a thread reply (as opposed to a root or a plain channel message).
    pub fn is_thread_reply(&self) -> bool {
        self.thread_ts.as_ref().is_some_and(|t| *t != self.ts)
    }
}

/// `edited`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edited {
    #[serde(default)]
    pub user: Option<UserId>,
    pub ts: Ts,
}

/// `bot_profile` on bot/app messages.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BotProfile {
    pub id: BotId,
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default)]
    pub icons: Option<BotIcons>,
    #[serde(default, deserialize_with = "null_default")]
    pub deleted: bool,
    #[serde(default)]
    pub team_id: Option<TeamId>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub updated: Option<i64>,
}

/// Bot avatar URLs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotIcons {
    #[serde(default)]
    pub image_36: Option<String>,
    #[serde(default)]
    pub image_48: Option<String>,
    #[serde(default)]
    pub image_72: Option<String>,
    #[serde(default)]
    pub emoji: Option<String>,
}

/// A legacy attachment; today mostly server-side link unfurls (`from_url` set) and app cards.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    #[serde(default, deserialize_with = "lenient_i64")]
    pub id: Option<i64>,
    #[serde(default)]
    pub fallback: Option<String>,
    /// Hex color (with or without `#`) or `good`/`warning`/`danger`.
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub pretext: Option<String>,
    #[serde(default)]
    pub author_name: Option<String>,
    #[serde(default)]
    pub author_link: Option<String>,
    #[serde(default)]
    pub author_icon: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub title_link: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub image_url: Option<String>,
    #[serde(default)]
    pub thumb_url: Option<String>,
    /// The unfurled URL.
    #[serde(default)]
    pub from_url: Option<String>,
    /// A shared-message unfurl (`is_msg_unfurl`): the conversation the shared message lives
    /// in, so the UI can name "Thread in <channel>" even when it is not in the local store.
    #[serde(default)]
    pub channel_id: Option<ChannelId>,
    #[serde(default, deserialize_with = "null_default")]
    pub is_msg_unfurl: bool,
    #[serde(default)]
    pub original_url: Option<String>,
    #[serde(default)]
    pub service_name: Option<String>,
    #[serde(default)]
    pub service_icon: Option<String>,
    #[serde(default)]
    pub footer: Option<String>,
    #[serde(default)]
    pub footer_icon: Option<String>,
    /// Number or string depending on the producer.
    #[serde(default)]
    pub ts: Option<Value>,
    #[serde(default, deserialize_with = "null_default")]
    pub fields: Vec<AttachmentField>,
    #[serde(default, deserialize_with = "null_default")]
    pub blocks: Vec<Block>,
    /// Fields this crate does not model (`actions`, `mrkdwn_in`, ...), preserved verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A title/value pair inside an attachment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentField {
    #[serde(default, deserialize_with = "ok_or_none")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "ok_or_none")]
    pub value: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub short: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rich_text::{
        BroadcastRange, ListStyle, RichTextBlock, RichTextContainer, RichTextElement,
    };

    /// A `chat.postMessage` response as the web client receives it: one rich_text block with
    /// styled text, mentions, emoji, a link, a list, a quote and a code block.
    const POST_MESSAGE_RESPONSE: &str = r##"{
      "ok": true,
      "channel": "C0123ABCD",
      "ts": "1772325870.309319",
      "message": {
        "type": "message",
        "user": "U0123USER",
        "team": "T0123TEAM",
        "text": "hey <@U0456> *bold* check <https://example.com|this>",
        "ts": "1772325870.309319",
        "client_msg_id": "a789f1c4-6d6e-4e4f-9a7b-2f6e0c5d1a11",
        "blocks": [{
          "type": "rich_text",
          "block_id": "Xy1z",
          "elements": [
            {"type": "rich_text_section", "elements": [
              {"type": "text", "text": "hey "},
              {"type": "user", "user_id": "U0456"},
              {"type": "text", "text": " "},
              {"type": "text", "text": "bold", "style": {"bold": true}},
              {"type": "text", "text": " and ", "style": {"italic": true, "strike": true}},
              {"type": "emoji", "name": "thumbsup", "unicode": "1f44d", "skin_tone": 2},
              {"type": "link", "url": "https://example.com", "text": "this"},
              {"type": "channel", "channel_id": "C0789"},
              {"type": "usergroup", "usergroup_id": "S0111"},
              {"type": "broadcast", "range": "here"},
              {"type": "date", "timestamp": 1700000000, "format": "{date_short}", "fallback": "Nov 14"},
              {"type": "color", "value": "#FF0000"},
              {"type": "attachment_mention", "url": "https://example.atlassian.net/browse/X-1", "text": "X-1 title", "app_id": "A0JIRA", "icon_name": "brand-jira-filled"}
            ]},
            {"type": "rich_text_list", "style": "ordered", "indent": 0, "border": 0, "elements": [
              {"type": "rich_text_section", "elements": [{"type": "text", "text": "one"}]},
              {"type": "rich_text_section", "elements": [{"type": "text", "text": "two", "style": {"code": true}}]}
            ]},
            {"type": "rich_text_quote", "elements": [{"type": "text", "text": "quoted"}]},
            {"type": "rich_text_preformatted", "border": 0, "elements": [{"type": "text", "text": "let x = 1;"}]}
          ]
        }],
        "reactions": [{"name": "eyes", "users": ["U1", "U2"], "count": 2}],
        "edited": {"user": "U0123USER", "ts": "1772325871.000000"},
        "some_future_field": {"nested": true}
      }
    }"##;

    #[derive(Deserialize)]
    struct PostMessage {
        channel: ChannelId,
        ts: Ts,
        message: Message,
    }

    #[test]
    fn parses_post_message_with_rich_text() {
        let resp: crate::response::ApiResponse<PostMessage> =
            serde_json::from_str(POST_MESSAGE_RESPONSE).unwrap();
        assert!(resp.ok);
        let PostMessage {
            channel,
            ts,
            message,
        } = resp.data;
        assert_eq!(channel.as_str(), "C0123ABCD");
        assert_eq!(ts, message.ts);
        assert_eq!(message.user.as_ref().unwrap().as_str(), "U0123USER");
        assert_eq!(message.subtype, None);
        assert_eq!(message.reactions[0].count, 2);
        assert_eq!(
            message.edited.as_ref().unwrap().ts.as_str(),
            "1772325871.000000"
        );
        assert!(message.extra.contains_key("some_future_field"));

        let Block::RichText(RichTextBlock { elements, block_id }) = &message.blocks[0] else {
            panic!("expected rich_text block, got {:?}", message.blocks[0]);
        };
        assert_eq!(block_id.as_deref(), Some("Xy1z"));
        assert_eq!(elements.len(), 4);

        let RichTextContainer::Section(section) = &elements[0] else {
            panic!("expected section");
        };
        let unknown: Vec<_> = section
            .elements
            .iter()
            .filter(|e| matches!(e, RichTextElement::Unknown { .. }))
            .collect();
        assert!(unknown.is_empty(), "unparsed inline elements: {unknown:?}");
        match &section.elements[3] {
            RichTextElement::Text(t) => {
                assert_eq!(t.text, "bold");
                assert!(t.style.unwrap().bold);
            }
            other => panic!("expected text, got {other:?}"),
        }
        assert!(matches!(
            &section.elements[9],
            RichTextElement::Broadcast(b) if b.range == BroadcastRange::Here
        ));
        assert!(matches!(
            &elements[1],
            RichTextContainer::List(l) if l.style == ListStyle::Ordered && l.elements.len() == 2
        ));
        assert!(matches!(&elements[2], RichTextContainer::Quote(_)));
        assert!(matches!(&elements[3], RichTextContainer::Preformatted(_)));
    }

    #[test]
    fn rich_text_round_trips() {
        let resp: crate::response::ApiResponse<PostMessage> =
            serde_json::from_str(POST_MESSAGE_RESPONSE).unwrap();
        let original = resp.data.message;
        let json = serde_json::to_value(&original).unwrap();
        let reparsed: Message = serde_json::from_value(json).unwrap();
        assert_eq!(original, reparsed);
    }

    #[test]
    fn plain_text_block_matches_web_client_wire_format() {
        let block: Block = RichTextBlock::plain_text("hello").into();
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            serde_json::json!({
                "type": "rich_text",
                "elements": [{"type": "rich_text_section", "elements": [{"type": "text", "text": "hello"}]}]
            })
        );
    }

    #[test]
    fn unknown_subtype_and_elements_are_preserved() {
        let msg: Message = serde_json::from_str(
            r#"{
              "type": "message",
              "subtype": "brand_new_subtype_2027",
              "ts": "1700000000.000100",
              "text": null,
              "blocks": [
                {"type": "section", "text": {"type": "mrkdwn", "text": "app block"}},
                {"type": "rich_text", "elements": [
                  {"elements": [{"type": "sparkle", "intensity": 9}]}
                ]}
              ]
            }"#,
        )
        .unwrap();
        assert_eq!(
            msg.subtype,
            Some(MessageSubtype::Other("brand_new_subtype_2027".into()))
        );
        assert_eq!(msg.text, "");
        assert!(matches!(&msg.blocks[0], Block::Unknown { r#type, .. } if r#type == "section"));
        let Block::RichText(rt) = &msg.blocks[1] else {
            panic!("expected rich_text");
        };
        // Missing container type defaults to rich_text_section.
        let RichTextContainer::Section(section) = &rt.elements[0] else {
            panic!("expected section, got {:?}", rt.elements[0]);
        };
        assert!(matches!(
            &section.elements[0],
            RichTextElement::Unknown { r#type, raw } if r#type == "sparkle" && raw["intensity"] == 9
        ));
        // Serializes back with the unknown subtype intact.
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["subtype"], "brand_new_subtype_2027");
        assert_eq!(json["blocks"][0]["type"], "section");
    }
}
