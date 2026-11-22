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
    TeamId, Ts, User, UserId, UserPresence, Usergroup,
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

/// One pending file to upload through Slack's external upload v2 flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadFile {
    pub filename: String,
    pub bytes: Vec<u8>,
}

/// Answer to `files.getUploadURLExternal`.
#[derive(Debug, Clone, Deserialize)]
pub struct UploadSlot {
    pub file_id: String,
    pub upload_url: String,
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

    fn push(&self, params: &mut Params) -> Result<()> {
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

/// Response to `chat.scheduleMessage`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ScheduledMessageCreated {
    pub scheduled_message_id: String,
    pub channel: ChannelId,
    pub post_at: u64,
}

/// One pending scheduled message returned by `chat.scheduledMessages.list`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ScheduledMessage {
    pub id: String,
    pub channel_id: ChannelId,
    pub post_at: u64,
    #[serde(default)]
    pub date_created: u64,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Deserialize)]
struct ScheduledMessagesList {
    #[serde(default)]
    scheduled_messages: Vec<ScheduledMessage>,
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

    /// `auth.revoke`: revokes the current token server-side on sign-out (`F-CORE-009`).
    pub async fn auth_revoke(&self) -> Result<()> {
        let _: serde_json::Value = self.call(method::AUTH_REVOKE, Vec::new()).await?.data;
        Ok(())
    }

    /// `conversations.addTab`: add a tab (`files`, `bookmarks`, `folder`, `canvas`, etc.) to `channel` (`F-CORE-061`).
    pub async fn conversations_add_tab(
        &self,
        channel: &ChannelId,
        tab_type: &str,
        label: Option<&str>,
    ) -> Result<()> {
        let mut params = vec![
            ("channel_id", channel.as_str().to_owned()),
            ("type", tab_type.to_owned()),
            ("reason", "add-channel-tab".to_owned()),
        ];
        if let Some(label) = label.filter(|s| !s.is_empty()) {
            params.push(("label", label.to_owned()));
        }
        let _: serde_json::Value = self.call(method::CONVERSATIONS_ADD_TAB, params).await?.data;
        Ok(())
    }

    /// `conversations.removeTab`: remove a tab from `channel` while keeping shared files (`F-CORE-061`).
    pub async fn conversations_remove_tab(&self, channel: &ChannelId, tab_id: &str) -> Result<()> {
        let params = vec![
            ("channel_id", channel.as_str().to_owned()),
            ("id", tab_id.to_owned()),
            ("keep_file_share", "true".to_owned()),
            ("reason", "remove-channel-tab".to_owned()),
        ];
        let _: serde_json::Value = self
            .call(method::CONVERSATIONS_REMOVE_TAB, params)
            .await?
            .data;
        Ok(())
    }

