//! Messaging, history options, scheduled messages, threads, cloud drafts, and GIF picker payloads.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::Result;
use crate::client::Params;
use crate::de::{
    lenient_bool, lenient_u32, lenient_u64, lenient_vec, null_default, or_default, value_as_u64,
};
use crate::types::{Attachment, Block, ChannelId, Message, RichTextBlock, Ts};

/// Window and page size for `conversations.history` / `conversations.replies`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryOpts {
    /// Only messages after this ts.
    pub oldest: Option<Ts>,
    /// Only messages before this ts.
    pub latest: Option<Ts>,
    /// Include messages exactly at `oldest`/`latest`.
    pub inclusive: bool,
    /// Page size (Slack max 999, default 100).
    pub limit: Option<u32>,
}

impl HistoryOpts {
    pub(in crate::methods) fn push(&self, params: &mut Params) {
        if let Some(oldest) = &self.oldest {
            params.push(("oldest", oldest.to_string()));
        }
        if let Some(latest) = &self.latest {
            params.push(("latest", latest.to_string()));
        }
        if self.inclusive {
            params.push(("inclusive", "true".to_owned()));
        }
        if let Some(limit) = self.limit {
            params.push(("limit", limit.to_string()));
        }
    }
}

/// Body of a message to send or edit: fallback `text`, Block Kit `blocks`, and optional
/// `attachments` (such as a GIF picked from the composer's GIFs tab).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MessageContent {
    /// Plain-text fallback (notifications, search, old clients).
    pub text: String,
    /// Usually one `rich_text` block, as the web client sends.
    pub blocks: Vec<Block>,
    /// Optional attachments (e.g. Slack's block-only GIF image attachment).
    pub attachments: Vec<Attachment>,
}

impl MessageContent {
    /// A plain message encoded the way the web client does: one `rich_text` block holding one
    /// section with one text run.
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            blocks: vec![RichTextBlock::plain_text(text.clone()).into()],
            text,
            attachments: Vec::new(),
        }
    }

    pub(in crate::methods) fn push(&self, params: &mut Params) -> Result<()> {
        params.push(("text", self.text.clone()));
        if !self.blocks.is_empty() {
            params.push(("blocks", serde_json::to_string(&self.blocks)?));
        }
        if !self.attachments.is_empty() {
            params.push(("attachments", serde_json::to_string(&self.attachments)?));
        }
        Ok(())
    }
}

/// `chat.postMessage` / `chat.update` result.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct PostedMessage {
    #[serde(default)]
    pub channel: ChannelId,
    /// Server-assigned ts: the message's permanent id.
    #[serde(default)]
    pub ts: Ts,
    #[serde(default, deserialize_with = "or_default")]
    pub message: Option<Message>,
    /// The `client_msg_id` this client generated for a post (dedup / optimistic reconcile).
    #[serde(skip)]
    pub client_msg_id: Option<String>,
}

/// Response to `chat.scheduleMessage` (or `drafts.create` with `date_scheduled`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ScheduledMessageCreated {
    pub scheduled_message_id: String,
    pub channel: ChannelId,
    #[serde(default, deserialize_with = "lenient_u64")]
    pub post_at: u64,
}

/// One pending scheduled message returned by `chat.scheduledMessages.list` or `drafts.list`
/// (`date_scheduled > 0`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ScheduledMessage {
    pub id: String,
    pub channel_id: ChannelId,
    #[serde(default, deserialize_with = "lenient_u64")]
    pub post_at: u64,
    #[serde(default, deserialize_with = "lenient_u64")]
    pub date_created: u64,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub(in crate::methods) struct ScheduledMessagesList {
    #[serde(default)]
    pub(in crate::methods) scheduled_messages: Vec<ScheduledMessage>,
}

/// `subscriptions.thread.getView`: the Threads view's followed threads, newest activity
/// first.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ThreadsView {
    #[serde(default, deserialize_with = "lenient_u32")]
    pub total_unread_replies: u32,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub has_more: bool,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub threads: Vec<ThreadSubscription>,
}

/// One thread of [`ThreadsView`]: the channel-level root plus its latest and unread replies.
/// The root's `extra` carries the view-only fields (`last_read`, `subscribed`, ...); a thread
/// whose root did not parse keeps the default root (empty `ts`) and is skipped by callers.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ThreadSubscription {
    #[serde(rename = "root_msg", default, deserialize_with = "null_default")]
    pub root: Message,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub latest_replies: Vec<Message>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub unread_replies: Vec<Message>,
}

