//! Typed endpoint methods. Method-name strings come from [`crate::types::endpoints::method`];
//! request params are built inline per method.
//!
//! Response structs model only what slackless uses; every field is defaulted, unknown keys are
//! kept in `extra` (or ignored), and lists use lenient element parsing.

use std::collections::HashMap;

use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::client::Params;
use crate::de::{lenient_bool, lenient_u32, lenient_vec, null_default, or_default};
use crate::pagination::{Page, paginate};
use crate::types::endpoints::method;
use crate::types::{
    ApiResponse, Attachment, Block, BotId, BotProfile, ChannelId, ChannelSection, Conversation,
    ConversationKind, DndStatus, Empty, File, Latest, Message, RichTextBlock, SectionKind, Team,
    TeamId, Ts, User, UserId, UserPresence,
};
use crate::{Result, SlackApi};

/// `_x_reason` the web client tags message sends with.
const REASON_MESSAGE_SEND: &str = "webapp_message_send";
/// `_x_reason` for reaction toggles.
const REASON_REACTION: &str = "changeReactionFromUserAction";
/// `_x_mode` the web client sends on writes.
const MODE_ONLINE: &str = "online";
/// `_x_reason` for slash commands Slack runs.
const REASON_COMMAND: &str = "executeCommand";
/// `_x_reason` for Block Kit interactions.
const REASON_BLOCK_ACTION: &str = "dispatch_action_to_developer";

/// The web client's per-request `client_token`: `web-<unix ms>`.
fn client_token() -> String {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("web-{}", since_epoch.as_millis())
}

/// Where a Block Kit action happened, as `blocks.actions`'s `container` names it.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionContainer {
    pub channel: ChannelId,
    pub message_ts: Ts,
    /// The legacy attachment holding the block (Giphy's preview), by its `id`.
    pub attachment_id: Option<i64>,
    pub is_ephemeral: bool,
}

impl ActionContainer {
    fn json(&self) -> Value {
        let (channel, ts) = (self.channel.as_str(), self.message_ts.as_str());
        match self.attachment_id {
            Some(id) => serde_json::json!({
                "type": "message_attachment", "message_ts": ts, "attachment_id": id,
                "channel_id": channel, "is_ephemeral": self.is_ephemeral,
            }),
            None => serde_json::json!({
                "type": "message", "message_ts": ts, "channel_id": channel,
                "is_ephemeral": self.is_ephemeral,
            }),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Request options
// ---------------------------------------------------------------------------------------------

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
    fn push(&self, params: &mut Params) {
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

/// Filters for `conversations.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListOpts {
    /// Kinds to list; empty means Slack's default (public channels).
    pub kinds: Vec<ConversationKind>,
    pub exclude_archived: bool,
    /// Page size (Slack max 999, default 100).
    pub limit: Option<u32>,
}

/// Filters for `files.list` (`FilesFilter::channel` / `FilesFilter::workspace` plus the
/// optional narrowings).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FilesFilter {
    /// The conversation whose shared files to list (the Files & links tab); `None` asks
    /// for every file the signed-in user can see workspace-wide (the Files view).
    pub channel: Option<ChannelId>,
    /// Only files uploaded by this person.
    pub user: Option<UserId>,
    /// Comma-delimited file kinds: Slack documents `images`, `pdfs`, `snippets`, `spaces`
    /// (legacy Posts) and more; `canvas` (canvases) is observed but undocumented.
    pub types: Option<String>,
}

impl FilesFilter {
    /// The files shared with one conversation.
    pub fn channel(channel: ChannelId) -> Self {
        Self {
            channel: Some(channel),
            ..Default::default()
        }
    }

    /// Every file the signed-in user can see in the workspace.
    pub fn workspace() -> Self {
        Self::default()
    }
}

/// `sort` for `search.messages`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchSort {
    #[default]
    Score,
    Timestamp,
}

/// Options for `search.messages`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchOpts {
    /// Results per page (max 100).
    pub count: Option<u32>,
    /// 1-based page (max 100). Ignored by Slack when `cursor` is set.
    pub page: Option<u32>,
    /// `*` for the first cursor page, then `pagination.next_cursor`.
    pub cursor: Option<String>,
    pub sort: SearchSort,
    /// Ascending instead of descending.
    pub ascending: bool,
    /// Wrap hits in U+E000 / U+E001.
    pub highlight: bool,
}

/// Body of a message to send or edit: fallback `text` plus Block Kit `blocks`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MessageContent {
    /// Plain-text fallback (notifications, search, old clients).
    pub text: String,
    /// Usually one `rich_text` block, as the web client sends.
    pub blocks: Vec<Block>,
}

impl MessageContent {
    /// A plain message encoded the way the web client does: one `rich_text` block holding one
    /// section with one text run.
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            blocks: vec![RichTextBlock::plain_text(text.clone()).into()],
            text,
        }
    }

    fn push(&self, params: &mut Params) -> Result<()> {
        params.push(("text", self.text.clone()));
        if !self.blocks.is_empty() {
            params.push(("blocks", serde_json::to_string(&self.blocks)?));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------------------------

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
    #[serde(default, deserialize_with = "lenient_bool")]
    pub has_unreads: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `client.counts` `threads`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ThreadCounts {
    #[serde(default, deserialize_with = "lenient_bool")]
    pub has_unreads: bool,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub mention_count: u32,
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

/// `search.messages` `messages` object.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct SearchMessages {
    #[serde(default, deserialize_with = "lenient_u32")]
    pub total: u32,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub matches: Vec<SearchMatch>,
    #[serde(default, deserialize_with = "or_default")]
    pub pagination: Option<SearchPagination>,
}

/// One `search.messages` hit. `channel` is an object here, unlike in [`Message`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct SearchMatch {
    #[serde(default, deserialize_with = "or_default")]
    pub channel: SearchChannel,
    #[serde(default)]
    pub ts: Ts,
    #[serde(default, deserialize_with = "or_default")]
    pub text: String,
    #[serde(default, deserialize_with = "or_default")]
    pub user: Option<UserId>,
    #[serde(default, deserialize_with = "or_default")]
    pub username: Option<String>,
    #[serde(default, deserialize_with = "or_default")]
    pub team: Option<TeamId>,
    #[serde(default, deserialize_with = "or_default")]
    pub permalink: Option<String>,
    #[serde(default, deserialize_with = "or_default")]
    pub iid: Option<String>,
    /// `message`, `im`, `group`, ...
    #[serde(rename = "type", default, deserialize_with = "or_default")]
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "or_default")]
    pub blocks: Vec<Block>,
    #[serde(default, deserialize_with = "or_default")]
    pub attachments: Vec<Attachment>,
    #[serde(default, deserialize_with = "or_default")]
    pub files: Vec<File>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `search.messages` match `channel`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct SearchChannel {
    #[serde(default)]
    pub id: ChannelId,
    /// For IM hits, the other user's id.
    #[serde(default, deserialize_with = "or_default")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub is_private: bool,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub is_mpim: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `search.messages` `pagination`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct SearchPagination {
    #[serde(default, deserialize_with = "lenient_u32")]
    pub page: u32,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub page_count: u32,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub per_page: u32,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub total_count: u32,
    /// Empty/absent on the last cursor page.
    #[serde(default, deserialize_with = "or_default")]
    pub next_cursor: Option<String>,
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

/// `conversations.canvases.create`: the id of the created channel canvas.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CanvasCreated {
    #[serde(default, deserialize_with = "or_default")]
    pub canvas_id: String,
}

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
    /// The row's body when the feed or [`SlackApi::messages_list`] carried it; the text to
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

