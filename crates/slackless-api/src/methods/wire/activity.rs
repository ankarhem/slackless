//! Activity feed and Saved/Later wire payloads and parsers.

use serde::{Deserialize, Serialize};

use crate::de::{lenient_bool, lenient_u32, lenient_vec, or_default};
use crate::types::{ChannelId, Message, Ts, UserId};

/// `activity.feed`: the Activity view's items, newest first, with the read state Slack
/// tracks per entry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActivityFeed {
    pub items: Vec<ActivityItem>,
    /// Cursor of the next (older) page; `None` on the last page.
    pub next_cursor: Option<String>,
}

/// The feed categories slackless models; each maps onto a tab of the Activity view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ActivityKind {
    #[default]
    Unknown,
    /// A message that mentions the user: every mention flavour plus keyword highlights.
    Mention,
    /// A followed thread with activity.
    Thread,
    /// A reaction to the user's message.
    Reaction,
    /// A direct message.
    Dm,
    /// An app/bot DM (`bot_dm_bundle`).
    App,
    /// Activity in a channel the user is not joined to (`channel`).
    Channel,
}

impl ActivityKind {
    /// The `type` string Slack tags the payload with (the same string `activity.markRead`
    /// hands back; alias mention flavours collapse to `at_user`).
    pub fn type_name(self) -> &'static str {
        match self {
            ActivityKind::Unknown => "unknown",
            ActivityKind::Mention => "at_user",
            ActivityKind::Thread => "thread_v2",
            ActivityKind::Reaction => "message_reaction",
            ActivityKind::Dm => "dm",
            ActivityKind::App => "bot_dm_bundle",
            ActivityKind::Channel => "channel",
        }
    }
}

/// One `activity.feed` row, normalized from its per-type payload: every modeled type points
/// at a conversation message and says who the row is about, so the view renders one shape.
/// Entries of a type slackless does not model fail the tag parse, so `lenient_vec` skips
/// them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActivityItem {
    pub kind: ActivityKind,
    pub unread: bool,
    /// When the entry landed in the feed; with `key`, the `activity.markRead` handles.
    pub feed_ts: Ts,
    /// The entry's server id, e.g. `thread_v2-C0123-1784891984.803759`.
    pub key: String,
    /// The conversation the row opens.
    pub channel: ChannelId,
    /// The thread the row opens (thread activity, replies to the user's threaded message).
    pub thread_ts: Option<Ts>,
    /// The row's message: its author and its stamp.
    pub ts: Ts,
    /// Who the row is about: the mentioning author, the reactor. Thread rows name their
    /// latest replier through the hydrated body.
    pub actor: Option<UserId>,
    /// `message_reaction`: the reaction and who added it.
    pub reaction: Option<ActivityReaction>,
    /// The unread pill count (thread activity), 0 when the row only shows a dot.
    pub count: u32,
    /// The row's body when the feed or [`crate::SlackApi::messages_list`] carried it; the text to
    /// render, and the avatar/name source once present.
    pub message: Option<Message>,
}

/// A reaction on a `message_reaction` row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityReaction {
    /// Emoji shortcode, e.g. `eyes`.
    pub name: String,
    /// Who reacted.
    pub user: Option<UserId>,
}

impl ActivityItem {
    /// The entry `activity.markRead` / `activity.markUnread` needs for this row: the type
    /// string is the key's prefix (Slack embeds it: `thread_v2-C…-…`), which keeps alias
    /// mention flavours (`at_channel`, …) their own type.
    pub fn entry(&self) -> ActivityEntry {
        let kind = self
            .key
            .split_once('-')
            .map(|(kind, _)| kind.to_owned())
            .filter(|kind| !kind.is_empty())
            .unwrap_or_else(|| self.kind.type_name().to_owned());
        ActivityEntry {
            kind,
            key: self.key.clone(),
            feed_ts: self.feed_ts.clone(),
        }
    }
}

/// One `activity.markRead` / `activity.markUnread` entry: the `type` / `key` / `feed_ts`
/// handles every feed item carries.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActivityEntry {
    #[serde(rename = "type")]
    pub kind: String,
    pub key: String,
    pub feed_ts: Ts,
}

