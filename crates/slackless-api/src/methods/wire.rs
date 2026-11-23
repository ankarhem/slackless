//! Request options, response DTOs, and internal wire structs / parsers for Slack endpoints.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::Result;
use crate::client::Params;
use crate::de::{lenient_bool, lenient_u32, lenient_vec, null_default, or_default};
use crate::pagination::Page;
use crate::types::{
    ApiResponse, Attachment, Block, BotId, ChannelId, ChannelSection, Conversation,
    ConversationKind, File, Latest, Message, RichTextBlock, SectionKind, Team, TeamId, Ts, User,
    UserId,
};

/// `_x_reason` the web client tags message sends with.
pub(super) const REASON_MESSAGE_SEND: &str = "webapp_message_send";
/// `_x_reason` for reaction toggles.
pub(super) const REASON_REACTION: &str = "changeReactionFromUserAction";
/// `_x_mode` the web client sends on writes.
pub(super) const MODE_ONLINE: &str = "online";
/// `_x_reason` for slash commands Slack runs.
pub(super) const REASON_COMMAND: &str = "executeCommand";
/// `_x_reason` for Block Kit interactions.
pub(super) const REASON_BLOCK_ACTION: &str = "dispatch_action_to_developer";

/// The web client's per-request `client_token`: `web-<unix ms>`.
pub(super) fn client_token() -> String {
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
    pub(super) fn json(&self) -> Value {
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
    pub(super) fn push(&self, params: &mut Params) {
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

    pub(super) fn push(&self, params: &mut Params) -> Result<()> {
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
pub(super) struct ScheduledMessagesList {
    #[serde(default)]
    pub(super) scheduled_messages: Vec<ScheduledMessage>,
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
pub(super) struct MessagesWire {
    #[serde(default, deserialize_with = "null_default")]
    pub(super) messages: Vec<Message>,
}

#[derive(Deserialize)]
pub(super) struct ChannelWire {
    pub(super) channel: Conversation,
}

#[derive(Deserialize)]
pub(super) struct TeamWire {
    pub(super) team: Team,
}

#[derive(Deserialize)]
pub(super) struct ChannelsWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) channels: Vec<Conversation>,
}

#[derive(Deserialize)]
pub(super) struct UserWire {
    pub(super) user: User,
}

#[derive(Deserialize)]
pub(super) struct MembersWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) members: Vec<User>,
}

/// `conversations.members` returns bare member ids.
#[derive(Deserialize)]
pub(super) struct MemberIdsWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) members: Vec<UserId>,
}

/// `admin.roles.entity.listAssignments`: who holds which role on one entity.
#[derive(Deserialize)]
pub(super) struct RoleAssignmentsWire {
    #[serde(default)]
    pub(super) role_assignments: Vec<RoleAssignmentWire>,
}

#[derive(Deserialize)]
pub(super) struct RoleAssignmentWire {
    pub(super) role_id: String,
    #[serde(default)]
    pub(super) users: Vec<UserId>,
}

/// Slack's channel-manager role, as its client reads it from
/// `admin.roles.entity.listAssignments`.
pub(super) const CHANNEL_MANAGER_ROLE: &str = "Rl0A";

/// `client.dms` answers one DMs-tab row per conversation: the `channel` object with the
/// row's bare `latest` duplicated beside it.
#[derive(Deserialize)]
pub(super) struct DmWire {
    #[serde(default)]
    pub(super) id: ChannelId,
    #[serde(default, deserialize_with = "or_default")]
    pub(super) latest: Option<Ts>,
    #[serde(default, deserialize_with = "or_default")]
    pub(super) channel: Option<Conversation>,
}

impl DmWire {
    /// The row's conversation: the nested object when Slack sent one, else the bare minimum
    /// (`id` + `latest`) the row carries on its own.
    pub(super) fn into_conversation(self) -> Conversation {
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
pub(super) struct DmsWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) ims: Vec<DmWire>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) mpims: Vec<DmWire>,
}

#[derive(Deserialize)]
pub(super) struct SearchWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(super) messages: SearchMessages,
}

#[derive(Deserialize)]
pub(super) struct EmojiWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(super) emoji: HashMap<String, String>,
}