#[derive(Deserialize)]
pub(in crate::methods) struct MessagesWire {
    #[serde(default, deserialize_with = "null_default")]
    pub(in crate::methods) messages: Vec<Message>,
}

/// `messages.list`: the bodies of the asked-for messages, by channel.
#[derive(Deserialize, Default)]
pub(in crate::methods) struct MessagesListWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) messages_data: HashMap<String, MessagesChannelWire>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct MessagesChannelWire {
    #[serde(default, deserialize_with = "null_default")]
    pub(in crate::methods) messages: Vec<Message>,
}

/// Channels per `messages.list` call before Slack answers `too_many_channels`
/// (docs/research/api-activity.md, `messages.list`).
pub(in crate::methods) const MESSAGES_LIST_CHANNELS: usize = 8;

/// One GIF item returned by Slack's `gif.featured` / `gif.search` picker endpoints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GifItem {
    pub id: String,
    pub title: String,
    /// Single-frame 200w still thumbnail (`media_formats.tinygifpreview.url`) for cheap grid rendering.
    pub still_url: String,
    /// Animated 200w thumbnail URL (`media_formats.tinygif.url`) played on hover and sent as the
    /// `image` block's `image_url`, matching official Slack's `chat.postMessage` payload.
    pub preview_url: String,
    /// Shareable Giphy URL (`url`), falling back to full GIF media URL.
    pub url: String,
}

impl GifItem {
    /// Build the exact Block Kit `image` attachment official Slack sends via `chat.postMessage`
    /// when posting a GIF selected from the composer's GIFs tab.
    pub fn to_attachment(&self) -> Attachment {
        let alt_text = if self.title.trim().is_empty() {
            "GIF"
        } else {
            self.title.as_str()
        };
        let block = Block::from_value(serde_json::json!({
            "type": "image",
            "image_url": self.preview_url,
            "alt_text": alt_text,
            "title": {
                "type": "plain_text",
                "text": "GIF",
                "emoji": true
            }
        }));
        Attachment {
            fallback: Some("shared a GIF".to_owned()),
            blocks: vec![block],
            ..Default::default()
        }
    }
}

#[derive(Deserialize)]
pub(in crate::methods) struct GifMediaFormat {
    #[serde(default)]
    pub(in crate::methods) url: String,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct GifMediaFormats {
    pub(in crate::methods) gif: Option<GifMediaFormat>,
    pub(in crate::methods) tinygif: Option<GifMediaFormat>,
    pub(in crate::methods) tinygifpreview: Option<GifMediaFormat>,
    pub(in crate::methods) gifpreview: Option<GifMediaFormat>,
}

#[derive(Deserialize)]
pub(in crate::methods) struct WireGif {
    #[serde(default)]
    pub(in crate::methods) id: String,
    #[serde(default)]
    pub(in crate::methods) content_description: String,
    #[serde(default)]
    pub(in crate::methods) url: String,
    #[serde(default)]
    pub(in crate::methods) media_formats: GifMediaFormats,
}

#[derive(Deserialize)]
pub(in crate::methods) struct GifListData {
    #[serde(default)]
    pub(in crate::methods) gifs: Vec<WireGif>,
}

pub(in crate::methods) fn parse_gif_list(data: GifListData) -> Vec<GifItem> {
    data.gifs
        .into_iter()
        .filter_map(|gif| {
            let preview_url = [
                gif.media_formats.tinygif.as_ref().map(|m| m.url.as_str()),
                gif.media_formats
                    .tinygifpreview
                    .as_ref()
                    .map(|m| m.url.as_str()),
                gif.media_formats.gif.as_ref().map(|m| m.url.as_str()),
                gif.media_formats
                    .gifpreview
                    .as_ref()
                    .map(|m| m.url.as_str()),
            ]
            .into_iter()
            .flatten()
            .find(|u| !u.is_empty())?
            .to_owned();
            let still_url = [
                gif.media_formats
                    .tinygifpreview
                    .as_ref()
                    .map(|m| m.url.as_str()),
                gif.media_formats
                    .gifpreview
                    .as_ref()
                    .map(|m| m.url.as_str()),
            ]
            .into_iter()
            .flatten()
            .find(|u| !u.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| preview_url.clone());
            let url = if !gif.url.is_empty() {
                gif.url
            } else {
                gif.media_formats
                    .gif
                    .map(|m| m.url)
                    .filter(|u| !u.is_empty())
                    .unwrap_or_else(|| preview_url.clone())
            };
            Some(GifItem {
                id: gif.id,
                title: gif.content_description,
                still_url,
                preview_url,
                url,
            })
        })
        .collect()
}

/// One cloud-synced draft returned by `drafts.list` (`F-CORE-089`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudDraft {
    pub id: String,
    pub channel: ChannelId,
    pub text: String,
    pub updated_secs: Option<u64>,
}

