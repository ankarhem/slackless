//! Typed endpoint methods. Method-name strings come from [`crate::types::endpoints::method`];
//! request params are built inline per method.
//!
//! Response structs model only what slackless uses; every field is defaulted, unknown keys are
//! kept in `extra` (or ignored), and lists use lenient element parsing.

use std::collections::HashMap;

use futures::TryStreamExt;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::client::Params;
use crate::de::{lenient_bool, lenient_u32, lenient_vec, null_default, or_default};
use crate::pagination::{Page, paginate};
use crate::types::endpoints::method;
use crate::types::{
    ApiResponse, Attachment, Block, BotId, ChannelId, ChannelSection, Conversation,
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

    /// One page of `files.list`: the files shared with `channel`, newest first. `files.list`
    /// paginates by `page`, so a single large first page covers the Files & links view.
    pub async fn files_list(
        &self,
        channel: &ChannelId,
        count: u32,
        page_no: u32,
    ) -> Result<Page<File>> {
        let params = vec![
            ("channel", channel.to_string()),
            ("count", count.to_string()),
            ("page", page_no.to_string()),
        ];
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
}