    /// `conversations.moveTab`: reorder a tab inside `channel` (`F-CORE-061`).
    pub async fn conversations_move_tab(
        &self,
        channel: &ChannelId,
        tab_id: &str,
        after: Option<&str>,
    ) -> Result<()> {
        let mut params = vec![
            ("channel_id", channel.as_str().to_owned()),
            ("id", tab_id.to_owned()),
        ];
        if let Some(after) = after.filter(|s| !s.is_empty()) {
            params.push(("after", after.to_owned()));
        }
        let _: serde_json::Value = self
            .call(method::CONVERSATIONS_MOVE_TAB, params)
            .await?
            .data;
        Ok(())
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

    /// `users.markAllRead`: every conversation and thread read (Slack's Shift+Esc).
    pub async fn users_mark_all_read(&self) -> Result<()> {
        self.call::<Empty>(method::USERS_MARK_ALL_READ, Vec::new())
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
    /// `conversations.join`: the signed-in user joins public `channel`; the answer carries
    /// the joined channel.
    pub async fn conversations_join(&self, channel: &ChannelId) -> Result<Conversation> {
        let params = vec![("channel", channel.to_string())];
        Ok(self
            .call::<ChannelWire>(method::CONVERSATIONS_JOIN, params)
            .await?
            .data
            .channel)
    }

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

    /// `conversations.create`: create a new public or private channel.
    pub async fn conversations_create(&self, name: &str, is_private: bool) -> Result<Conversation> {
        let params = vec![
            ("name", name.to_owned()),
            (
                "is_private",
                if is_private { "true" } else { "false" }.to_owned(),
            ),
        ];
        Ok(self
            .call::<ChannelWire>(method::CONVERSATIONS_CREATE, params)
            .await?
            .data
            .channel)
    }

    /// `conversations.rename`: rename an existing channel.
    pub async fn conversations_rename(
        &self,
        channel: &ChannelId,
        name: &str,
    ) -> Result<Conversation> {
        let params = vec![("channel", channel.to_string()), ("name", name.to_owned())];
        Ok(self
            .call::<ChannelWire>(method::CONVERSATIONS_RENAME, params)
            .await?
            .data
            .channel)
    }

    /// `conversations.archive`: archive an active channel.
    pub async fn conversations_archive(&self, channel: &ChannelId) -> Result<()> {
        let params = vec![("channel", channel.to_string())];
        self.call::<Empty>(method::CONVERSATIONS_ARCHIVE, params)
            .await?;
        Ok(())
    }

    /// `conversations.unarchive`: restore an archived channel.
    pub async fn conversations_unarchive(&self, channel: &ChannelId) -> Result<()> {
        let params = vec![("channel", channel.to_string())];
        self.call::<Empty>(method::CONVERSATIONS_UNARCHIVE, params)
            .await?;
        Ok(())
    }

    /// `conversations.invite`: invite one or more users into `channel`.
    pub async fn conversations_invite(
        &self,
        channel: &ChannelId,
        users: &[UserId],
    ) -> Result<Conversation> {
        let joined = users
            .iter()
            .map(UserId::as_str)
            .collect::<Vec<_>>()
            .join(",");
        let params = vec![("channel", channel.to_string()), ("users", joined)];
        Ok(self
            .call::<ChannelWire>(method::CONVERSATIONS_INVITE, params)
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

    /// `blocks.suggestions`: fetch dynamic option suggestions for `external_select` /
    /// `multi_external_select` elements (`F-COLLAB-786`).
    #[allow(clippy::too_many_arguments)]
    pub async fn blocks_suggestions(
        &self,
        bot: &BotId,
        app_id: &str,
        team: &TeamId,
        action_id: &str,
        block_id: &str,
        value: &str,
        container: &ActionContainer,
    ) -> Result<Vec<(String, String)>> {
        let params = vec![
            ("service_id", bot.to_string()),
            ("app_id", app_id.to_owned()),
            ("service_team_id", team.to_string()),
            ("action_id", action_id.to_owned()),
            ("block_id", block_id.to_owned()),
            ("value", value.to_owned()),
            ("container", container.json().to_string()),
            ("client_token", client_token()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        let raw: Value = self.call(method::BLOCKS_SUGGESTIONS, params).await?.data;
        let groups = raw["option_groups"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|g| g["options"].as_array())
            .flatten();
        let options = raw["options"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(groups)
            .filter_map(|opt| {
                let text = opt["text"]["text"]
                    .as_str()
                    .or_else(|| opt["text"].as_str())?
                    .to_owned();
                let val = opt["value"].as_str()?.to_owned();
                Some((text, val))
            })
            .collect();
        Ok(options)
    }

    /// `views.submit`: submit a Block Kit modal view's `state.values` back to the app (`F-COLLAB-725`).
    pub async fn views_submit(
        &self,
        view_id: &str,
        hash: Option<&str>,
        state_values: &Value,
    ) -> Result<()> {
        let mut params = vec![
            ("view_id", view_id.to_owned()),
            (
                "state",
                serde_json::json!({ "values": state_values }).to_string(),
            ),
            ("client_token", client_token()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(hash) = hash {
            params.push(("hash", hash.to_owned()));
        }
        self.call::<Empty>(method::VIEWS_SUBMIT, params).await?;
        Ok(())
    }

    /// `views.close`: notify Slack that the user dismissed a Block Kit modal (`notify_on_close`).
    pub async fn views_close(&self, view_id: &str, hash: Option<&str>) -> Result<()> {
        let mut params = vec![
            ("view_id", view_id.to_owned()),
            ("client_token", client_token()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(hash) = hash {
            params.push(("hash", hash.to_owned()));
        }
        self.call::<Empty>(method::VIEWS_CLOSE, params).await?;
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

    /// `chat.scheduleMessage`: queue a message for delivery at unix timestamp `post_at`.
    pub async fn chat_schedule_message(
        &self,
        channel: &ChannelId,
        post_at: u64,
        content: &MessageContent,
        thread_ts: Option<&Ts>,
    ) -> Result<ScheduledMessageCreated> {
        let mut params = vec![
            ("channel", channel.to_string()),
            ("post_at", post_at.to_string()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(thread_ts) = thread_ts {
            params.push(("thread_ts", thread_ts.to_string()));
        }
        content.push(&mut params)?;
        Ok(self.call(method::CHAT_SCHEDULE_MESSAGE, params).await?.data)
    }

    /// `chat.scheduledMessages.list`: pending scheduled messages (all channels when `None`).
    pub async fn chat_scheduled_messages_list(
        &self,
        channel: Option<&ChannelId>,
    ) -> Result<Vec<ScheduledMessage>> {
        let mut params = Vec::new();
        if let Some(channel) = channel {
            params.push(("channel", channel.to_string()));
        }
        Ok(self
            .call::<ScheduledMessagesList>(method::CHAT_SCHEDULED_MESSAGES_LIST, params)
            .await?
            .data
            .scheduled_messages)
    }

    /// `chat.deleteScheduledMessage`: cancel a pending scheduled message.
    pub async fn chat_delete_scheduled_message(
        &self,
        channel: &ChannelId,
        scheduled_message_id: &str,
    ) -> Result<()> {
        let params = vec![
            ("channel", channel.to_string()),
            ("scheduled_message_id", scheduled_message_id.to_owned()),
        ];
        self.call::<Empty>(method::CHAT_DELETE_SCHEDULED_MESSAGE, params)
            .await?;
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

    /// `users.profile.get`: one user's full profile including custom `fields` and `pronouns`.
    pub async fn users_profile_get(&self, user: &UserId) -> Result<slackless_types::Profile> {
        #[derive(Deserialize)]
        struct Wire {
            profile: slackless_types::Profile,
        }
        let params = vec![
            ("user", user.to_string()),
            ("include_labels", "true".to_owned()),
        ];
        Ok(self
            .call::<Wire>(method::USERS_PROFILE_GET, params)
            .await?
            .data
            .profile)
    }

    /// `team.profile.get`: workspace custom profile field definitions (`id` -> `label`).
    pub async fn team_profile_get(&self) -> Result<HashMap<String, String>> {
        #[derive(Deserialize)]
        struct FieldDef {
            id: String,
            #[serde(default)]
            label: String,
        }
        #[derive(Deserialize)]
        struct ProfileDef {
            #[serde(default, deserialize_with = "lenient_vec")]
            fields: Vec<FieldDef>,
        }
        #[derive(Deserialize)]
        struct Wire {
            profile: ProfileDef,
        }
        let wire = self
            .call::<Wire>(method::TEAM_PROFILE_GET, Vec::new())
            .await?
            .data;
        Ok(wire
            .profile
            .fields
            .into_iter()
            .filter(|f| !f.label.is_empty())
            .map(|f| (f.id, f.label))
            .collect())
    }

    /// `dnd.info`: one user's do-not-disturb schedule and snooze state. Read-only.
    pub async fn dnd_info(&self, user: &UserId) -> Result<DndStatus> {
        let params = vec![("user", user.to_string())];
        Ok(self.call(method::DND_INFO, params).await?.data)
    }

    /// `dnd.setSnooze`: pause the signed-in user's notifications for `num_minutes`.
    pub async fn dnd_set_snooze(&self, num_minutes: u32) -> Result<DndStatus> {
        let params = vec![("num_minutes", num_minutes.to_string())];
        Ok(self.call(method::DND_SET_SNOOZE, params).await?.data)
    }

    /// `dnd.endSnooze`: resume the signed-in user's notifications immediately.
    pub async fn dnd_end_snooze(&self) -> Result<DndStatus> {
        Ok(self.call(method::DND_END_SNOOZE, Vec::new()).await?.data)
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

    /// `usergroups.list`: the workspace's active `@handle` user groups.
    pub async fn usergroups_list(&self) -> Result<Vec<Usergroup>> {
        let params = vec![
            ("include_count", "true".to_owned()),
            ("include_disabled", "false".to_owned()),
        ];
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default, deserialize_with = "lenient_vec")]
            usergroups: Vec<Usergroup>,
        }
        Ok(self
            .call::<Wire>(method::USERGROUPS_LIST, params)
            .await?
            .data
            .usergroups)
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

    /// `pins.add`: pin the message `ts` in `channel`.
    pub async fn pins_add(&self, channel: &ChannelId, ts: &Ts) -> Result<()> {
        let params = vec![
            ("channel", channel.to_string()),
            ("timestamp", ts.to_string()),
        ];
        self.call::<Empty>(method::PINS_ADD, params).await?;
        Ok(())
    }

    /// `pins.remove`: unpin the message `ts` in `channel`.
    pub async fn pins_remove(&self, channel: &ChannelId, ts: &Ts) -> Result<()> {
        let params = vec![
            ("channel", channel.to_string()),
            ("timestamp", ts.to_string()),
        ];
        self.call::<Empty>(method::PINS_REMOVE, params).await?;
        Ok(())
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

    /// `bookmarks.add` of a `link` bookmark to the conversation's bar (or inside `parent_id`).
    pub async fn bookmarks_add_link(
        &self,
        channel: &ChannelId,
        title: &str,
        link: &str,
    ) -> Result<()> {
        self.bookmarks_add_link_in_folder(channel, title, link, None)
            .await
    }

    /// `bookmarks.add` of a `link` bookmark optionally placed inside a folder (`parent_id`).
    pub async fn bookmarks_add_link_in_folder(
        &self,
        channel: &ChannelId,
        title: &str,
        link: &str,
        parent_id: Option<&str>,
    ) -> Result<()> {
        let mut params = vec![
            ("channel_id", channel.to_string()),
            ("title", title.to_owned()),
            ("type", "link".to_owned()),
            ("link", link.to_owned()),
        ];
        if let Some(parent) = parent_id.filter(|s| !s.is_empty()) {
            params.push(("parent_id", parent.to_owned()));
        }
        self.call::<Empty>(method::BOOKMARKS_ADD, params).await?;
        Ok(())
    }

    /// `bookmarks.remove`: delete a bookmark from `channel` (`F-CORE-062`).
    pub async fn bookmarks_remove(&self, channel: &ChannelId, bookmark_id: &str) -> Result<()> {
        let params = vec![
            ("channel_id", channel.to_string()),
            ("bookmark_id", bookmark_id.to_owned()),
        ];
        self.call::<Empty>(method::BOOKMARKS_REMOVE, params).await?;
        Ok(())
    }

    /// `drafts.list`: fetch the signed-in user's cloud-synced drafts (`F-CORE-089`).
    pub async fn drafts_list(&self) -> Result<Vec<CloudDraft>> {
        let params = vec![
            ("limit", "100".to_owned()),
            ("is_active", "true".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        let raw: Value = self.call(method::DRAFTS_LIST, params).await?.data;
        Ok(parse_cloud_drafts(&raw))
    }

    /// `drafts.create`: create a cloud-synced draft for `channel` and return its draft id.
    pub async fn drafts_create(&self, channel: &ChannelId, text: &str) -> Result<Option<String>> {
        let blocks = serde_json::json!([{
            "type": "rich_text",
            "elements": [{
                "type": "rich_text_section",
                "elements": [{ "type": "text", "text": text }]
            }]
        }]);
        let destinations = serde_json::json!([{ "channel_id": channel.as_str() }]);
        let params = vec![
            ("blocks", blocks.to_string()),
            ("destinations", destinations.to_string()),
            ("client_msg_id", client_token()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        let raw: Value = self.call(method::DRAFTS_CREATE, params).await?.data;
        Ok(raw
            .get("draft")
            .and_then(|d| d.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned))
    }

    /// `drafts.update`: update an existing cloud draft's text.
    pub async fn drafts_update(
        &self,
        draft_id: &str,
        channel: &ChannelId,
        text: &str,
    ) -> Result<()> {
        let blocks = serde_json::json!([{
            "type": "rich_text",
            "elements": [{
                "type": "rich_text_section",
                "elements": [{ "type": "text", "text": text }]
            }]
        }]);
        let destinations = serde_json::json!([{ "channel_id": channel.as_str() }]);
        let params = vec![
            ("draft_id", draft_id.to_owned()),
            ("blocks", blocks.to_string()),
            ("destinations", destinations.to_string()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::DRAFTS_UPDATE, params).await?;
        Ok(())
    }

    /// `drafts.delete`: remove a cloud draft once sent or cleared.
    pub async fn drafts_delete(&self, draft_id: &str) -> Result<()> {
        let params = vec![
            ("draft_id", draft_id.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::DRAFTS_DELETE, params).await?;
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

    /// `files.getUploadURLExternal`: reserve an upload slot for `filename` of `length` bytes.
    pub async fn files_get_upload_url_external(
        &self,
        filename: &str,
        length: usize,
    ) -> Result<UploadSlot> {
        let params = vec![
            ("filename", filename.to_owned()),
            ("length", length.to_string()),
        ];
        Ok(self
            .call::<UploadSlot>(method::FILES_GET_UPLOAD_U_R_L_EXTERNAL, params)
            .await?
            .data)
    }

    /// `files.completeUploadExternal`: finalize uploaded file IDs and share them to `channel`
    /// (optionally inside `thread_ts` and accompanied by `content`).
    pub async fn files_complete_upload_external(
        &self,
        files: &[(String, String)],
        channel: &ChannelId,
        thread_ts: Option<&Ts>,
        content: Option<&MessageContent>,
    ) -> Result<Vec<File>> {
        let entries: Vec<Value> = files
            .iter()
            .map(|(id, title)| serde_json::json!({ "id": id, "title": title }))
            .collect();
        let mut params = vec![
            ("files", serde_json::to_string(&entries)?),
            ("channel_id", channel.to_string()),
        ];
        if let Some(ts) = thread_ts {
            params.push(("thread_ts", ts.to_string()));
        }
        if let Some(content) = content {
            if !content.blocks.is_empty() {
                params.push(("blocks", serde_json::to_string(&content.blocks)?));
            } else if !content.text.trim().is_empty() {
                params.push(("initial_comment", content.text.clone()));
            }
        }
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default, deserialize_with = "lenient_vec")]
            files: Vec<File>,
        }
        Ok(self
            .call::<Wire>(method::FILES_COMPLETE_UPLOAD_EXTERNAL, params)
            .await?
            .data
            .files)
    }

    /// Full external upload v2 flow: reserve a slot per file, POST its raw bytes, and finalize
    /// all files in one `files.completeUploadExternal` call into `channel`.
    pub async fn upload_files(
        &self,
        channel: &ChannelId,
        thread_ts: Option<&Ts>,
        content: Option<&MessageContent>,
        files: &[UploadFile],
    ) -> Result<Vec<File>> {
        let mut completed = Vec::with_capacity(files.len());
        for file in files {
            if file.bytes.is_empty() {
                return Err(crate::ApiError::Slack("empty_file".to_owned()));
            }
            let slot = self
                .files_get_upload_url_external(&file.filename, file.bytes.len())
                .await?;
            self.post_upload_bytes(&slot.upload_url, &file.bytes)
                .await?;
            completed.push((slot.file_id, file.filename.clone()));
        }
        self.files_complete_upload_external(&completed, channel, thread_ts, content)
            .await
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
struct GifMediaFormat {
    #[serde(default)]
    url: String,
}

#[derive(Deserialize, Default)]
struct GifMediaFormats {
    gif: Option<GifMediaFormat>,
    tinygif: Option<GifMediaFormat>,
    tinygifpreview: Option<GifMediaFormat>,
    gifpreview: Option<GifMediaFormat>,
}

#[derive(Deserialize)]
struct WireGif {
    #[serde(default)]
    id: String,
    #[serde(default)]
    content_description: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    media_formats: GifMediaFormats,
}

#[derive(Deserialize)]
struct GifListData {
    #[serde(default)]
    gifs: Vec<WireGif>,
}

fn parse_gif_list(data: GifListData) -> Vec<GifItem> {
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

impl SlackApi {
    /// Featured/trending GIFs for Slack's emoji/GIF picker (`gif.featured`).
    pub async fn gif_featured(&self, limit: u32) -> Result<Vec<GifItem>> {
        let resp: ApiResponse<GifListData> = self
            .call(method::GIF_FEATURED, vec![("limit", limit.to_string())])
            .await?;
        Ok(parse_gif_list(resp.data))
    }

    /// Search GIFs for Slack's emoji/GIF picker (`gif.search`).
    pub async fn gif_search(&self, query: &str, limit: u32) -> Result<Vec<GifItem>> {
        let resp: ApiResponse<GifListData> = self
            .call(
                method::GIF_SEARCH,
                vec![("query", query.to_owned()), ("limit", limit.to_string())],
            )
            .await?;
        Ok(parse_gif_list(resp.data))
    }

    /// `saved.list`: items saved for Later under `filter` (`"saved"`, `"completed"`, `"archived"`).
    pub async fn saved_list(&self, filter: &str, limit: u32) -> Result<Vec<SavedItem>> {
        let resp: ApiResponse<SavedListWire> = self
            .call(
                method::SAVED_LIST,
                vec![
                    ("filter", filter.to_owned()),
                    ("limit", limit.to_string()),
                    ("include_tombstones", "true".to_owned()),
                    ("_x_reason", "saved-api/savedList".to_owned()),
                    ("_x_mode", MODE_ONLINE.to_owned()),
                ],
            )
            .await?;
        Ok(parse_saved_list(resp.data, filter))
    }

    /// `saved.add`: save a message for Later, optionally with a reminder due time (unix seconds).
    pub async fn saved_add(
        &self,
        channel: &ChannelId,
        ts: &Ts,
        date_due: Option<u64>,
    ) -> Result<()> {
        let mut params = vec![
            ("item_type", "message".to_owned()),
            ("item_id", channel.to_string()),
            ("ts", ts.to_string()),
            ("_x_reason", "saved-api/addSavedMessage".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(due) = date_due {
            params.push(("date_due", due.to_string()));
        }
        self.call::<Empty>(method::SAVED_ADD, params).await?;
        Ok(())
    }

    /// `saved.add`: save a file for Later (`item_type=file`, `F-CORE-157`).
    pub async fn saved_add_file(&self, file_id: &str, date_due: Option<u64>) -> Result<()> {
        let mut params = vec![
            ("item_type", "file".to_owned()),
            ("item_id", file_id.to_owned()),
            ("_x_reason", "saved-api/addSavedFile".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(due) = date_due {
            params.push(("date_due", due.to_string()));
        }
        self.call::<Empty>(method::SAVED_ADD, params).await?;
        Ok(())
    }

    /// `saved.update`: transition a saved item's state (`mark`) and/or update or clear (`Some(None)`)
    /// its reminder due time (`date_due`).
    pub async fn saved_update(
        &self,
        channel: &ChannelId,
        ts: &Ts,
        mark: Option<SavedMark>,
        date_due: Option<Option<u64>>,
    ) -> Result<()> {
        let mut params = vec![
            ("item_type", "message".to_owned()),
            ("item_id", channel.to_string()),
            ("ts", ts.to_string()),
            ("_x_reason", "saved-api/updateSavedMessage".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(mark) = mark {
            params.push(("mark", mark.as_str().to_owned()));
        }
        if let Some(due) = date_due {
            params.push(("date_due", due.unwrap_or(0).to_string()));
        }
        self.call::<Empty>(method::SAVED_UPDATE, params).await?;
        Ok(())
    }

    /// `saved.delete`: remove a message from Later.
    pub async fn saved_delete(&self, channel: &ChannelId, ts: &Ts) -> Result<()> {
        let params = vec![
            ("item_type", "message".to_owned()),
            ("item_id", channel.to_string()),
            ("ts", ts.to_string()),
            ("_x_reason", "saved-api/deleteSavedMessage".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::SAVED_DELETE, params).await?;
        Ok(())
    }

    /// `saved.clearCompleted`: clear all completed Later items.
    pub async fn saved_clear_completed(&self) -> Result<()> {
        let params = vec![
            ("_x_reason", "manually_marked_all_completed".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::SAVED_CLEAR_COMPLETED, params)
            .await?;
        Ok(())
    }

    /// `client.appCommands`: workspace slash command catalog (`core`, `app`, `service`).
    pub async fn client_app_commands(&self) -> Result<Vec<AppCommand>> {
        let params = vec![
            ("_x_reason", "dispatch-client-extras".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        let resp: ApiResponse<AppCommandsWire> =
            self.call(method::CLIENT_APP_COMMANDS, params).await?;
        Ok(parse_app_commands(resp.data))
    }

    /// Edge API `users/search`: fast typeahead lookup across workspace & org people for Cmd+K.
    pub async fn edge_users_search(
        &self,
        team_id: &TeamId,
        query: &str,
        count: u32,
    ) -> Result<Vec<User>> {
        let payload = serde_json::json!({
            "query": query,
            "count": count,
            "fuzz": 1,
            "include_profile_only_users": true,
            "enable_workspace_ranking": true,
            "search_email": true,
            "filter": "NOT deactivated",
            "default_workspace": team_id.as_str(),
        });
        let resp: ApiResponse<EdgeUsersWire> = self
            .call_edge_json(team_id.as_str(), "users/search", payload)
            .await?;
        Ok(resp.data.results)
    }

    /// Edge API `channels/search`: fast typeahead lookup across joined and unjoined channels for Cmd+K.
    pub async fn edge_channels_search(
        &self,
        team_id: &TeamId,
        query: &str,
        count: u32,
    ) -> Result<Vec<Conversation>> {
        let payload = serde_json::json!({
            "query": query,
            "count": count,
            "fuzz": 1,
            "filter": "xws",
            "include_record_channels": false,
            "check_membership": true,
            "default_workspace": team_id.as_str(),
        });
        let resp: ApiResponse<EdgeChannelsWire> = self
            .call_edge_json(team_id.as_str(), "channels/search", payload)
            .await?;
        let members: std::collections::HashSet<&ChannelId> =
            resp.data.member_channels.iter().collect();
        let mut results = resp.data.results;
        for ch in &mut results {
            if members.contains(&ch.id) {
                ch.is_member = true;
            }
        }
        Ok(results)
    }

    /// `search.modules.messages`: private web-client message search (`docs/research/api-search.md`).
    pub async fn search_modules_messages(
        &self,
        query: &str,
        sort: SearchSort,
        page: usize,
    ) -> Result<RemoteMessagePage> {
        let sort_key = match sort {
            SearchSort::Score => "score",
            SearchSort::Timestamp => "timestamp",
        };
        let params = vec![
            ("module", "messages".to_owned()),
            ("query", query.to_owned()),
            ("page", page.max(1).to_string()),
            ("count", "20".to_owned()),
            ("extracts", "1".to_owned()),
            ("highlight", "0".to_owned()),
            ("max_extract_len", "200".to_owned()),
            ("extra_message_data", "1".to_owned()),
            ("no_user_profile", "1".to_owned()),
            ("file_title_only", "false".to_owned()),
            ("query_rewrite_disabled", "false".to_owned()),
            ("include_files_shares", "1".to_owned()),
            ("search_context", "desktop_messages_tab".to_owned()),
            ("sort", sort_key.to_owned()),
            ("sort_dir", "desc".to_owned()),
            ("search_tab_filter", "messages".to_owned()),
            ("search_tab_sort", sort_key.to_owned()),
            ("_x_reason", "search-modules-messages".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
            ("_x_app_name", "client".to_owned()),
        ];
        let wire: SearchModulesMessagesWire =
            self.call("search.modules.messages", params).await?.data;
        Ok(parse_search_modules_messages(wire))
    }

    /// `search.modules.files`: private web-client file search (`docs/research/api-search.md`).
    pub async fn search_modules_files(
        &self,
        query: &str,
        sort: SearchSort,
        page: usize,
    ) -> Result<RemoteFilePage> {
        let sort_key = match sort {
            SearchSort::Score => "score",
            SearchSort::Timestamp => "timestamp",
        };
        let params = vec![
            ("module", "files".to_owned()),
            ("query", query.to_owned()),
            ("page", page.max(1).to_string()),
            ("count", "20".to_owned()),
            ("extracts", "1".to_owned()),
            ("highlight", "0".to_owned()),
            ("extra_message_data", "1".to_owned()),
            ("file_title_only", "false".to_owned()),
            ("query_rewrite_disabled", "false".to_owned()),
            ("include_files_shares", "1".to_owned()),
            ("search_context", "desktop_files_tab".to_owned()),
            ("sort", sort_key.to_owned()),
            ("sort_dir", "desc".to_owned()),
            ("search_tab_filter", "messages".to_owned()),
            ("search_tab_sort", sort_key.to_owned()),
            ("_x_reason", "search-modules-files".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
            ("_x_app_name", "client".to_owned()),
        ];
        let wire: SearchModulesFilesWire = self.call("search.modules.files", params).await?.data;
        Ok(parse_search_modules_files(wire))
    }

    /// `users.profile.set`: sparse update to the caller's profile or custom status.
    pub async fn users_profile_set(
        &self,
        update: &ProfileUpdate,
    ) -> Result<slackless_types::Profile> {
        let profile_json = serde_json::to_string(update)
            .map_err(|e| crate::ApiError::Slack(format!("invalid profile update payload: {e}")))?;
        let params = vec![
            ("profile", profile_json),
            ("_x_reason", "profile-edit".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        let resp: ProfileSetWire = self.call("users.profile.set", params).await?.data;
        Ok(resp.profile)
    }

    /// `presence.set`: private web-client manual presence toggle (`"active"` or `"away"`).
    pub async fn presence_set(&self, away: bool) -> Result<()> {
        let presence = if away { "away" } else { "active" };
        let params = vec![
            ("presence", presence.to_owned()),
            ("_x_reason", "manual-presence".to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>("presence.set", params).await?;
        Ok(())
    }

    /// `users.prefs.set`: write a single user preference key/value pair back to Slack.
    pub async fn users_prefs_set(&self, name: &str, value: &str) -> Result<()> {
        let params = vec![
            ("name", name.to_owned()),
            ("value", value.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>("users.prefs.set", params).await?;
        Ok(())
    }
}

/// One cloud-synced draft returned by `drafts.list` (`F-CORE-089`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudDraft {
    pub id: String,
    pub channel: ChannelId,
    pub text: String,
    pub updated_secs: Option<u64>,
}

fn parse_cloud_drafts(raw: &Value) -> Vec<CloudDraft> {
    fn extract_text(blocks: &Value) -> String {
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
    raw["drafts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            if d["is_deleted"].as_bool().unwrap_or(false) || d["is_sent"].as_bool().unwrap_or(false)
            {
                return None;
            }
            let id = d["id"].as_str()?.to_owned();
            let channel = d["destinations"]
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|dest| dest["channel_id"].as_str())
                .map(ChannelId::from)?;
            let text = extract_text(&d["blocks"]);
            if text.trim().is_empty() {
                return None;
            }
            let updated_secs = d["date_last_updated"]
                .as_u64()
                .or_else(|| d["date_created"].as_u64());
            Some(CloudDraft {
                id,
                channel,
                text,
                updated_secs,
            })
        })
        .collect()
}

/// One message hit returned by `search.modules.messages`.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteMessageHit {
    pub channel: ChannelId,
    pub channel_name: Option<String>,
    pub message: Message,
}

/// One page of remote message search hits.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RemoteMessagePage {
    pub hits: Vec<RemoteMessageHit>,
    pub total: usize,
    pub page: usize,
    pub page_count: usize,
}

/// One page of remote file search hits.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RemoteFilePage {
    pub files: Vec<File>,
    pub total: usize,
    pub page: usize,
    pub page_count: usize,
}

/// Sparse payload for `users.profile.set`: only `Some` fields are serialized.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ProfileUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_emoji: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_expiration: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub real_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Value>,
}

#[derive(Deserialize, Default)]
struct ProfileSetWire {
    #[serde(default)]
    profile: slackless_types::Profile,
}

#[derive(Deserialize, Default)]
struct ModulePaginationWire {
    #[serde(default)]
    total_count: usize,
    #[serde(default)]
    page: usize,
    #[serde(default)]
    page_count: usize,
}

#[derive(Deserialize, Default)]
struct SearchModuleChannelWire {
    #[serde(default)]
    id: ChannelId,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize, Default)]
struct SearchModuleMessageGroupWire {
    #[serde(default)]
    channel: SearchModuleChannelWire,
    #[serde(default, deserialize_with = "lenient_vec")]
    messages: Vec<Message>,
}

#[derive(Deserialize, Default)]
struct SearchModulesMessagesWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    items: Vec<SearchModuleMessageGroupWire>,
    #[serde(default)]
    pagination: ModulePaginationWire,
}

#[derive(Deserialize, Default)]
struct SearchModulesFilesWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    items: Vec<File>,
    #[serde(default)]
    pagination: ModulePaginationWire,
}

fn strip_highlight_markers(s: &str) -> String {
    s.chars()
        .filter(|&c| c != '\u{e000}' && c != '\u{e001}')
        .collect()
}

fn parse_search_modules_messages(wire: SearchModulesMessagesWire) -> RemoteMessagePage {
    let mut hits = Vec::new();
    for group in wire.items {
        if group.channel.id.as_str().is_empty() {
            continue;
        }
        let channel_name = group.channel.name.filter(|s| !s.is_empty());
        for mut message in group.messages {
            if message.text.contains('\u{e000}') || message.text.contains('\u{e001}') {
                message.text = strip_highlight_markers(&message.text);
            }
            hits.push(RemoteMessageHit {
                channel: group.channel.id.clone(),
                channel_name: channel_name.clone(),
                message,
            });
        }
    }
    RemoteMessagePage {
        hits,
        total: wire.pagination.total_count,
        page: wire.pagination.page.max(1),
        page_count: wire.pagination.page_count,
    }
}

fn parse_search_modules_files(wire: SearchModulesFilesWire) -> RemoteFilePage {
    RemoteFilePage {
        files: wire.items,
        total: wire.pagination.total_count,
        page: wire.pagination.page.max(1),
        page_count: wire.pagination.page_count,
    }
}

/// One workspace slash command returned by `client.appCommands` (`core`, `app`, or `service`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppCommand {
    /// Name without leading `/`, lowercased (e.g. `"jira"`, `"linear"`, `"giphy"`).
    pub name: String,
    /// Argument hint (e.g. `"[issue key]"`).
    pub usage: String,
    /// Description shown below the command in `/` autocomplete.
    pub description: String,
    /// Owning app or service name (e.g. `"Jira Cloud"`, `"Giphy"`).
    pub app_name: Option<String>,
    /// Owning app id (`A...`) for app-provided commands.
    pub app_id: Option<String>,
}

/// Combined remote results for the Cmd+K quick switcher (`edgeapi` `users/search` + `channels/search`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EdgeSearchResults {
    pub users: Vec<User>,
    pub channels: Vec<Conversation>,
}

#[derive(Deserialize)]
struct WireAppCommand {
    #[serde(default)]
    name: String,
    #[serde(default)]
    desc: String,
    #[serde(default)]
    usage: String,
    #[serde(default)]
    app: Option<String>,
    #[serde(default)]
    app_name: Option<String>,
    #[serde(default)]
    service_name: Option<String>,
    #[serde(default)]
    alias_of: Option<String>,
}

#[derive(Deserialize, Default)]
struct AppCommandsWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    commands: Vec<WireAppCommand>,
}

fn parse_app_commands(wire: AppCommandsWire) -> Vec<AppCommand> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for cmd in wire.commands {
        let name = cmd.name.trim().trim_start_matches('/').to_lowercase();
        if name.is_empty() || !seen.insert(name.clone()) {
            continue;
        }
        let app_name = cmd
            .app_name
            .filter(|s| !s.is_empty())
            .or_else(|| cmd.service_name.filter(|s| !s.is_empty()));
        let description = if !cmd.desc.trim().is_empty() {
            cmd.desc.trim().to_owned()
        } else if let Some(alias) = cmd.alias_of.filter(|s| !s.is_empty()) {
            format!("Alias for {alias}")
        } else {
            String::new()
        };
        out.push(AppCommand {
            name,
            usage: cmd.usage.trim().to_owned(),
            description,
            app_name,
            app_id: cmd.app.filter(|s| !s.is_empty()),
        });
    }
    out
}

#[derive(Deserialize, Default)]
struct EdgeUsersWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    results: Vec<User>,
}

#[derive(Deserialize, Default)]
struct EdgeChannelsWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    results: Vec<Conversation>,
    #[serde(default, deserialize_with = "or_default")]
    member_channels: Vec<ChannelId>,
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
struct WireSavedItem {
    #[serde(default)]
    item_id: String,
    #[serde(default)]
    item_type: String,
    #[serde(default)]
    ts: String,
    #[serde(default)]
    date_created: u64,
    #[serde(default)]
    date_due: u64,
    #[serde(default)]
    date_completed: u64,
    #[serde(default, deserialize_with = "lenient_bool")]
    is_archived: bool,
    #[serde(default)]
    state: String,
    #[serde(default)]
    todo_state: String,
}

#[derive(Deserialize, Default)]
struct SavedListWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    saved_items: Vec<WireSavedItem>,
    #[serde(default, deserialize_with = "lenient_vec")]
    items: Vec<WireSavedItem>,
}

fn parse_saved_list(wire: SavedListWire, filter: &str) -> Vec<SavedItem> {
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

    #[test]
    fn parses_gif_featured_and_search_wire_payload() {
        let wire: GifListData = serde_json::from_value(serde_json::json!({
            "gifs": [
                {
                    "id": "R7m04yMaGWVeE",
                    "content_description": "Will Smith Mib GIF",
                    "url": "https://giphy.com/gifs/will-smith-R7m04yMaGWVeE",
                    "media_formats": {
                        "tinygif": { "url": "https://media2.giphy.com/media/R7m04yMaGWVeE/200w.webp" },
                        "tinygifpreview": { "url": "https://media2.giphy.com/media/R7m04yMaGWVeE/200w_s.gif" },
                        "gif": { "url": "https://media2.giphy.com/media/R7m04yMaGWVeE/giphy.webp" }
                    }
                },
                {
                    "id": "empty_should_skip",
                    "content_description": "",
                    "url": "",
                    "media_formats": {}
                }
            ]
        }))
        .unwrap();
        let parsed = parse_gif_list(wire);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "R7m04yMaGWVeE");
        assert_eq!(parsed[0].title, "Will Smith Mib GIF");
        assert_eq!(
            parsed[0].still_url,
            "https://media2.giphy.com/media/R7m04yMaGWVeE/200w_s.gif"
        );
        assert_eq!(
            parsed[0].preview_url,
            "https://media2.giphy.com/media/R7m04yMaGWVeE/200w.webp"
        );
        assert_eq!(
            parsed[0].url,
            "https://giphy.com/gifs/will-smith-R7m04yMaGWVeE"
        );
        let attachment = parsed[0].to_attachment();
        assert_eq!(
            serde_json::to_value(&attachment).unwrap(),
            serde_json::json!({
                "fallback": "shared a GIF",
                "blocks": [{
                    "type": "image",
                    "image_url": "https://media2.giphy.com/media/R7m04yMaGWVeE/200w.webp",
                    "alt_text": "Will Smith Mib GIF",
                    "title": {
                        "type": "plain_text",
                        "text": "GIF",
                        "emoji": true
                    }
                }]
            })
        );
    }

    #[test]
    fn parses_saved_list_wire_payload() {
        let wire: SavedListWire = serde_json::from_value(serde_json::json!({
            "saved_items": [
                {
                    "item_id": "C01",
                    "item_type": "message",
                    "ts": "1700000001.000100",
                    "date_created": 1700000010,
                    "date_due": 1700003600,
                    "date_completed": 0,
                    "is_archived": false,
                    "todo_state": "to_do"
                },
                {
                    "item_id": "C02",
                    "item_type": "message",
                    "ts": "1700000002.000200",
                    "date_completed": 1700001000,
                    "is_archived": false,
                    "todo_state": "completed"
                },
                {
                    "item_id": "C03",
                    "item_type": "message",
                    "ts": "1700000003.000300",
                    "is_archived": true
                },
                {
                    "item_id": "F04",
                    "item_type": "file",
                    "ts": ""
                }
            ]
        }))
        .unwrap();
        let parsed = parse_saved_list(wire, "saved");
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].channel.as_str(), "C01");
        assert_eq!(parsed[0].state, SavedItemState::InProgress);
        assert_eq!(parsed[0].date_due, Some(1700003600));
        assert_eq!(parsed[1].state, SavedItemState::Completed);
        assert_eq!(parsed[1].date_due, None);
        assert_eq!(parsed[2].state, SavedItemState::Archived);
    }

    #[test]
    fn parses_client_app_commands_wire_payload() {
        let wire: AppCommandsWire = serde_json::from_value(serde_json::json!({
            "commands": [
                {
                    "name": "/linear",
                    "type": "app",
                    "app": "AEMQ3Q4F4",
                    "app_name": "Linear",
                    "desc": "Create new issue on Linear",
                    "usage": "issue title (optional) "
                },
                {
                    "name": "/giphy",
                    "type": "service",
                    "service_name": "Giphy",
                    "desc": "Post a random gif to channel",
                    "usage": "[text]"
                },
                {
                    "name": "/dm",
                    "alias_of": "/msg",
                    "type": "core"
                }
            ]
        }))
        .unwrap();
        let parsed = parse_app_commands(wire);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].name, "linear");
        assert_eq!(parsed[0].app_name.as_deref(), Some("Linear"));
        assert_eq!(parsed[0].usage, "issue title (optional)");
        assert_eq!(parsed[1].app_name.as_deref(), Some("Giphy"));
        assert_eq!(parsed[2].description, "Alias for /msg");
    }

    #[test]
    fn parses_search_modules_messages_and_files_and_sparse_profile_update() {
        let msg_wire: SearchModulesMessagesWire = serde_json::from_value(serde_json::json!({
            "pagination": { "total_count": 42, "page": 1, "page_count": 3 },
            "items": [
                {
                    "channel": { "id": "C01", "name": "general" },
                    "messages": [
                        { "ts": "1700000001.000100", "user": "U01", "text": "hello \u{e000}deploy\u{e001} ok" }
                    ]
                }
            ]
        }))
        .unwrap();
        let page = parse_search_modules_messages(msg_wire);
        assert_eq!(page.total, 42);
        assert_eq!(page.page_count, 3);
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].channel.as_str(), "C01");
        assert_eq!(page.hits[0].channel_name.as_deref(), Some("general"));
        assert_eq!(page.hits[0].message.text, "hello deploy ok");

        let file_wire: SearchModulesFilesWire = serde_json::from_value(serde_json::json!({
            "pagination": { "total_count": 1, "page": 1, "page_count": 1 },
            "items": [
                { "id": "F01", "name": "plan.pdf", "title": "Q4 Plan", "mimetype": "application/pdf", "size": 1024 }
            ]
        }))
        .unwrap();
        let files = parse_search_modules_files(file_wire);
        assert_eq!(files.total, 1);
        assert_eq!(files.files.len(), 1);
        assert_eq!(files.files[0].id.as_str(), "F01");

        let update = ProfileUpdate {
            status_text: Some("In a meeting".into()),
            status_emoji: Some(":calendar:".into()),
            status_expiration: Some(1_700_003_600),
            ..Default::default()
        };
        let v = serde_json::to_value(&update).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "status_text": "In a meeting",
                "status_emoji": ":calendar:",
                "status_expiration": 1_700_003_600
            })
        );
    }
}