fn draft_extract_text(blocks: &Value) -> String {
    let mut out = String::new();
    for block in blocks.as_array().into_iter().flatten() {
        for container in block["elements"].as_array().into_iter().flatten() {
            for el in container["elements"].as_array().into_iter().flatten() {
                if let Some(t) = el["text"].as_str() {
                    out.push_str(t);
                }
            }
        }
    }
    out
}

/// Pad a Slack draft timestamp (`last_updated_ts` / `date_last_updated`) to 7 fractional digits
/// as required by `drafts.update` and `drafts.delete` (`1700000000.1` -> `1700000000.1000000`).
pub(in crate::methods) fn pad_draft_ts(ts: &str) -> String {
    let trimmed = ts.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    match trimmed.split_once('.') {
        Some((whole, frac)) => {
            let mut f: String = frac.chars().filter(char::is_ascii_digit).take(7).collect();
            while f.len() < 7 {
                f.push('0');
            }
            format!("{whole}.{f}")
        }
        None => format!("{trimmed}.0000000"),
    }
}

pub(in crate::methods) fn parse_cloud_drafts(raw: &Value) -> Vec<CloudDraft> {
    raw["drafts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            if d["is_deleted"].as_bool().unwrap_or(false) || d["is_sent"].as_bool().unwrap_or(false)
            {
                return None;
            }
            // Scheduled drafts (`date_scheduled > 0`) belong in the Scheduled tab, not Drafts.
            if value_as_u64(&d["date_scheduled"]).unwrap_or(0) > 0 {
                return None;
            }
            let id = d["id"].as_str()?.to_owned();
            let channel = d["destinations"]
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|dest| dest["channel_id"].as_str())
                .map(ChannelId::from)?;
            let text = draft_extract_text(&d["blocks"]);
            if text.trim().is_empty() {
                return None;
            }
            let updated_secs = value_as_u64(&d["date_last_updated"])
                .or_else(|| value_as_u64(&d["last_updated_ts"]))
                .or_else(|| value_as_u64(&d["date_created"]));
            Some(CloudDraft {
                id,
                channel,
                text,
                updated_secs,
            })
        })
        .collect()
}

/// Extract pending scheduled messages (`date_scheduled > 0`) from a `drafts.list` response.
pub(in crate::methods) fn parse_scheduled_drafts(
    raw: &Value,
    channel: Option<&ChannelId>,
) -> Vec<ScheduledMessage> {
    raw["drafts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            if d["is_deleted"].as_bool().unwrap_or(false) || d["is_sent"].as_bool().unwrap_or(false)
            {
                return None;
            }
            let post_at = value_as_u64(&d["date_scheduled"]).filter(|&ts| ts > 0)?;
            let id = d["id"].as_str()?.to_owned();
            let channel_id = d["destinations"]
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|dest| dest["channel_id"].as_str())
                .map(ChannelId::from)?;
            if let Some(filter) = channel
                && &channel_id != filter
            {
                return None;
            }
            let date_created = value_as_u64(&d["date_created"]).unwrap_or(0);
            let text = draft_extract_text(&d["blocks"]);
            Some(ScheduledMessage {
                id,
                channel_id,
                post_at,
                date_created,
                text,
            })
        })
        .collect()
}

/// Look up a draft's 7-decimal padded `client_last_updated_ts` from a `drafts.list` payload.
pub(in crate::methods) fn find_draft_last_updated_ts(
    raw: &Value,
    draft_id: &str,
) -> Option<String> {
    raw["drafts"].as_array()?.iter().find_map(|d| {
        if d["id"].as_str() != Some(draft_id) {
            return None;
        }
        let raw_ts = d["last_updated_ts"]
            .as_str()
            .map(ToOwned::to_owned)
            .or_else(|| d["last_updated_ts"].as_f64().map(|f| format!("{f:.7}")))
            .or_else(|| d["date_last_updated"].as_u64().map(|n| n.to_string()))
            .or_else(|| d["date_last_updated"].as_str().map(ToOwned::to_owned))?;
        let padded = pad_draft_ts(&raw_ts);
        (!padded.is_empty()).then_some(padded)
    })
}