/// One `activity.feed` entry as Slack sends it: the read-state envelope around a payload
/// tagged with its `type`. Unknown tags fail the tag parse, so `lenient_vec` skips the
/// whole entry.
#[derive(Deserialize)]
pub(in crate::methods) struct ActivityItemWire {
    #[serde(rename = "is_unread", default, deserialize_with = "lenient_bool")]
    pub(in crate::methods) unread: bool,
    #[serde(default)]
    pub(in crate::methods) feed_ts: Ts,
    #[serde(default)]
    pub(in crate::methods) key: String,
    #[serde(default)]
    pub(in crate::methods) item: ActivityPayloadWire,
}

#[derive(Deserialize, Default)]
#[serde(tag = "type")]
pub(in crate::methods) enum ActivityPayloadWire {
    /// Every mention flavour the feed carries in one shape: `at_user`, `at_user_group`,
    /// `at_channel`, `at_everyone`, `keyword`, `unjoined_channel_mention`.
    #[serde(
        rename = "at_user",
        alias = "at_user_group",
        alias = "at_channel",
        alias = "at_everyone",
        alias = "keyword",
        alias = "unjoined_channel_mention"
    )]
    Mention {
        #[serde(default)]
        message: ActivityRefWire,
    },
    /// A followed thread's activity.
    #[serde(rename = "thread_v2")]
    Thread {
        #[serde(default)]
        bundle_info: ActivityThreadBundleWire,
    },
    /// Someone reacted to the user's message.
    #[serde(rename = "message_reaction")]
    Reaction {
        #[serde(default)]
        message: ActivityRefWire,
        #[serde(default)]
        reaction: ActivityReactionWire,
    },
    /// A DM (`bundle_info.payload.dm_entry`).
    #[serde(rename = "dm")]
    Dm {
        #[serde(default)]
        bundle_info: ActivityMessageBundleWire,
    },
    /// An app/bot DM (`bundle_info.payload.message`).
    #[serde(rename = "bot_dm_bundle")]
    App {
        #[serde(default)]
        bundle_info: ActivityMessageBundleWire,
    },
    /// Channel activity (`bundle_info.payload.channel_entry`).
    #[serde(rename = "channel")]
    Channel {
        #[serde(default)]
        bundle_info: ActivityMessageBundleWire,
    },
    /// The envelope's `item` was missing; never produced for a tag Slack sent.
    #[default]
    Unknown,
}

/// The message pointer a mention or reaction carries: ids, no body (those rows are hydrated
/// through `messages.list`).
#[derive(Deserialize, Default)]
pub(in crate::methods) struct ActivityRefWire {
    #[serde(default)]
    pub(in crate::methods) channel: ChannelId,
    #[serde(default)]
    pub(in crate::methods) ts: Ts,
    #[serde(rename = "author_user_id", default)]
    pub(in crate::methods) author: Option<UserId>,
    #[serde(default)]
    pub(in crate::methods) thread_ts: Option<Ts>,
}

/// `thread_v2`'s `bundle_info`: the unread pill plus the nested `payload.thread_entry`.
#[derive(Deserialize, Default)]
pub(in crate::methods) struct ActivityThreadBundleWire {
    #[serde(default, deserialize_with = "lenient_u32")]
    pub(in crate::methods) unread_count: u32,
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) payload: ActivityThreadPayloadWire,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct ActivityThreadPayloadWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) thread_entry: Option<ActivityThreadEntryWire>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct ActivityThreadEntryWire {
    #[serde(rename = "channel_id", default)]
    pub(in crate::methods) channel: ChannelId,
    #[serde(default)]
    pub(in crate::methods) thread_ts: Ts,
    #[serde(default)]
    pub(in crate::methods) latest_ts: Ts,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub(in crate::methods) unread_msg_count: u32,
}

/// `message_reaction`'s `reaction`.
#[derive(Deserialize, Default)]
pub(in crate::methods) struct ActivityReactionWire {
    #[serde(default)]
    pub(in crate::methods) name: String,
    #[serde(default)]
    pub(in crate::methods) user: Option<UserId>,
}

