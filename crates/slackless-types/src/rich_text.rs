//! Block Kit `blocks` and the `rich_text` block model the web client sends and receives.
//!
//! A user-typed message is one `rich_text` block whose `elements` are containers
//! (section/list/quote/preformatted) holding inline elements (text, link, mentions, emoji, ...).
//! Every level has an `Unknown` catch-all so a new element type never fails a message.

use serde::{Deserialize, Serialize};

use crate::de::null_default;
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
    }
}

/// A paragraph of inline elements.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RichTextSection {
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

fn is_false(b: &bool) -> bool {
    !*b
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