// Wire wrappers for payloads that nest the useful value under one key.

#[derive(Deserialize)]
struct MessagesWire {
    #[serde(default, deserialize_with = "null_default")]
    messages: Vec<Message>,
}

#[derive(Deserialize)]
struct ChannelWire {
    channel: Conversation,
}

#[derive(Deserialize)]
struct TeamWire {
    team: Team,
}

#[derive(Deserialize)]
struct ChannelsWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    channels: Vec<Conversation>,
}

#[derive(Deserialize)]
struct UserWire {
    user: User,
}

#[derive(Deserialize)]
struct MembersWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    members: Vec<User>,
}

/// `conversations.members` returns bare member ids.
#[derive(Deserialize)]
struct MemberIdsWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    members: Vec<UserId>,
}

/// `admin.roles.entity.listAssignments`: who holds which role on one entity.
#[derive(Deserialize)]
struct RoleAssignmentsWire {
    #[serde(default)]
    role_assignments: Vec<RoleAssignmentWire>,
}

#[derive(Deserialize)]
struct RoleAssignmentWire {
    role_id: String,
    #[serde(default)]
    users: Vec<UserId>,
}

/// Slack's channel-manager role, as its client reads it from
/// `admin.roles.entity.listAssignments`.
const CHANNEL_MANAGER_ROLE: &str = "Rl0A";

/// `client.dms` answers one DMs-tab row per conversation: the `channel` object with the
/// row's bare `latest` duplicated beside it.
#[derive(Deserialize)]
struct DmWire {
    #[serde(default)]
    id: ChannelId,
    #[serde(default, deserialize_with = "or_default")]
    latest: Option<Ts>,
    #[serde(default, deserialize_with = "or_default")]
    channel: Option<Conversation>,
}