/// `dm` / `bot_dm_bundle` / `channel`'s `bundle_info`: the newest message under `payload`
/// (keyed by the payload's own entry name) plus the bundle's unread count.
#[derive(Deserialize, Default)]
pub(in crate::methods) struct ActivityMessageBundleWire {
    #[serde(default, deserialize_with = "lenient_u32")]
    pub(in crate::methods) unread_count: u32,
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) payload: ActivityPayloadEntryWire,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct ActivityPayloadEntryWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) dm_entry: Option<ActivityLatestWire>,
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) channel_entry: Option<ActivityLatestWire>,
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) message: Option<Message>,
}

/// An entry whose `latest_message` is a full message object (the channel id rides in its
/// `extra`, as with every `Message`).
#[derive(Deserialize, Default)]
pub(in crate::methods) struct ActivityLatestWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) latest_message: Option<Message>,
}

/// `activity.feed`'s envelope.
#[derive(Deserialize)]
pub(in crate::methods) struct ActivityFeedWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) items: Vec<ActivityItemWire>,
}

/// Item types the feed is asked for: every category slackless models, minus
/// `unjoined_channel_mention`, which this workspace rejects with `invalid_arguments`
/// (probed live; Slack's fuller bootstrap list adds only types the view skips anyway).
pub(in crate::methods) const ACTIVITY_TYPES: &str = concat!(
    "at_user,at_user_group,at_channel,at_everyone,keyword,",
    "thread_v2,message_reaction,dm,bot_dm_bundle,channel"
);

/// Normalizes one wire entry into the row the view renders; `None` for the shapes slackless
/// does not model (`Unknown`) or cannot place (no channel or message ts).
pub(in crate::methods) fn row(wire: ActivityItemWire) -> Option<ActivityItem> {
    let unread = wire.unread;
    let feed_ts = wire.feed_ts;
    let key = wire.key;
    let item = match wire.item {
        ActivityPayloadWire::Mention { message } => ActivityItem {
            kind: ActivityKind::Mention,
            unread,
            feed_ts,
            key,
            channel: message.channel.clone(),
            thread_ts: message.thread_ts.clone(),
            ts: message.ts.clone(),
            actor: message.author.clone(),
            reaction: None,
            count: 0,
            message: None,
        },
        ActivityPayloadWire::Thread { bundle_info } => {
            let entry = bundle_info.payload.thread_entry?;
            ActivityItem {
                kind: ActivityKind::Thread,
                unread,
                feed_ts,
                key,
                channel: entry.channel.clone(),
                thread_ts: Some(entry.thread_ts.clone()),
                ts: entry.latest_ts.clone(),
                actor: None,
                reaction: None,
                count: bundle_info.unread_count.max(entry.unread_msg_count),
                message: None,
            }
        }
        ActivityPayloadWire::Reaction { message, reaction } => ActivityItem {
            kind: ActivityKind::Reaction,
            unread,
            feed_ts,
            key,
            channel: message.channel.clone(),
            thread_ts: message.thread_ts.clone(),
            ts: message.ts.clone(),
            actor: reaction.user.clone(),
            reaction: Some(ActivityReaction {
                name: reaction.name,
                user: reaction.user,
            }),
            count: 0,
            message: None,
        },
        ActivityPayloadWire::Dm { bundle_info } => {
            let latest = bundle_info
                .payload
                .dm_entry
                .and_then(|entry| entry.latest_message);
            let count = bundle_info.unread_count;
            message_row(ActivityKind::Dm, unread, feed_ts, key, latest, count)?
        }
        ActivityPayloadWire::App { bundle_info } => {
            let count = bundle_info.unread_count;
            message_row(
                ActivityKind::App,
                unread,
                feed_ts,
                key,
                bundle_info.payload.message,
                count,
            )?
        }
        ActivityPayloadWire::Channel { bundle_info } => {
            let latest = bundle_info
                .payload
                .channel_entry
                .and_then(|entry| entry.latest_message);
            let count = bundle_info.unread_count;
            message_row(ActivityKind::Channel, unread, feed_ts, key, latest, count)?
        }
        ActivityPayloadWire::Unknown => return None,
    };
    (item.channel.as_str() != "" && item.ts.as_str() != "").then_some(item)
}