#[derive(Deserialize)]
pub(super) struct ChannelSectionsWire {
    /// Pagination cursor, top-level on this method; empty string on the last page.
    #[serde(default, deserialize_with = "or_default")]
    pub(super) cursor: Option<String>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) channel_sections: Vec<SectionWire>,
}

#[derive(Deserialize)]
pub(super) struct SectionWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(super) channel_section_id: String,
    #[serde(default, deserialize_with = "or_default")]
    pub(super) name: String,
    #[serde(rename = "type", default, deserialize_with = "or_default")]
    pub(super) kind: Option<SectionKind>,
    #[serde(default, deserialize_with = "or_default")]
    pub(super) emoji: String,
    /// The next section in display order; empty string when this is the last one.
    #[serde(default, deserialize_with = "or_default")]
    pub(super) next_channel_section_id: String,
    #[serde(default, deserialize_with = "or_default")]
    pub(super) channel_ids_page: IdsPageWire,
}

#[derive(Default, Deserialize)]
pub(super) struct IdsPageWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(super) channel_ids: Vec<ChannelId>,
}

/// One `activity.feed` entry as Slack sends it: the read-state envelope around a payload
/// tagged with its `type`. Unknown tags fail the tag parse, so `lenient_vec` skips the
/// whole entry.
#[derive(Deserialize)]
pub(super) struct ActivityItemWire {
    #[serde(rename = "is_unread", default, deserialize_with = "lenient_bool")]
    pub(super) unread: bool,
    #[serde(default)]
    pub(super) feed_ts: Ts,
    #[serde(default)]
    pub(super) key: String,
    #[serde(default)]
    pub(super) item: ActivityPayloadWire,
}

#[derive(Deserialize, Default)]
#[serde(tag = "type")]
pub(super) enum ActivityPayloadWire {
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
pub(super) struct ActivityRefWire {
    #[serde(default)]
    pub(super) channel: ChannelId,
    #[serde(default)]
    pub(super) ts: Ts,
    #[serde(rename = "author_user_id", default)]
    pub(super) author: Option<UserId>,
    #[serde(default)]
    pub(super) thread_ts: Option<Ts>,
}

/// `thread_v2`'s `bundle_info`: the unread pill plus the nested `payload.thread_entry`.
#[derive(Deserialize, Default)]
pub(super) struct ActivityThreadBundleWire {
    #[serde(default, deserialize_with = "lenient_u32")]
    pub(super) unread_count: u32,
    #[serde(default, deserialize_with = "or_default")]
    pub(super) payload: ActivityThreadPayloadWire,
}

#[derive(Deserialize, Default)]
pub(super) struct ActivityThreadPayloadWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(super) thread_entry: Option<ActivityThreadEntryWire>,
}

#[derive(Deserialize, Default)]
pub(super) struct ActivityThreadEntryWire {
    #[serde(rename = "channel_id", default)]
    pub(super) channel: ChannelId,
    #[serde(default)]
    pub(super) thread_ts: Ts,
    #[serde(default)]
    pub(super) latest_ts: Ts,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub(super) unread_msg_count: u32,
}

/// `message_reaction`'s `reaction`.
#[derive(Deserialize, Default)]
pub(super) struct ActivityReactionWire {
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) user: Option<UserId>,
}

/// `dm` / `bot_dm_bundle` / `channel`'s `bundle_info`: the newest message under `payload`
/// (keyed by the payload's own entry name) plus the bundle's unread count.
#[derive(Deserialize, Default)]
pub(super) struct ActivityMessageBundleWire {
    #[serde(default, deserialize_with = "lenient_u32")]
    pub(super) unread_count: u32,
    #[serde(default, deserialize_with = "or_default")]
    pub(super) payload: ActivityPayloadEntryWire,
}

#[derive(Deserialize, Default)]
pub(super) struct ActivityPayloadEntryWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(super) dm_entry: Option<ActivityLatestWire>,
    #[serde(default, deserialize_with = "or_default")]
    pub(super) channel_entry: Option<ActivityLatestWire>,
    #[serde(default, deserialize_with = "or_default")]
    pub(super) message: Option<Message>,
}

/// An entry whose `latest_message` is a full message object (the channel id rides in its
/// `extra`, as with every `Message`).
#[derive(Deserialize, Default)]
pub(super) struct ActivityLatestWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(super) latest_message: Option<Message>,
}