impl DmWire {
    /// The row's conversation: the nested object when Slack sent one, else the bare minimum
    /// (`id` + `latest`) the row carries on its own.
    fn into_conversation(self) -> Conversation {
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
struct DmsWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    ims: Vec<DmWire>,
    #[serde(default, deserialize_with = "lenient_vec")]
    mpims: Vec<DmWire>,
}

#[derive(Deserialize)]
struct SearchWire {
    #[serde(default, deserialize_with = "or_default")]
    messages: SearchMessages,
}

#[derive(Deserialize)]
struct EmojiWire {
    #[serde(default, deserialize_with = "or_default")]
    emoji: HashMap<String, String>,
}

#[derive(Deserialize)]
struct ChannelSectionsWire {
    /// Pagination cursor, top-level on this method; empty string on the last page.
    #[serde(default, deserialize_with = "or_default")]
    cursor: Option<String>,
    #[serde(default, deserialize_with = "lenient_vec")]
    channel_sections: Vec<SectionWire>,
}

#[derive(Deserialize)]
struct SectionWire {
    #[serde(default, deserialize_with = "or_default")]
    channel_section_id: String,
    #[serde(default, deserialize_with = "or_default")]
    name: String,
    #[serde(rename = "type", default, deserialize_with = "or_default")]
    kind: Option<SectionKind>,
    #[serde(default, deserialize_with = "or_default")]
    emoji: String,
    /// The next section in display order; empty string when this is the last one.
    #[serde(default, deserialize_with = "or_default")]
    next_channel_section_id: String,
    #[serde(default, deserialize_with = "or_default")]
    channel_ids_page: IdsPageWire,
}

#[derive(Default, Deserialize)]
struct IdsPageWire {
    #[serde(default, deserialize_with = "or_default")]
    channel_ids: Vec<ChannelId>,
}

/// One `activity.feed` entry as Slack sends it: the read-state envelope around a payload
/// tagged with its `type`. Unknown tags fail the tag parse, so `lenient_vec` skips the
/// whole entry.
#[derive(Deserialize)]
struct ActivityItemWire {
    #[serde(rename = "is_unread", default, deserialize_with = "lenient_bool")]
    unread: bool,
    #[serde(default)]
    feed_ts: Ts,
    #[serde(default)]
    key: String,
    #[serde(default)]
    item: ActivityPayloadWire,
}

#[derive(Deserialize, Default)]
#[serde(tag = "type")]
enum ActivityPayloadWire {
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
struct ActivityRefWire {
    #[serde(default)]
    channel: ChannelId,
    #[serde(default)]
    ts: Ts,
    #[serde(rename = "author_user_id", default)]
    author: Option<UserId>,
    #[serde(default)]
    thread_ts: Option<Ts>,
}

/// `thread_v2`'s `bundle_info`: the unread pill plus the nested `payload.thread_entry`.
#[derive(Deserialize, Default)]
struct ActivityThreadBundleWire {
    #[serde(default, deserialize_with = "lenient_u32")]
    unread_count: u32,
    #[serde(default, deserialize_with = "or_default")]
    payload: ActivityThreadPayloadWire,
}

#[derive(Deserialize, Default)]
struct ActivityThreadPayloadWire {
    #[serde(default, deserialize_with = "or_default")]
    thread_entry: Option<ActivityThreadEntryWire>,
}

#[derive(Deserialize, Default)]
struct ActivityThreadEntryWire {
    #[serde(rename = "channel_id", default)]
    channel: ChannelId,
    #[serde(default)]
    thread_ts: Ts,
    #[serde(default)]
    latest_ts: Ts,
    #[serde(default, deserialize_with = "lenient_u32")]
    unread_msg_count: u32,
}

/// `message_reaction`'s `reaction`.
#[derive(Deserialize, Default)]
struct ActivityReactionWire {
    #[serde(default)]
    name: String,
    #[serde(default)]
    user: Option<UserId>,
}

/// `dm` / `bot_dm_bundle` / `channel`'s `bundle_info`: the newest message under `payload`
/// (keyed by the payload's own entry name) plus the bundle's unread count.
#[derive(Deserialize, Default)]
struct ActivityMessageBundleWire {
    #[serde(default, deserialize_with = "lenient_u32")]
    unread_count: u32,
    #[serde(default, deserialize_with = "or_default")]
    payload: ActivityPayloadEntryWire,
}

#[derive(Deserialize, Default)]
struct ActivityPayloadEntryWire {
    #[serde(default, deserialize_with = "or_default")]
    dm_entry: Option<ActivityLatestWire>,
    #[serde(default, deserialize_with = "or_default")]
    channel_entry: Option<ActivityLatestWire>,
    #[serde(default, deserialize_with = "or_default")]
    message: Option<Message>,
}

/// An entry whose `latest_message` is a full message object (the channel id rides in its
/// `extra`, as with every `Message`).
#[derive(Deserialize, Default)]
struct ActivityLatestWire {
    #[serde(default, deserialize_with = "or_default")]
    latest_message: Option<Message>,
}

/// `messages.list`: the bodies of the asked-for messages, by channel.
#[derive(Deserialize, Default)]
struct MessagesListWire {
    #[serde(default, deserialize_with = "or_default")]
    messages_data: HashMap<String, MessagesChannelWire>,
}

#[derive(Deserialize, Default)]
struct MessagesChannelWire {
    #[serde(default, deserialize_with = "null_default")]
    messages: Vec<Message>,
}

#[derive(Deserialize)]
struct FilesWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    files: Vec<File>,
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
fn merge_section(sections: &mut Vec<ChannelSection>, section: ChannelSection) {
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

fn page<W, T>(resp: ApiResponse<W>, items: impl FnOnce(W) -> Vec<T>) -> Page<T> {
    let next_cursor = resp.next_cursor().map(str::to_owned);
    Page {
        items: items(resp.data),
        next_cursor,
    }
}

fn kind_wire(kind: ConversationKind) -> &'static str {
    match kind {
        ConversationKind::PublicChannel => "public_channel",
        ConversationKind::PrivateChannel => "private_channel",
        ConversationKind::Im => "im",
        ConversationKind::Mpim => "mpim",
    }
}

/// The shared `types`/`exclude_archived`/`limit`/`cursor` params of the conversation listing
/// methods (`conversations.list`, `users.conversations`).
fn listing_params(opts: &ListOpts, cursor: Option<String>) -> Params {
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

// ---------------------------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------------------------

impl SlackApi {
    /// `auth.test`: validates the session and says who/where we are.
    pub async fn auth_test(&self) -> Result<AuthTest> {
        Ok(self.call(method::AUTH_TEST, Vec::new()).await?.data)
    }

    /// `client.userBoot` (cold boot): self, team, channels, IMs, priorities, prefs.
    pub async fn client_user_boot(&self) -> Result<UserBoot> {
        let params = vec![
            ("version_all_channels", "false".to_owned()),
            // The official boot asks for every MPDM relevant to the sidebar; without it the
            // boot channel list is capped and newer group DMs never arrive
            // (docs/research/api-channels.md, "The boot channel list is not a directory").
            ("return_all_relevant_mpdms", "true".to_owned()),
            ("omit_channels", "false".to_owned()),
            ("include_min_version_bump_check", "1".to_owned()),
            ("_x_reason", "initial-data".to_owned()),
            ("_x_sonic", "true".to_owned()),
        ];
        Ok(self.call(method::CLIENT_USER_BOOT, params).await?.data)
    }

    /// `client.counts`: unread and mention counts for every open conversation and threads.
    pub async fn client_counts(&self) -> Result<ClientCounts> {
        let params = vec![
            ("thread_counts_by_channel", "true".to_owned()),
            // As the official client asks: without it Slack leaves some joined conversations
            // (e.g. Slack Connect channels) out, and the sidebar cannot list them.
            ("include_all_unreads", "true".to_owned()),
            ("org_wide_aware", "true".to_owned()),
            ("include_file_channels", "true".to_owned()),
            (
                "_x_reason",
                "client-counts-api/fetchClientCounts".to_owned(),
            ),
        ];
        Ok(self.call(method::CLIENT_COUNTS, params).await?.data)
    }

    /// `conversations.view`: channel metadata plus the newest `count` messages in one call.
    pub async fn conversations_view(
        &self,
        channel: &ChannelId,
        count: Option<u32>,
    ) -> Result<ConversationView> {
        let mut params = vec![
            ("channel", channel.to_string()),
            ("canonical_avatars", "true".to_owned()),
        ];
        if let Some(count) = count {
            params.push(("count", count.to_string()));
        }
        Ok(self.call(method::CONVERSATIONS_VIEW, params).await?.data)
    }

    /// One page of `conversations.history`, newest first. Pass the previous page's
    /// `next_cursor` to continue; see [`paginate`] to walk them all.
    pub async fn conversations_history(
        &self,
        channel: &ChannelId,
        opts: &HistoryOpts,
        cursor: Option<String>,
    ) -> Result<Page<Message>> {
        let mut params = vec![("channel", channel.to_string())];
        opts.push(&mut params);
        if let Some(cursor) = cursor {
            params.push(("cursor", cursor));
        }
        let resp = self
            .call::<MessagesWire>(method::CONVERSATIONS_HISTORY, params)
            .await?;
        Ok(page(resp, |w| w.messages))
    }

    /// One page of `conversations.replies`: the thread root followed by replies.
    pub async fn conversations_replies(
        &self,
        channel: &ChannelId,
        thread_ts: &Ts,
        opts: &HistoryOpts,
        cursor: Option<String>,
    ) -> Result<Page<Message>> {
        let mut params = vec![
            ("channel", channel.to_string()),
            ("ts", thread_ts.to_string()),
        ];
        opts.push(&mut params);
        if let Some(cursor) = cursor {
            params.push(("cursor", cursor));
        }
        let resp = self
            .call::<MessagesWire>(method::CONVERSATIONS_REPLIES, params)
            .await?;
        Ok(page(resp, |w| w.messages))
    }

    /// `conversations.info`.
    pub async fn conversations_info(&self, channel: &ChannelId) -> Result<Conversation> {
        let params = vec![
            ("channel", channel.to_string()),
            ("include_num_members", "true".to_owned()),
        ];
        let resp = self
            .call::<ChannelWire>(method::CONVERSATIONS_INFO, params)
            .await?;
        Ok(resp.data.channel)
    }

    /// One page of `conversations.list`.
    pub async fn conversations_list(
        &self,
        opts: &ListOpts,
        cursor: Option<String>,
    ) -> Result<Page<Conversation>> {
        let params = listing_params(opts, cursor);
        let resp = self
            .call::<ChannelsWire>(method::CONVERSATIONS_LIST, params)
            .await?;
        Ok(page(resp, |w| w.channels))
    }

    /// One page of `users.conversations`: the conversations the signed-in user is a member
    /// of. Reports no `is_open`, so it is no source for DM visibility.
    pub async fn users_conversations(
        &self,
        opts: &ListOpts,
        cursor: Option<String>,
    ) -> Result<Page<Conversation>> {
        let params = listing_params(opts, cursor);
        let resp = self
            .call::<ChannelsWire>(method::USERS_CONVERSATIONS, params)
            .await?;
        Ok(page(resp, |w| w.channels))
    }

    /// Every page of `users.conversations`.
    pub async fn users_conversations_all(&self, opts: &ListOpts) -> Result<Vec<Conversation>> {
        paginate(|cursor| self.users_conversations(opts, cursor))
            .try_collect()
            .await
    }

    /// Every page of `conversations.list`.
    pub async fn conversations_list_all(&self, opts: &ListOpts) -> Result<Vec<Conversation>> {
        paginate(|cursor| self.conversations_list(opts, cursor))
            .try_collect()
            .await
    }

    /// One page of `conversations.members`: the conversation's member ids.
    pub async fn conversations_members(
        &self,
        channel: &ChannelId,
        cursor: Option<String>,
    ) -> Result<Page<UserId>> {
        let mut params = vec![("channel", channel.to_string())];
        if let Some(cursor) = cursor {
            params.push(("cursor", cursor));
        }
        let resp = self
            .call::<MemberIdsWire>(method::CONVERSATIONS_MEMBERS, params)
            .await?;
        Ok(page(resp, |w| w.members))
    }

    /// Every page of `conversations.members`.
    pub async fn conversations_members_all(&self, channel: &ChannelId) -> Result<Vec<UserId>> {
        paginate(|cursor| self.conversations_members(channel, cursor))
            .try_collect()
            .await
    }

    /// A channel's managers: the users holding Slack's channel-manager role on it, from
    /// `admin.roles.entity.listAssignments` (what Slack's channel details read). Read-only.
    pub async fn channel_managers(&self, channel: &ChannelId) -> Result<Vec<UserId>> {
        let params = vec![("entity_id", channel.to_string())];
        let resp = self
            .call::<RoleAssignmentsWire>(method::ADMIN_ROLES_ENTITY_LIST_ASSIGNMENTS, params)
            .await?;
        Ok(resp
            .data
            .role_assignments
            .into_iter()
            .filter(|assignment| assignment.role_id == CHANNEL_MANAGER_ROLE)
            .flat_map(|assignment| assignment.users)
            .collect())
    }

    /// One page of `client.dms`: the DMs-tab list (IMs and MPIMs) with per-DM `is_open`,
    /// closed DMs included (docs/research/api-channels.md, `client.dms`).
    pub async fn client_dms(&self, cursor: Option<String>) -> Result<Page<Conversation>> {
        let mut params = vec![
            ("count", "250".to_owned()),
            ("include_closed", "true".to_owned()),
            ("include_channel", "true".to_owned()),
            ("_x_reason", "dms-tab-populate".to_owned()),
        ];
        if let Some(cursor) = cursor {
            params.push(("cursor", cursor));
        }
        let resp = self.call::<DmsWire>(method::CLIENT_DMS, params).await?;
        let next_cursor = resp.next_cursor().map(str::to_owned);
        let items = resp
            .data
            .ims
            .into_iter()
            .chain(resp.data.mpims)
            .map(DmWire::into_conversation)
            .collect();
        Ok(Page { items, next_cursor })
    }

    /// Every page of `client.dms`.
    pub async fn client_dms_all(&self) -> Result<Vec<Conversation>> {
        paginate(|cursor| self.client_dms(cursor))
            .try_collect()
            .await
    }

    /// `conversations.mark`: moves the read cursor to `ts` (an earlier ts marks unread).
    /// `conversations.leave`: the signed-in user leaves `channel`.
    pub async fn conversations_leave(&self, channel: &ChannelId) -> Result<()> {
        let params = vec![("channel", channel.to_string())];
        self.call::<Empty>(method::CONVERSATIONS_LEAVE, params)
            .await?;
        Ok(())
    }

    pub async fn conversations_mark(&self, channel: &ChannelId, ts: &Ts) -> Result<()> {
        let params = vec![("channel", channel.to_string()), ("ts", ts.to_string())];
        self.call::<Empty>(method::CONVERSATIONS_MARK, params)
            .await?;
        Ok(())
    }

    /// `subscriptions.thread.add` / `subscriptions.thread.remove`: follow or unfollow a
    /// thread, so its replies notify you and it lists under Threads. `last_read` is the
    /// thread's read cursor, which Slack's client sends with both.
    pub async fn subscriptions_thread_set(
        &self,
        channel: &ChannelId,
        thread_ts: &Ts,
        last_read: &Ts,
        subscribed: bool,
    ) -> Result<()> {
        let params = vec![
            ("channel", channel.to_string()),
            ("thread_ts", thread_ts.to_string()),
            ("last_read", last_read.to_string()),
        ];
        let method = if subscribed {
            method::SUBSCRIPTIONS_THREAD_ADD
        } else {
            method::SUBSCRIPTIONS_THREAD_REMOVE
        };
        self.call::<Empty>(method, params).await?;
        Ok(())
    }

    /// `conversations.open`: opens (or finds) the DM with `users`, a comma-separated id list.
    /// `return_im` makes a single-user call answer one `channel` object.
    pub async fn conversations_open(&self, users: &str) -> Result<Conversation> {
        let params = vec![
            ("users", users.to_owned()),
            ("return_im", "true".to_owned()),
        ];
        let resp = self
            .call::<ChannelWire>(method::CONVERSATIONS_OPEN, params)
            .await?;
        Ok(resp.data.channel)
    }

    /// `conversations.setTopic`: replace `channel`'s topic; the answer carries the updated
    /// channel object.
    pub async fn conversations_set_topic(
        &self,
        channel: &ChannelId,
        topic: &str,
    ) -> Result<Conversation> {
        let params = vec![
            ("channel", channel.to_string()),
            ("topic", topic.to_owned()),
        ];
        Ok(self
            .call::<ChannelWire>(method::CONVERSATIONS_SET_TOPIC, params)
            .await?
            .data
            .channel)
    }

    /// `conversations.setPurpose`: replace `channel`'s description; the answer carries the
    /// updated channel object.
    pub async fn conversations_set_purpose(
        &self,
        channel: &ChannelId,
        purpose: &str,
    ) -> Result<Conversation> {
        let params = vec![
            ("channel", channel.to_string()),
            ("purpose", purpose.to_owned()),
        ];
        Ok(self
            .call::<ChannelWire>(method::CONVERSATIONS_SET_PURPOSE, params)
            .await?
            .data
            .channel)
    }

    /// `team.info`: one workspace's record, by team id. Reaches the other organizations of a
    /// Slack Connect channel, which `client.userBoot` never carries.
    pub async fn team_info(&self, team: &TeamId) -> Result<Team> {
        let params = vec![("team", team.to_string())];
        Ok(self
            .call::<TeamWire>(method::TEAM_INFO, params)
            .await?
            .data
            .team)
    }

    /// `subscriptions.thread.getView`: the followed threads of the Threads view, newest
    /// activity first (the web client's `fetch_threads_state` / `priority_mode` parameters;
    /// `limit` is the page size, `has_more` says older threads exist).
    pub async fn subscriptions_thread_get_view(&self, limit: u32) -> Result<ThreadsView> {
        let params = vec![
            ("limit", limit.to_string()),
            ("fetch_threads_state", "true".to_owned()),
            ("priority_mode", "all".to_owned()),
        ];
        Ok(self
            .call(method::SUBSCRIPTIONS_THREAD_GET_VIEW, params)
            .await?
            .data)
    }

    /// `chat.postMessage` in the web client's wire format: `blocks` JSON, a fresh UUID
    /// `client_msg_id` for server-side dedup, `unfurl=[]`. Set `thread_ts` to reply in a thread;
    /// `reply_broadcast` also posts the reply to the channel.
    pub async fn chat_post_message(
        &self,
        channel: &ChannelId,
        content: &MessageContent,
        thread_ts: Option<&Ts>,
        reply_broadcast: bool,
    ) -> Result<PostedMessage> {
        let client_msg_id = uuid::Uuid::new_v4().to_string();
        let mut params = vec![
            ("channel", channel.to_string()),
            ("type", "message".to_owned()),
            ("client_msg_id", client_msg_id.clone()),
            ("unfurl", "[]".to_owned()),
            ("_x_reason", REASON_MESSAGE_SEND.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        content.push(&mut params)?;
        if let Some(thread_ts) = thread_ts {
            params.push(("thread_ts", thread_ts.to_string()));
            if reply_broadcast {
                params.push(("reply_broadcast", "true".to_owned()));
            }
        }
        let mut posted = self
            .call::<PostedMessage>(method::CHAT_POST_MESSAGE, params)
            .await?
            .data;
        posted.client_msg_id = Some(client_msg_id);
        Ok(posted)
    }

    /// `chat.command` in the web client's wire format: the arguments as one `rich_text` block
    /// (omitted when there are none) and a fresh client token. Slack answers on the
    /// websocket: a posted message, an ephemeral (Giphy's preview) or a modal.
    pub async fn chat_command(
        &self,
        channel: &ChannelId,
        command: &str,
        args: &str,
        thread_ts: Option<&Ts>,
        team: &TeamId,
    ) -> Result<()> {
        let mut params = vec![
            ("command", command.to_owned()),
            ("disp", command.to_owned()),
        ];
        if !args.is_empty() {
            let blocks: [Block; 1] = [RichTextBlock::plain_text(args.to_owned()).into()];
            params.push(("blocks", serde_json::to_string(&blocks)?));
        }
        params.push(("channel", channel.to_string()));
        params.push(("client_token", client_token()));
        if let Some(thread_ts) = thread_ts {
            params.push(("thread_ts", thread_ts.to_string()));
        }
        params.extend([
            ("team_id", team.to_string()),
            ("_x_reason", REASON_COMMAND.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ]);
        self.call::<Empty>(method::CHAT_COMMAND, params).await?;
        Ok(())
    }

    /// `blocks.actions`: a Block Kit interaction dispatched to the app (`bot`, `app_id`) that
    /// posted the message. `action` is the element as the message declared it plus its
    /// `block_id`. Slack answers on the websocket: the message changed or deleted, a message
    /// posted.
    pub async fn blocks_actions(
        &self,
        bot: &BotId,
        app_id: &str,
        team: &TeamId,
        action: &Value,
        container: &ActionContainer,
    ) -> Result<()> {
        let params = vec![
            ("service_id", bot.to_string()),
            ("app_id", app_id.to_owned()),
            ("service_team_id", team.to_string()),
            ("actions", serde_json::to_string(&[action])?),
            ("container", container.json().to_string()),
            ("client_token", client_token()),
            ("state", r#"{"values":{}}"#.to_owned()),
            ("_x_reason", REASON_BLOCK_ACTION.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::BLOCKS_ACTIONS, params).await?;
        Ok(())
    }

    /// `bots.info`: a bot's profile (name, icons, app) for messages that carry only its id.
    pub async fn bots_info(&self, bot: &BotId) -> Result<BotProfile> {
        #[derive(Deserialize)]
        struct BotInfo {
            bot: BotProfile,
        }
        let params = vec![("bot", bot.to_string())];
        Ok(self
            .call::<BotInfo>(method::BOTS_INFO, params)
            .await?
            .data
            .bot)
    }

    /// `chat.update`: replaces the text and blocks of message `ts`.
    pub async fn chat_update(
        &self,
        channel: &ChannelId,
        ts: &Ts,
        content: &MessageContent,
    ) -> Result<PostedMessage> {
        let mut params = vec![
            ("channel", channel.to_string()),
            ("ts", ts.to_string()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        content.push(&mut params)?;
        Ok(self.call(method::CHAT_UPDATE, params).await?.data)
    }

    /// `chat.delete`.
    pub async fn chat_delete(&self, channel: &ChannelId, ts: &Ts) -> Result<()> {
        let params = vec![
            ("channel", channel.to_string()),
            ("ts", ts.to_string()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::CHAT_DELETE, params).await?;
        Ok(())
    }

    /// `reactions.add`. `name` may include surrounding colons and a `::skin-tone-N` suffix.
    pub async fn reactions_add(&self, channel: &ChannelId, ts: &Ts, name: &str) -> Result<()> {
        self.reaction(method::REACTIONS_ADD, channel, ts, name)
            .await
    }

    /// `reactions.remove`.
    pub async fn reactions_remove(&self, channel: &ChannelId, ts: &Ts, name: &str) -> Result<()> {
        self.reaction(method::REACTIONS_REMOVE, channel, ts, name)
            .await
    }

    async fn reaction(&self, method: &str, channel: &ChannelId, ts: &Ts, name: &str) -> Result<()> {
        let params = vec![
            ("channel", channel.to_string()),
            ("timestamp", ts.to_string()),
            ("name", name.trim_matches(':').to_owned()),
            ("_x_reason", REASON_REACTION.to_owned()),
        ];
        self.call::<Empty>(method, params).await?;
        Ok(())
    }

    /// `users.info`.
    pub async fn users_prefs_set_notifications(
        &self,
        channel: &ChannelId,
        name: &str,
        value: &str,
    ) -> Result<()> {
        let params = vec![
            ("name", name.to_owned()),
            ("value", value.to_owned()),
            ("channel_id", channel.to_string()),
            // A per-channel override, as the web client sends it (`prefs-store`).
            ("global", "false".to_owned()),
            ("sync", "false".to_owned()),
        ];
        self.call::<Empty>(method::USERS_PREFS_SET_NOTIFICATIONS, params)
            .await?;
        Ok(())
    }

    pub async fn users_info(&self, user: &UserId) -> Result<User> {
        let params = vec![("user", user.to_string())];
        Ok(self
            .call::<UserWire>(method::USERS_INFO, params)
            .await?
            .data
            .user)
    }

    /// `dnd.info`: one user's do-not-disturb schedule and snooze state. Read-only.
    pub async fn dnd_info(&self, user: &UserId) -> Result<DndStatus> {
        let params = vec![("user", user.to_string())];
        Ok(self.call(method::DND_INFO, params).await?.data)
    }

    /// `users.getPresence`: one user's current presence. Read-only.
    pub async fn users_get_presence(&self, user: &UserId) -> Result<UserPresence> {
        let params = vec![("user", user.to_string())];
        Ok(self.call(method::USERS_GET_PRESENCE, params).await?.data)
    }

    /// One page of `users.list` (`limit` max 1000).
    pub async fn users_list(
        &self,
        limit: Option<u32>,
        cursor: Option<String>,
    ) -> Result<Page<User>> {
        let mut params = Vec::new();
        if let Some(limit) = limit {
            params.push(("limit", limit.to_string()));
        }
        if let Some(cursor) = cursor {
            params.push(("cursor", cursor));
        }
        let resp = self.call::<MembersWire>(method::USERS_LIST, params).await?;
        Ok(page(resp, |w| w.members))
    }

    /// Every page of `users.list`.
    pub async fn users_list_all(&self) -> Result<Vec<User>> {
        paginate(|cursor| self.users_list(Some(1000), cursor))
            .try_collect()
            .await
    }

    /// Every page of `users.channelSections.list`: the user's sidebar sections, in the order
    /// Slack returns them (the store applies display order). Unlike most methods, pagination
    /// uses the top-level `cursor`, not `response_metadata.next_cursor`.
    pub async fn users_channel_sections_list(&self) -> Result<Vec<ChannelSection>> {
        let mut sections = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut params = Vec::new();
            if let Some(cursor) = &cursor {
                params.push(("cursor", cursor.clone()));
            }
            let data = self
                .call::<ChannelSectionsWire>(method::USERS_CHANNEL_SECTIONS_LIST, params)
                .await?
                .data;
            for section in data.channel_sections.into_iter().map(ChannelSection::from) {
                merge_section(&mut sections, section);
            }
            let previous = cursor;
            // Stop on an empty cursor, or when Slack hands back the cursor it was just given
            // (the same loop guard `paginate` applies to `response_metadata` cursors).
            cursor = data
                .cursor
                .filter(|c| !c.is_empty() && Some(c) != previous.as_ref());
            if cursor.is_none() {
                break;
            }
        }
        Ok(sections)
    }

    /// `users.channelSections.create`: a sidebar section. `emoji` is a bare emoji name; the
    /// key is sent even when empty because Slack rejects a call without it
    /// (docs/research/api-channels.md).
    pub async fn users_channel_sections_create(
        &self,
        name: &str,
        emoji: &str,
    ) -> Result<SectionCreated> {
        let params = vec![("name", name.to_owned()), ("emoji", emoji.to_owned())];
        Ok(self
            .call(method::USERS_CHANNEL_SECTIONS_CREATE, params)
            .await?
            .data)
    }

    /// `users.channelSections.set`: renames a section. Every omitted key is CLEARED, so
    /// `name` and `emoji` must both be sent, with `emoji` read back from
    /// `users.channelSections.list` (`users.channelSections.update` / `.rename` answer
    /// `unknown_method`; docs/research/api-channels.md).
    pub async fn users_channel_sections_set(
        &self,
        channel_section_id: &str,
        name: &str,
        emoji: &str,
    ) -> Result<()> {
        let params = vec![
            ("channel_section_id", channel_section_id.to_owned()),
            ("name", name.to_owned()),
            ("emoji", emoji.to_owned()),
        ];
        self.call::<Empty>(method::USERS_CHANNEL_SECTIONS_SET, params)
            .await?;
        Ok(())
    }

    /// `users.channelSections.delete`: deletes a section; its conversations fall back to
    /// their default buckets.
    pub async fn users_channel_sections_delete(&self, channel_section_id: &str) -> Result<()> {
        let params = vec![("channel_section_id", channel_section_id.to_owned())];
        self.call::<Empty>(method::USERS_CHANNEL_SECTIONS_DELETE, params)
            .await?;
        Ok(())
    }

    /// `users.channelSections.channels.bulkUpdate`: moves channels between sections in one
    /// form-encoded call whose `insert` / `remove` values are JSON strings shaped
    /// `[{"channel_section_id":"L…","channel_ids":["C…"]}]` (docs/research/api-channels.md).
    /// A channel already in a section only moves when `remove` (its source) rides along with
    /// `insert` (its target); each side is omitted when empty, as the observed client does.
    pub async fn users_channel_sections_channels_bulk_update(
        &self,
        insert: Option<(&str, ChannelId)>,
        remove: Option<(&str, ChannelId)>,
    ) -> Result<()> {
        let side = |(section, channel): (&str, ChannelId)| {
            serde_json::to_string(&serde_json::json!([{
                "channel_section_id": section,
                "channel_ids": [channel.as_str()],
            }]))
            .unwrap_or_default()
        };
        let mut params = Vec::new();
        if let Some(insert) = insert {
            params.push(("insert", side(insert)));
        }
        if let Some(remove) = remove {
            params.push(("remove", side(remove)));
        }
        self.call::<Empty>(method::USERS_CHANNEL_SECTIONS_CHANNELS_BULK_UPDATE, params)
            .await?;
        Ok(())
    }

    /// `search.messages` with Slack's query modifier syntax (`in:`, `from:`, `before:`, ...).
    pub async fn search_messages(&self, query: &str, opts: &SearchOpts) -> Result<SearchMessages> {
        let mut params = vec![
            ("query", query.to_owned()),
            (
                "sort",
                match opts.sort {
                    SearchSort::Score => "score",
                    SearchSort::Timestamp => "timestamp",
                }
                .to_owned(),
            ),
            (
                "sort_dir",
                if opts.ascending { "asc" } else { "desc" }.to_owned(),
            ),
        ];
        if let Some(count) = opts.count {
            params.push(("count", count.to_string()));
        }
        if let Some(page) = opts.page {
            params.push(("page", page.to_string()));
        }
        if let Some(cursor) = &opts.cursor {
            params.push(("cursor", cursor.clone()));
        }
        if opts.highlight {
            params.push(("highlight", "true".to_owned()));
        }
        let resp = self
            .call::<SearchWire>(method::SEARCH_MESSAGES, params)
            .await?;
        Ok(resp.data.messages)
    }

    /// `emoji.list`: custom emoji name -> image URL or `alias:<name>`.
    pub async fn emoji_list(&self) -> Result<HashMap<String, String>> {
        Ok(self
            .call::<EmojiWire>(method::EMOJI_LIST, Vec::new())
            .await?
            .data
            .emoji)
    }

    /// `pins.list`: a conversation's pinned messages and files, oldest pin first.
    pub async fn pins_list(&self, channel: &ChannelId) -> Result<PinsList> {
        let params = vec![("channel", channel.to_string())];
        Ok(self.call(method::PINS_LIST, params).await?.data)
    }

    /// `bookmarks.list`: the conversation's bookmarks, folders included. Fetched lazily by
    /// the tab row (once per conversation, when a bookmarks surface is opened).
    pub async fn bookmarks_list(&self, channel: &ChannelId) -> Result<BookmarksList> {
        let params = vec![
            ("channel", channel.to_string()),
            ("include_folders", "true".to_owned()),
        ];
        Ok(self.call(method::BOOKMARKS_LIST, params).await?.data)
    }

    /// `bookmarks.add` of a `link` bookmark to the conversation's bar.
    pub async fn bookmarks_add_link(
        &self,
        channel: &ChannelId,
        title: &str,
        link: &str,
    ) -> Result<()> {
        let params = vec![
            ("channel_id", channel.to_string()),
            ("title", title.to_owned()),
            ("type", "link".to_owned()),
            ("link", link.to_owned()),
        ];
        self.call::<Empty>(method::BOOKMARKS_ADD, params).await?;
        Ok(())
    }

    /// One page of `files.list`: the files matching `filter`, newest first. A `channel`
    /// scopes the call to a conversation; without it Slack answers every file the
    /// signed-in user can see workspace-wide. `files.list` paginates by `page`, so a
    /// single large first page covers both the Files & links tab and the Files view.
    pub async fn files_list(
        &self,
        filter: &FilesFilter,
        count: u32,
        page_no: u32,
    ) -> Result<Page<File>> {
        let mut params = Vec::new();
        if let Some(channel) = &filter.channel {
            params.push(("channel", channel.to_string()));
        }
        if let Some(user) = &filter.user {
            params.push(("user", user.to_string()));
        }
        if let Some(types) = &filter.types {
            params.push(("types", types.clone()));
        }
        params.push(("count", count.to_string()));
        params.push(("page", page_no.to_string()));
        let resp = self.call::<FilesWire>(method::FILES_LIST, params).await?;
        Ok(page(resp, |w| w.files))
    }

    /// `conversations.canvases.create`: creates the conversation's empty channel canvas and
    /// returns its file id. Fails with `channel_canvas_already_exists` when one exists
    /// already (read the id from `conversations.info` -> `properties.canvas` then).
    pub async fn conversations_canvases_create(
        &self,
        channel: &ChannelId,
    ) -> Result<CanvasCreated> {
        let params = vec![("channel_id", channel.to_string())];
        Ok(self
            .call(method::CONVERSATIONS_CANVASES_CREATE, params)
            .await?
            .data)
    }

    /// `activity.feed`, page one (the view's first `limit` items, newest first; the next
    /// page's cursor rides in [`ActivityFeed::next_cursor`]). Row bodies arrive separately
    /// through [`SlackApi::messages_list`] when the feed itself carried none.
    pub async fn activity_feed(&self, limit: u32) -> Result<ActivityFeed> {
        let params = vec![
            ("types", ACTIVITY_TYPES.to_owned()),
            ("mode", "chrono_v1".to_owned()),
            ("limit", limit.to_string()),
            ("is_activity_inbox", "true".to_owned()),
        ];
        let resp = self
            .call::<ActivityFeedWire>(method::ACTIVITY_FEED, params)
            .await?;
        let next_cursor = resp.next_cursor().map(str::to_owned);
        Ok(ActivityFeed {
            items: resp.data.items.into_iter().filter_map(row).collect(),
            next_cursor,
        })
    }

    /// `messages.list`: the bodies of the given messages, as the web client fills the
    /// Activity feed's previews. At most [`MESSAGES_LIST_CHANNELS`] channels per call (a
    /// workspace that accepted 9 rejected 39), so wider asks are batched here.
    pub async fn messages_list(
        &self,
        ids: &[(ChannelId, Vec<Ts>)],
    ) -> Result<HashMap<ChannelId, Vec<Message>>> {
        let mut bodies = HashMap::new();
        for batch in ids.chunks(MESSAGES_LIST_CHANNELS) {
            let message_ids: Vec<Value> = batch
                .iter()
                .map(|(channel, stamps)| {
                    Value::Object(Map::from_iter([
                        (
                            "channel".to_owned(),
                            Value::String(channel.as_str().to_owned()),
                        ),
                        (
                            "timestamps".to_owned(),
                            Value::Array(
                                stamps
                                    .iter()
                                    .map(|ts| Value::String(ts.as_str().to_owned()))
                                    .collect(),
                            ),
                        ),
                    ]))
                })
                .collect();
            let params = vec![
                ("message_ids", serde_json::to_string(&message_ids)?),
                ("org_wide_aware", "true".to_owned()),
                ("cached_latest_updates", "{}".to_owned()),
            ];
            let resp = self
                .call::<MessagesListWire>(method::MESSAGES_LIST, params)
                .await?;
            for (channel, entry) in resp.data.messages_data {
                bodies.insert(ChannelId::from(channel.as_str()), entry.messages);
            }
        }
        Ok(bodies)
    }

    /// `activity.markRead`: marks feed entries read (each item's handles, see
    /// [`ActivityItem::entry`]).
    pub async fn activity_mark_read(&self, entries: &[ActivityEntry]) -> Result<()> {
        self.activity_mark(method::ACTIVITY_MARK_READ, entries)
            .await
    }

    /// `activity.markUnread`: marks feed entries unread.
    pub async fn activity_mark_unread(&self, entries: &[ActivityEntry]) -> Result<()> {
        self.activity_mark(method::ACTIVITY_MARK_UNREAD, entries)
            .await
    }

    async fn activity_mark(&self, method: &str, entries: &[ActivityEntry]) -> Result<()> {
        let params = vec![
            ("entries", serde_json::to_string(entries)?),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method, params).await?;
        Ok(())
    }
}

/// `activity.feed`'s envelope.
#[derive(Deserialize)]
struct ActivityFeedWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    items: Vec<ActivityItemWire>,
}

/// Item types the feed is asked for: every category slackless models, minus
/// `unjoined_channel_mention`, which this workspace rejects with `invalid_arguments`
/// (probed live; Slack's fuller bootstrap list adds only types the view skips anyway).
const ACTIVITY_TYPES: &str = concat!(
    "at_user,at_user_group,at_channel,at_everyone,keyword,",
    "thread_v2,message_reaction,dm,bot_dm_bundle,channel"
);

/// Channels per `messages.list` call before Slack answers `too_many_channels`
/// (docs/research/api-activity.md, `messages.list`).
const MESSAGES_LIST_CHANNELS: usize = 8;

/// Normalizes one wire entry into the row the view renders; `None` for the shapes slackless
/// does not model (`Unknown`) or cannot place (no channel or message ts).
fn row(wire: ActivityItemWire) -> Option<ActivityItem> {
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
fn message_row(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ApiResponse;

    /// A captured `activity.feed` answer (personal data replaced): one row per modeled type,
    /// an unknown type, and a thread entry without its bundle.
    const FEED: &str = r#"{
        "ok": true,
        "items": [
            {
                "is_unread": true,
                "is_bot": false,
                "is_archived": false,
                "version": "v2",
                "feed_ts": "1784892914.789649",
                "key": "thread_v2-C0ENG-1784891984.803759",
                "item": {
                    "type": "thread_v2",
                    "bundle_info": {
                        "unread_count": 3,
                        "payload": {
                            "thread_entry": {
                                "channel_id": "C0ENG",
                                "thread_ts": "1784891984.803759",
                                "latest_ts": "1784892900.000100",
                                "unread_msg_count": 2
                            }
                        }
                    }
                }
            },
            {
                "is_unread": false,
                "is_bot": false,
                "feed_ts": "1784891000.000200",
                "key": "at_user-C0ENG-1784891000.000200",
                "item": {
                    "type": "at_user",
                    "message": {
                        "ts": "1784891000.000200",
                        "channel": "C0ENG",
                        "author_user_id": "U0AUTHOR",
                        "thread_ts": "1784891984.803759",
                        "is_broadcast": false
                    }
                }
            },
            {
                "is_unread": false,
                "feed_ts": "1784890000.000300",
                "key": "at_channel-C0ENG-1784890000.000300",
                "item": {
                    "type": "at_channel",
                    "message": {
                        "ts": "1784890000.000300",
                        "channel": "C0ENG",
                        "author_user_id": "U0AUTHOR",
                        "is_broadcast": true
                    }
                }
            },
            {
                "is_unread": true,
                "feed_ts": "1784889000.000400",
                "key": "message_reaction-C0ENG-1784889000.000400",
                "item": {
                    "type": "message_reaction",
                    "message": {
                        "ts": "1784800000.000400",
                        "channel": "C0ENG",
                        "author_user_id": "U0ME"
                    },
                    "reaction": { "user": "U0REACTOR", "name": "eyes" }
                }
            },
            {
                "is_unread": true,
                "feed_ts": "1784888000.000500",
                "key": "dm-D0PEER",
                "item": {
                    "type": "dm",
                    "bundle_info": {
                        "unread_count": 2,
                        "payload": {
                            "dm_entry": {
                                "latest_message": {
                                    "ts": "1784888000.000500",
                                    "channel": "D0PEER"
                                }
                            }
                        }
                    }
                }
            },
            {
                "is_bot": true,
                "is_unread": false,
                "feed_ts": "1784887000.000600",
                "key": "bot_dm_bundle-D0BOT",
                "item": {
                    "type": "bot_dm_bundle",
                    "bundle_info": {
                        "unread_count": 0,
                        "payload": {
                            "message": {
                                "ts": "1784887000.000600",
                                "channel": "D0BOT"
                            }
                        }
                    }
                }
            },
            {
                "is_unread": false,
                "feed_ts": "1784886000.000700",
                "key": "saved_reminder-X0-1784886000.000700",
                "item": {
                    "type": "saved_reminder",
                    "saved_reminder_payload": { "item_type": "message" }
                }
            }
        ],
        "response_metadata": { "next_cursor": "abc" }
    }"#;

    #[test]
    fn parses_activity_feed_and_skips_unknown_types() {
        let value: Value = serde_json::from_str(FEED).unwrap();
        let resp: ApiResponse<ActivityFeedWire> =
            ApiResponse::from_value(value).expect("feed parses");
        let next_cursor = resp.next_cursor().map(str::to_owned);
        let feed = ActivityFeed {
            items: resp.data.items.into_iter().filter_map(row).collect(),
            next_cursor,
        };

        assert_eq!(feed.next_cursor.as_deref(), Some("abc"));
        let items = feed.items;
        assert_eq!(items.len(), 6, "the saved_reminder row is skipped");

        let thread = &items[0];
        assert_eq!(thread.kind, ActivityKind::Thread);
        assert!(thread.unread);
        assert_eq!(thread.channel.as_str(), "C0ENG");
        assert_eq!(
            thread.thread_ts.as_ref().map(|ts| ts.as_str()),
            Some("1784891984.803759")
        );
        assert_eq!(thread.ts.as_str(), "1784892900.000100");
        assert_eq!(
            thread.count, 3,
            "the bundle's unread count wins over the entry's"
        );
        assert!(thread.message.is_none(), "thread rows hydrate separately");

        let mention = &items[1];
        assert_eq!(mention.kind, ActivityKind::Mention);
        assert_eq!(mention.actor.as_ref().map(|u| u.as_str()), Some("U0AUTHOR"));
        assert_eq!(
            mention.thread_ts.as_ref().map(|ts| ts.as_str()),
            Some("1784891984.803759")
        );

        // The at_channel alias parses into the same mention shape.
        assert_eq!(items[2].kind, ActivityKind::Mention);
        assert!(items[2].thread_ts.is_none());

        let reaction = &items[3];
        assert_eq!(reaction.kind, ActivityKind::Reaction);
        assert_eq!(
            reaction.actor.as_ref().map(|u| u.as_str()),
            Some("U0REACTOR")
        );
        let reaction = reaction.reaction.as_ref().unwrap();
        assert_eq!(
            (
                reaction.name.as_str(),
                reaction.user.as_ref().map(|u| u.as_str())
            ),
            ("eyes", Some("U0REACTOR"))
        );

        let dm = &items[4];
        assert_eq!(dm.kind, ActivityKind::Dm);
        assert_eq!(dm.channel.as_str(), "D0PEER", "the ref carries the channel");
        assert_eq!(dm.ts.as_str(), "1784888000.000500");
        assert_eq!(dm.count, 2, "the bundle's unread count becomes the pill");
        assert!(dm.message.is_none(), "a bare ref hydrates separately");

        let app = &items[5];
        assert_eq!(app.kind, ActivityKind::App);
        assert_eq!(app.channel.as_str(), "D0BOT");
        assert!(app.message.is_none());
        assert_eq!(app.count, 0);
    }

    #[test]
    fn team_info_wire_parses_the_connected_org_record() {
        let value: Value = serde_json::from_str(
            r#"{
                "ok": true,
                "team": {
                    "id": "T0KLARNA",
                    "name": "Klarna",
                    "domain": "klarna",
                    "email_domain": "klarna.com",
                    "icon": {
                        "image_default": true,
                        "image_34": "https://a.slack-edge.com/img/avatars-teams/ava_34.png",
                        "image_68": "https://a.slack-edge.com/img/avatars-teams/ava_68.png"
                    },
                    "avatar_base_url": "https://ca-slack-atl.com/"
                }
            }"#,
        )
        .unwrap();
        let resp: ApiResponse<TeamWire> = ApiResponse::from_value(value).expect("team parses");
        let team = resp.data.team;
        assert_eq!(team.id.as_str(), "T0KLARNA");
        assert_eq!(team.name, "Klarna");
        assert_eq!(team.domain, "klarna");
        assert_eq!(
            team.icon.as_ref().and_then(|icon| icon.get("image_34")),
            Some(&Value::String(
                "https://a.slack-edge.com/img/avatars-teams/ava_34.png".into()
            )),
            "the icon object survives verbatim for the UI to pick a URL from"
        );
        assert!(team.extra.contains_key("avatar_base_url"));
    }

    #[test]
    fn mark_read_entry_keeps_the_keys_type_prefix() {
        let value: Value = serde_json::from_str(FEED).unwrap();
        let resp: ApiResponse<ActivityFeedWire> =
            ApiResponse::from_value(value).expect("feed parses");
        let next_cursor = resp.next_cursor().map(str::to_owned);
        let items: Vec<ActivityItem> = resp.data.items.into_iter().filter_map(row).collect();
        assert_eq!(next_cursor.as_deref(), Some("abc"));
        let entries: Vec<ActivityEntry> = items.iter().map(ActivityItem::entry).collect();
        assert_eq!(entries[0].kind, "thread_v2");
        assert_eq!(entries[0].key, "thread_v2-C0ENG-1784891984.803759");
        assert_eq!(entries[0].feed_ts.as_str(), "1784892914.789649");
        // An at_channel mention keeps its own type, not at_user.
        assert_eq!(entries[2].kind, "at_channel");
        assert_eq!(entries[5].kind, "bot_dm_bundle");
        // The wire form is the JSON array Slack expects.
        assert_eq!(
            serde_json::to_string(&entries[0]).unwrap(),
            "{\"type\":\"thread_v2\",\"key\":\"thread_v2-C0ENG-1784891984.803759\",\"feed_ts\":\"1784892914.789649\"}"
        );
    }

    #[test]
    fn messages_list_bodies_parse_by_channel() {
        let wire: MessagesListWire = serde_json::from_str(
            r#"{ "messages_data": {
                "C0ENG": { "messages": [
                    { "ts": "1784891000.000200", "user": "U0AUTHOR", "text": "hey <@U0ME>" }
                ] },
                "D0PEER": { "messages": [] }
            } }"#,
        )
        .unwrap();
        let bodies: HashMap<ChannelId, Vec<Message>> = wire
            .messages_data
            .into_iter()
            .map(|(channel, entry)| (ChannelId::from(channel.as_str()), entry.messages))
            .collect();
        let eng = &bodies[&ChannelId::from("C0ENG")];
        assert_eq!((eng.len(), eng[0].text.as_str()), (1, "hey <@U0ME>"));
        assert!(bodies[&ChannelId::from("D0PEER")].is_empty());
    }

    /// A one-off probe of the live `activity.feed` answer, printed as raw JSON so the
    /// parser is built against the real shape (values carry personal data; the shipped
    /// fixture replaces them). Run manually with the signed-in session in the app's store;
    /// never in CI:
    ///
    /// ```text
    /// CARGO_BUILD_JOBS=4 direnv exec . cargo test -p slackless-api activity_feed_live \
    ///     -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "probes the live workspace; run manually (see the test docs)"]
    fn activity_feed_live() {
        use std::process::Command;

        let sessions: Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
                    .join("Library/Application Support/slackless/sessions.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let team = sessions[0]["team_id"].as_str().unwrap().to_owned();
        let url = sessions[0]["workspace_url"].as_str().unwrap().to_owned();
        let secret = Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                "slackless",
                "-a",
                &team,
                "-w",
            ])
            .output()
            .unwrap();
        let secret: Value = serde_json::from_slice(secret.stdout.trim_ascii_end()).unwrap();
        let host = url::Url::parse(&url)
            .unwrap()
            .host_str()
            .unwrap()
            .to_owned();
        let domain = host.strip_suffix(".slack.com").unwrap_or(&host);
        let api = SlackApi::for_workspace(
            domain,
            secret["xoxc"].as_str().unwrap(),
            secret["d"].as_str().unwrap(),
        )
        .unwrap();

        // Param bisection: which type lists Slack accepts.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let params: Vec<(&'static str, String)> = vec![
            ("types", ACTIVITY_TYPES.to_owned()),
            ("mode", "chrono_v1".to_owned()),
            ("limit", "50".to_owned()),
            ("is_activity_inbox", "true".to_owned()),
        ];
        let resp = rt
            .block_on(api.call::<Value>(method::ACTIVITY_FEED, params))
            .unwrap();
        println!("{}", serde_json::to_string_pretty(&resp.data).unwrap());

        // messages.list hydration of the first few rows.
        let feed = rt.block_on(api.activity_feed(10)).unwrap();
        let ids: Vec<(ChannelId, Vec<Ts>)> = feed
            .items
            .iter()
            .take(6)
            .map(|item| (item.channel.clone(), vec![item.ts.clone()]))
            .collect();
        let bodies = rt.block_on(api.messages_list(&ids)).unwrap();
        println!("hydrated {} channels", bodies.len());
        for (channel, messages) in &bodies {
            println!(
                "{channel}: {} messages, first text {} chars",
                messages.len(),
                messages.first().map(|m| m.text.len()).unwrap_or(0)
            );
        }
    }
}