/// A `dm` / `bot_dm_bundle` / `channel` row: its entry points at the newest message, which
/// the feed sends as a bare ref (channel + ts) — the body arrives through `messages.list`
/// hydration, so `message` is only set once that lands (or immediately, when Slack someday
/// sends the body inline).
pub(in crate::methods) fn message_row(
    kind: ActivityKind,
    unread: bool,
    feed_ts: Ts,
    key: String,
    message: Option<Message>,
    count: u32,
) -> Option<ActivityItem> {
    let message = message?;
    let mut item = ActivityItem {
        kind,
        unread,
        feed_ts,
        key,
        channel: message.channel.clone().unwrap_or_default(),
        thread_ts: None,
        ts: message.ts.clone(),
        actor: message.user.clone(),
        reaction: None,
        count,
        message: None,
    };
    let has_body = !message.text.is_empty() || !message.blocks.is_empty();
    if has_body {
        item.message = Some(message);
    }
    Some(item)
}

/// Which tab of Slack's Later / Saved view an item belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavedItemState {
    InProgress,
    Archived,
    Completed,
}

/// State transition sent to `saved.update`'s `mark` parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavedMark {
    Completed,
    Uncompleted,
    Archived,
    Unarchived,
}

impl SavedMark {
    pub fn as_str(self) -> &'static str {
        match self {
            SavedMark::Completed => "completed",
            SavedMark::Uncompleted => "uncompleted",
            SavedMark::Archived => "archived",
            SavedMark::Unarchived => "unarchived",
        }
    }
}

/// One saved message returned by `saved.list` (and hydrated via `messages.list`).
#[derive(Debug, Clone, PartialEq)]
pub struct SavedItem {
    /// Channel ID (`item_id` on the wire).
    pub channel: ChannelId,
    /// Message timestamp (`ts`).
    pub ts: Ts,
    pub state: SavedItemState,
    /// Reminder due time in unix seconds (`None` when unset / `0`).
    pub date_due: Option<u64>,
    /// Creation timestamp in unix seconds.
    pub date_created: Option<u64>,
    /// Message body once hydrated via `messages.list`.
    pub message: Option<Message>,
}

#[derive(Deserialize)]
pub(in crate::methods) struct WireSavedItem {
    #[serde(default)]
    pub(in crate::methods) item_id: String,
    #[serde(default)]
    pub(in crate::methods) item_type: String,
    #[serde(default)]
    pub(in crate::methods) ts: String,
    #[serde(default)]
    pub(in crate::methods) date_created: u64,
    #[serde(default)]
    pub(in crate::methods) date_due: u64,
    #[serde(default)]
    pub(in crate::methods) date_completed: u64,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub(in crate::methods) is_archived: bool,
    #[serde(default)]
    pub(in crate::methods) state: String,
    #[serde(default)]
    pub(in crate::methods) todo_state: String,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct SavedListWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) saved_items: Vec<WireSavedItem>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) items: Vec<WireSavedItem>,
}

pub(in crate::methods) fn parse_saved_list(wire: SavedListWire, filter: &str) -> Vec<SavedItem> {
    let raw = if !wire.saved_items.is_empty() {
        wire.saved_items
    } else {
        wire.items
    };
    raw.into_iter()
        .filter(|item| {
            (item.item_type.is_empty() || item.item_type == "message")
                && !item.item_id.is_empty()
                && !item.ts.is_empty()
        })
        .map(|item| {
            let state = if item.is_archived || item.state == "archived" || filter == "archived" {
                SavedItemState::Archived
            } else if item.date_completed > 0
                || item.todo_state == "completed"
                || item.state == "completed"
                || filter == "completed"
            {
                SavedItemState::Completed
            } else {
                SavedItemState::InProgress
            };
            SavedItem {
                channel: ChannelId::from(item.item_id.as_str()),
                ts: Ts::new(item.ts),
                state,
                date_due: (item.date_due > 0).then_some(item.date_due),
                date_created: (item.date_created > 0).then_some(item.date_created),
                message: None,
            }
        })
        .collect()
}