/// `messages.list`: the bodies of the asked-for messages, by channel.
#[derive(Deserialize, Default)]
pub(super) struct MessagesListWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(super) messages_data: HashMap<String, MessagesChannelWire>,
}

#[derive(Deserialize, Default)]
pub(super) struct MessagesChannelWire {
    #[serde(default, deserialize_with = "null_default")]
    pub(super) messages: Vec<Message>,
}

#[derive(Deserialize)]
pub(super) struct FilesWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) files: Vec<File>,
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
pub(super) fn merge_section(sections: &mut Vec<ChannelSection>, section: ChannelSection) {
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

pub(super) fn page<W, T>(resp: ApiResponse<W>, items: impl FnOnce(W) -> Vec<T>) -> Page<T> {
    let next_cursor = resp.next_cursor().map(str::to_owned);
    Page {
        items: items(resp.data),
        next_cursor,
    }
}

pub(super) fn kind_wire(kind: ConversationKind) -> &'static str {
    match kind {
        ConversationKind::PublicChannel => "public_channel",
        ConversationKind::PrivateChannel => "private_channel",
        ConversationKind::Im => "im",
        ConversationKind::Mpim => "mpim",
    }
}

/// The shared `types`/`exclude_archived`/`limit`/`cursor` params of the conversation listing
/// methods (`conversations.list`, `users.conversations`).
pub(super) fn listing_params(opts: &ListOpts, cursor: Option<String>) -> Params {
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

/// `activity.feed`'s envelope.
#[derive(Deserialize)]
pub(super) struct ActivityFeedWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) items: Vec<ActivityItemWire>,
}

/// Item types the feed is asked for: every category slackless models, minus
/// `unjoined_channel_mention`, which this workspace rejects with `invalid_arguments`
/// (probed live; Slack's fuller bootstrap list adds only types the view skips anyway).
pub(super) const ACTIVITY_TYPES: &str = concat!(
    "at_user,at_user_group,at_channel,at_everyone,keyword,",
    "thread_v2,message_reaction,dm,bot_dm_bundle,channel"
);

/// Channels per `messages.list` call before Slack answers `too_many_channels`
/// (docs/research/api-activity.md, `messages.list`).
pub(super) const MESSAGES_LIST_CHANNELS: usize = 8;

/// Normalizes one wire entry into the row the view renders; `None` for the shapes slackless
/// does not model (`Unknown`) or cannot place (no channel or message ts).
pub(super) fn row(wire: ActivityItemWire) -> Option<ActivityItem> {
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
pub(super) fn message_row(
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
pub(super) struct GifMediaFormat {
    #[serde(default)]
    pub(super) url: String,
}

#[derive(Deserialize, Default)]
pub(super) struct GifMediaFormats {
    pub(super) gif: Option<GifMediaFormat>,
    pub(super) tinygif: Option<GifMediaFormat>,
    pub(super) tinygifpreview: Option<GifMediaFormat>,
    pub(super) gifpreview: Option<GifMediaFormat>,
}

#[derive(Deserialize)]
pub(super) struct WireGif {
    #[serde(default)]
    pub(super) id: String,
    #[serde(default)]
    pub(super) content_description: String,
    #[serde(default)]
    pub(super) url: String,
    #[serde(default)]
    pub(super) media_formats: GifMediaFormats,
}

#[derive(Deserialize)]
pub(super) struct GifListData {
    #[serde(default)]
    pub(super) gifs: Vec<WireGif>,
}

pub(super) fn parse_gif_list(data: GifListData) -> Vec<GifItem> {
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

pub(super) fn parse_cloud_drafts(raw: &Value) -> Vec<CloudDraft> {
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
pub(super) struct ProfileSetWire {
    #[serde(default)]
    pub(super) profile: slackless_types::Profile,
}

#[derive(Deserialize, Default)]
pub(super) struct ModulePaginationWire {
    #[serde(default)]
    pub(super) total_count: usize,
    #[serde(default)]
    pub(super) page: usize,
    #[serde(default)]
    pub(super) page_count: usize,
}

#[derive(Deserialize, Default)]
pub(super) struct SearchModuleChannelWire {
    #[serde(default)]
    pub(super) id: ChannelId,
    #[serde(default)]
    pub(super) name: Option<String>,
}

#[derive(Deserialize, Default)]
pub(super) struct SearchModuleMessageGroupWire {
    #[serde(default)]
    pub(super) channel: SearchModuleChannelWire,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) messages: Vec<Message>,
}

#[derive(Deserialize, Default)]
pub(super) struct SearchModulesMessagesWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) items: Vec<SearchModuleMessageGroupWire>,
    #[serde(default)]
    pub(super) pagination: ModulePaginationWire,
}

#[derive(Deserialize, Default)]
pub(super) struct SearchModulesFilesWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) items: Vec<File>,
    #[serde(default)]
    pub(super) pagination: ModulePaginationWire,
}

