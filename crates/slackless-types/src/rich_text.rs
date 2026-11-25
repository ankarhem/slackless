//! Block Kit `blocks` and the `rich_text` block model the web client sends and receives.
//!
//! A user-typed message is one `rich_text` block whose `elements` are containers
//! (section/list/quote/preformatted) holding inline elements (text, link, mentions, emoji, ...).
//! Every level has an `Unknown` catch-all so a new element type never fails a message.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::de::{is_false, null_default};
use crate::ids::{ChannelId, FileId, TeamId, Ts, UserId, UsergroupId};
use crate::macros::{string_enum, tagged_enum};

tagged_enum! {
    /// A top-level Block Kit block. Only `rich_text` is modeled; everything else (section,
    /// context, image, actions, ... from apps) is kept raw in `Unknown`.
    pub enum Block (tag = "type", default = None) {
        "rich_text" => RichText(RichTextBlock),
    }
}

/// `{"type":"rich_text","block_id":...,"elements":[...]}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RichTextBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub elements: Vec<RichTextContainer>,
}

impl RichTextBlock {
    /// One section with one plain text run: what the web client sends for a plain message.
    pub fn plain_text(text: impl Into<String>) -> Self {
        Self {
            block_id: None,
            elements: vec![RichTextContainer::Section(RichTextSection {
                elements: vec![RichTextElement::Text(TextElement {
                    text: text.into(),
                    style: None,
                })],
            })],
        }
    }
}

impl From<RichTextBlock> for Block {
    fn from(block: RichTextBlock) -> Self {
        Block::RichText(block)
    }
}

tagged_enum! {
    /// A container inside a `rich_text` block. A missing `type` means `rich_text_section`.
    pub enum RichTextContainer (tag = "type", default = Some("rich_text_section")) {
        "rich_text_section" => Section(RichTextSection),
        "rich_text_list" => List(RichTextList),
        "rich_text_quote" => Quote(RichTextQuote),
        "rich_text_preformatted" => Preformatted(RichTextPreformatted),
        "rich_text_header" => Header(RichTextHeader),
        "rich_text_divider" => Divider(RichTextDivider),
    }
}

/// A horizontal rule; Slack serializes it as just `{"type": "rich_text_divider"}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RichTextDivider {}

/// A paragraph of inline elements.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RichTextSection {
    #[serde(default, deserialize_with = "null_default")]
    pub elements: Vec<RichTextElement>,
}

/// A heading paragraph (the composer's Big / Medium / Small heading). `level` is 1 (big) to
/// 3 (small); Slack reads a missing level as 2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RichTextHeader {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<u8>,
    #[serde(default, deserialize_with = "null_default")]
    pub elements: Vec<RichTextElement>,
}

string_enum! {
    /// `rich_text_list.style`.
    pub enum ListStyle {
        Bullet = "bullet",
        Ordered = "ordered",
    }
}

/// A bullet or ordered list; each item is a container (normally a section).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RichTextList {
    #[serde(default = "default_list_style")]
    pub style: ListStyle,
    #[serde(default, deserialize_with = "null_default")]
    pub elements: Vec<RichTextContainer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indent: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<u32>,
}

fn default_list_style() -> ListStyle {
    ListStyle::Bullet
}

/// A block quote.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RichTextQuote {
    #[serde(default, deserialize_with = "null_default")]
    pub elements: Vec<RichTextElement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<u32>,
}

/// A code block.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RichTextPreformatted {
    #[serde(default, deserialize_with = "null_default")]
    pub elements: Vec<RichTextElement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

tagged_enum! {
    /// An inline element inside a container.
    pub enum RichTextElement (tag = "type", default = None) {
        "text" => Text(TextElement),
        "link" => Link(LinkElement),
        "user" => User(UserElement),
        "channel" => Channel(ChannelElement),
        "usergroup" => Usergroup(UsergroupElement),
        "team" => Team(TeamElement),
        "broadcast" => Broadcast(BroadcastElement),
        "emoji" => Emoji(EmojiElement),
        "date" => Date(DateElement),
        "color" => Color(ColorElement),
        "message_mention" => MessageMention(MessageMentionElement),
        "canvas" => Canvas(CanvasElement),
        "attachment_mention" => AttachmentMention(AttachmentMentionElement),
    }
}