pub(super) fn strip_highlight_markers(s: &str) -> String {
    s.chars()
        .filter(|&c| c != '\u{e000}' && c != '\u{e001}')
        .collect()
}

pub(super) fn parse_search_modules_messages(wire: SearchModulesMessagesWire) -> RemoteMessagePage {
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

pub(super) fn parse_search_modules_files(wire: SearchModulesFilesWire) -> RemoteFilePage {
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
pub(super) struct WireAppCommand {
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) desc: String,
    #[serde(default)]
    pub(super) usage: String,
    #[serde(default)]
    pub(super) app: Option<String>,
    #[serde(default)]
    pub(super) app_name: Option<String>,
    #[serde(default)]
    pub(super) service_name: Option<String>,
    #[serde(default)]
    pub(super) alias_of: Option<String>,
}

#[derive(Deserialize, Default)]
pub(super) struct AppCommandsWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) commands: Vec<WireAppCommand>,
}

pub(super) fn parse_app_commands(wire: AppCommandsWire) -> Vec<AppCommand> {
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
pub(super) struct EdgeUsersWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) results: Vec<User>,
}

#[derive(Deserialize, Default)]
pub(super) struct EdgeChannelsWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) results: Vec<Conversation>,
    #[serde(default, deserialize_with = "or_default")]
    pub(super) member_channels: Vec<ChannelId>,
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
pub(super) struct WireSavedItem {
    #[serde(default)]
    pub(super) item_id: String,
    #[serde(default)]
    pub(super) item_type: String,
    #[serde(default)]
    pub(super) ts: String,
    #[serde(default)]
    pub(super) date_created: u64,
    #[serde(default)]
    pub(super) date_due: u64,
    #[serde(default)]
    pub(super) date_completed: u64,
    #[serde(default, deserialize_with = "lenient_bool")]
    pub(super) is_archived: bool,
    #[serde(default)]
    pub(super) state: String,
    #[serde(default)]
    pub(super) todo_state: String,
}

#[derive(Deserialize, Default)]
pub(super) struct SavedListWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) saved_items: Vec<WireSavedItem>,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) items: Vec<WireSavedItem>,
}

pub(super) fn parse_saved_list(wire: SavedListWire, filter: &str) -> Vec<SavedItem> {
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

// ---------------------------------------------------------------------------
// Live Slack Lists (`files.info` + `lists.records.list` / `slackLists.items.list`)
// ---------------------------------------------------------------------------

/// One select choice option on a Slack List select column.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlackListChoice {
    pub value: String,
    pub label: String,
    pub color: String,
}

/// One column in a Slack List schema (`list_metadata.schema`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlackListColumn {
    pub id: String,
    pub key: String,
    pub name: String,
    pub kind: String,
    pub is_primary: bool,
    pub choices: Vec<SlackListChoice>,
}

/// Typed cell value inside a Slack List row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SlackListCellValue {
    #[default]
    Empty,
    Text(String),
    Users(Vec<UserId>),
    Select(Vec<String>),
    Date(String),
    Checkbox(bool),
    Number(String),
    Link {
        label: String,
        url: String,
    },
}

/// One record/row of a Slack List.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlackListRow {
    pub id: String,
    pub cells: HashMap<String, SlackListCellValue>,
}

/// A parsed Slack List ready for native read-only table rendering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlackListData {
    pub file_id: String,
    pub title: String,
    pub description: String,
    pub columns: Vec<SlackListColumn>,
    pub rows: Vec<SlackListRow>,
}

/// Parse `list_metadata.schema` from `files.info` (or `slackLists.items.list`'s `list` object).
pub(super) fn parse_list_schema(list_obj: &Value) -> (String, String, Vec<SlackListColumn>) {
    let title = list_obj
        .get("title")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| list_obj.get("name").and_then(Value::as_str))
        .unwrap_or("List")
        .to_owned();
    let meta = list_obj.get("list_metadata");
    let description = meta
        .and_then(|m| m.get("description"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let mut columns = Vec::new();
    if let Some(schema) = meta.and_then(|m| m.get("schema")).and_then(Value::as_array) {
        for col in schema {
            let id = col
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let key = col
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if id.is_empty() && key.is_empty() {
                continue;
            }
            let name = col
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| if !key.is_empty() { &key } else { &id })
                .to_owned();
            let kind = col
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("text")
                .to_owned();
            let is_primary = col
                .get("is_primary_column")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let mut choices = Vec::new();
            if let Some(arr) = col
                .get("options")
                .and_then(|o| o.get("choices"))
                .and_then(Value::as_array)
            {
                for ch in arr {
                    let value = ch
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let label = ch
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or(&value)
                        .to_owned();
                    let color = ch
                        .get("color")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    if !value.is_empty() || !label.is_empty() {
                        choices.push(SlackListChoice {
                            value,
                            label,
                            color,
                        });
                    }
                }
            }
            columns.push(SlackListColumn {
                id: if id.is_empty() { key.clone() } else { id },
                key,
                name,
                kind,
                is_primary,
                choices,
            });
        }
    }
    (title, description, columns)
}