/// Inline style flags. Only `true` flags are serialized.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextStyle {
    #[serde(skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub strike: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub code: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub underline: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub highlight: bool,
    /// Set by the server on search-result matches.
    #[serde(skip_serializing_if = "is_false")]
    pub client_highlight: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub unlink: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TextElement {
    #[serde(default, deserialize_with = "null_default")]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<TextStyle>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LinkElement {
    #[serde(default, deserialize_with = "null_default")]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<TextStyle>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserElement {
    pub user_id: UserId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<TextStyle>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChannelElement {
    pub channel_id: ChannelId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<TextStyle>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsergroupElement {
    pub usergroup_id: UsergroupId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<TextStyle>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TeamElement {
    pub team_id: TeamId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<TextStyle>,
}

string_enum! {
    /// `@here` / `@channel` / `@everyone`.
    pub enum BroadcastRange {
        Here = "here",
        Channel = "channel",
        Everyone = "everyone",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BroadcastElement {
    pub range: BroadcastRange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<TextStyle>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EmojiElement {
    /// Shortcode without colons, e.g. `thumbsup`.
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    /// Hex code points joined by `-`, e.g. `1f44d`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unicode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin_tone: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<TextStyle>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DateElement {
    /// Unix seconds.
    #[serde(default)]
    pub timestamp: i64,
    /// Slack date format string, e.g. `{date_short_pretty} at {time}`.
    #[serde(default, deserialize_with = "null_default")]
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ColorElement {
    /// Hex color, e.g. `#FF0000`.
    #[serde(default, deserialize_with = "null_default")]
    pub value: String,
}

/// A pasted message permalink rendered as a mention.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageMentionElement {
    pub channel_id: ChannelId,
    pub message_ts: Ts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_id: Option<UserId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CanvasElement {
    pub file_id: FileId,
}

/// A link an app unfurls inline (a Jira issue, say): the page's title as a chip.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AttachmentMentionElement {
    #[serde(default, deserialize_with = "null_default")]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<TextStyle>,
    /// The app's ids, icon and the source message (`app_id`, `icon_url`, `channel_id`, ...).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn header_divider_and_default_bullet_list_round_trip() {
        let block: Block = serde_json::from_value(json!({
            "type": "rich_text",
            "elements": [
                {
                    "type": "rich_text_header",
                    "level": 1,
                    "elements": [{"type": "text", "text": "Heading"}]
                },
                {
                    "type": "rich_text_header",
                    "elements": [{"type": "text", "text": "Default level"}]
                },
                {
                    "type": "rich_text_divider"
                },
                {
                    "type": "rich_text_list",
                    "elements": [
                        {"type": "rich_text_section", "elements": [{"type": "text", "text": "item"}]}
                    ]
                }
            ]
        }))
        .unwrap();

        let Block::RichText(rt) = &block else {
            panic!("expected RichText, got {block:?}");
        };
        assert_eq!(rt.elements.len(), 4);
        assert!(matches!(
            &rt.elements[0],
            RichTextContainer::Header(h) if h.level == Some(1) && h.elements.len() == 1
        ));
        assert!(matches!(
            &rt.elements[1],
            RichTextContainer::Header(h) if h.level.is_none()
        ));
        assert!(matches!(
            &rt.elements[2],
            RichTextContainer::Divider(RichTextDivider {})
        ));
        assert!(matches!(
            &rt.elements[3],
            RichTextContainer::List(l) if l.style == ListStyle::Bullet
        ));

        let serialized = serde_json::to_value(&block).unwrap();
        assert_eq!(
            serialized["elements"][2],
            json!({"type": "rich_text_divider"})
        );
        let reparsed: Block = serde_json::from_value(serialized).unwrap();
        assert_eq!(reparsed, block);
    }

    #[test]
    fn team_message_mention_and_canvas_elements_round_trip() {
        let section: RichTextContainer = serde_json::from_value(json!({
            "type": "rich_text_section",
            "elements": [
                {"type": "team", "team_id": "T123", "style": {"bold": true}},
                {
                    "type": "message_mention",
                    "channel_id": "C123",
                    "message_ts": "1700000000.000100",
                    "author_id": "U456",
                    "text": "preview",
                    "url": "https://acme.slack.com/archives/C123/p1700000000000100"
                },
                {"type": "canvas", "file_id": "F789"}
            ]
        }))
        .unwrap();

        let RichTextContainer::Section(sec) = &section else {
            panic!("expected Section");
        };
        assert!(matches!(
            &sec.elements[0],
            RichTextElement::Team(TeamElement { team_id, style })
                if team_id.as_str() == "T123" && style.unwrap().bold
        ));
        assert!(matches!(
            &sec.elements[1],
            RichTextElement::MessageMention(MessageMentionElement {
                channel_id,
                message_ts,
                author_id,
                text,
                url,
            }) if channel_id.as_str() == "C123"
                && message_ts.as_str() == "1700000000.000100"
                && author_id.as_ref().map(UserId::as_str) == Some("U456")
                && text.as_deref() == Some("preview")
                && url.is_some()
        ));
        assert!(matches!(
            &sec.elements[2],
            RichTextElement::Canvas(CanvasElement { file_id }) if file_id.as_str() == "F789"
        ));

        let round =
            serde_json::from_value::<RichTextContainer>(serde_json::to_value(&section).unwrap())
                .unwrap();
        assert_eq!(round, section);
    }

    #[test]
    fn text_style_omits_false_booleans_on_serialization() {
        assert_eq!(
            serde_json::to_value(TextStyle::default()).unwrap(),
            json!({})
        );

        let style = TextStyle {
            bold: true,
            code: true,
            underline: true,
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(style).unwrap(),
            json!({"bold": true, "code": true, "underline": true})
        );
    }
}