/// Extract plain text from a Slack List cell's `rich_text` JSON array when `text` is absent.
fn list_rich_text_plain(val: &Value) -> String {
    fn walk(v: &Value, out: &mut String) {
        match v {
            Value::Array(arr) => {
                for item in arr {
                    walk(item, out);
                }
            }
            Value::Object(map) => {
                if let Some(t) = map.get("text").and_then(Value::as_str) {
                    out.push_str(t);
                } else if let Some(u) = map.get("url").and_then(Value::as_str) {
                    out.push_str(u);
                }
                if let Some(elems) = map.get("elements") {
                    walk(elems, out);
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    walk(val, &mut out);
    out.trim().to_owned()
}

/// Parse `records` (`lists.records.list`) or `items` (`slackLists.items.list`) into typed rows.
pub(super) fn parse_list_records(records: &[Value]) -> Vec<SlackListRow> {
    let mut rows = Vec::new();
    for rec in records {
        if rec
            .get("archived")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        let id = rec
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mut cells = HashMap::new();
        if let Some(fields) = rec.get("fields").and_then(Value::as_array) {
            for field in fields {
                let col_key = field
                    .get("column_id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .or_else(|| field.get("key").and_then(Value::as_str))
                    .unwrap_or_default();
                if col_key.is_empty() {
                    continue;
                }
                let cell = parse_list_cell(field);
                if cell != SlackListCellValue::Empty {
                    cells.insert(col_key.to_owned(), cell.clone());
                    if let Some(key) = field.get("key").and_then(Value::as_str) {
                        if !key.is_empty() && key != col_key {
                            cells.insert(key.to_owned(), cell);
                        }
                    }
                }
            }
        }
        rows.push(SlackListRow { id, cells });
    }
    rows
}

fn parse_list_cell(field: &Value) -> SlackListCellValue {
    if let Some(b) = field.get("checkbox").and_then(Value::as_bool) {
        return SlackListCellValue::Checkbox(b);
    }
    if let Some(arr) = field.get("checkbox").and_then(Value::as_array) {
        if let Some(b) = arr.first().and_then(Value::as_bool) {
            return SlackListCellValue::Checkbox(b);
        }
    }
    if let Some(arr) = field.get("user").and_then(Value::as_array) {
        let users: Vec<UserId> = arr
            .iter()
            .filter_map(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(UserId::from)
            .collect();
        if !users.is_empty() {
            return SlackListCellValue::Users(users);
        }
    }
    if let Some(arr) = field.get("select").and_then(Value::as_array) {
        let opts: Vec<String> = arr
            .iter()
            .filter_map(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if !opts.is_empty() {
            return SlackListCellValue::Select(opts);
        }
    }
    if let Some(arr) = field.get("date").and_then(Value::as_array) {
        if let Some(d) = arr
            .first()
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            return SlackListCellValue::Date(d.to_owned());
        }
    }
    if let Some(arr) = field.get("link").and_then(Value::as_array) {
        if let Some(first) = arr.first() {
            let url = first
                .get("originalUrl")
                .or_else(|| first.get("original_url"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !url.is_empty() {
                let label = first
                    .get("displayName")
                    .or_else(|| first.get("display_name"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(url);
                return SlackListCellValue::Link {
                    label: label.to_owned(),
                    url: url.to_owned(),
                };
            }
        }
    }
    if let Some(arr) = field.get("number").and_then(Value::as_array) {
        if let Some(n) = arr.first() {
            let s = n
                .as_f64()
                .map(|v| {
                    if v.fract() == 0.0 {
                        format!("{}", v as i64)
                    } else {
                        v.to_string()
                    }
                })
                .or_else(|| n.as_str().map(str::to_owned));
            if let Some(s) = s {
                return SlackListCellValue::Number(s);
            }
        }
    }
    if let Some(arr) = field.get("rating").and_then(Value::as_array) {
        if let Some(n) = arr.first().and_then(Value::as_u64) {
            return SlackListCellValue::Text("\u{2605}".repeat((n as usize).min(10)));
        }
    }
    if let Some(t) = field.get("text").and_then(Value::as_str) {
        let trimmed = t.trim();
        if !trimmed.is_empty() {
            return SlackListCellValue::Text(trimmed.to_owned());
        }
    }
    if let Some(rt) = field.get("rich_text") {
        let plain = list_rich_text_plain(rt);
        if !plain.is_empty() {
            return SlackListCellValue::Text(plain);
        }
    }
    for arr_key in ["email", "phone", "channel"] {
        if let Some(arr) = field.get(arr_key).and_then(Value::as_array) {
            let joined: Vec<&str> = arr.iter().filter_map(Value::as_str).collect();
            if !joined.is_empty() {
                return SlackListCellValue::Text(joined.join(", "));
            }
        }
    }
    if let Some(v) = field.get("value").and_then(Value::as_str) {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            return SlackListCellValue::Text(trimmed.to_owned());
        }
    }
    SlackListCellValue::Empty
}

// ---------------------------------------------------------------------------
// Files Browser Extras (`files.favorites.*`, `files.collections.*`, `files.recentlyDeleted`)
// ---------------------------------------------------------------------------

/// One custom file collection/section (`files.collections.list`, `F-CORE-133`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileCollection {
    pub id: String,
    pub name: String,
    pub emoji: String,
    pub file_ids: Vec<String>,
}

/// One recently deleted canvas or list (`files.recentlyDeleted`, `F-CORE-133`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeletedFile {
    pub id: String,
    pub title: String,
    pub date_delete: i64,
    pub file_type: String,
}

#[derive(Deserialize, Default)]
pub(super) struct FileFavoritesWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(super) file_ids: Vec<String>,
}

#[derive(Deserialize, Default)]
pub(super) struct FileCollectionFileWire {
    #[serde(default)]
    pub(super) id: String,
}

#[derive(Deserialize, Default)]
pub(super) struct FileCollectionWire {
    #[serde(default)]
    pub(super) id: String,
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) emoji: String,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) files: Vec<FileCollectionFileWire>,
}

#[derive(Deserialize, Default)]
pub(super) struct FileCollectionsListWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) collections: Vec<FileCollectionWire>,
}

impl From<FileCollectionWire> for FileCollection {
    fn from(w: FileCollectionWire) -> Self {
        Self {
            id: w.id,
            name: w.name,
            emoji: w.emoji,
            file_ids: w
                .files
                .into_iter()
                .map(|f| f.id)
                .filter(|id| !id.is_empty())
                .collect(),
        }
    }
}

#[derive(Deserialize, Default)]
pub(super) struct DeletedFileWire {
    #[serde(default)]
    pub(super) id: String,
    #[serde(default)]
    pub(super) title: String,
    #[serde(default)]
    pub(super) date_delete: i64,
    #[serde(default)]
    pub(super) file_type: String,
}

#[derive(Deserialize, Default)]
pub(super) struct RecentlyDeletedWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(super) files: Vec<DeletedFileWire>,
}
