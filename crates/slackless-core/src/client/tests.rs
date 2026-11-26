use std::ops::RangeInclusive;
use std::time::Duration;

use super::*;
use crate::api::{
    self, ChannelBookmark, ClientCounts, FilesFilter, MessageContent, Page, PinnedItem,
    PostedMessage, SearchMessages, UserBoot,
};
use crate::rtm::ConnectionState;
use crate::types::{
    ChannelCanvas, ChannelSection, Conversation, ConversationProperties, DndStatus, File, FileId,
    Message, Presence, SectionKind, Team, Ts, User, UserId, UserPresence,
};

/// History page size of the fake.
const PAGE: usize = 2;

fn test_media_tx() -> HuddleMediaSender {
    mpsc::channel(HUDDLE_MEDIA_CHANNEL_CAPACITY).0
}

/// In-process stand-in for Slack: a fixed channel history paged by index cursors.
#[derive(Clone, Default)]
pub(crate) struct FakeBackend {
    state: Arc<Mutex<FakeState>>,
}

#[derive(Default)]
struct FakeState {
    history: Vec<Message>,
    calls: Vec<(Option<Ts>, Option<String>)>,
    /// One thread (`conversations.replies`): the root followed by its replies.
    thread: Vec<Message>,
    /// The `subscriptions.thread.getView` answer.
    threads_view: api::ThreadsView,
    /// Sidebar sections served by `channel_sections`; `create_section` appends here.
    sections: Vec<ChannelSection>,
    reply_calls: Vec<(Ts, Option<String>)>,
    /// `subscriptions.thread.add` / `.remove` asks: thread, read cursor, follow.
    subscription_calls: Vec<(Ts, Ts, bool)>,
    /// `admin.roles.entity.listAssignments` answer: the channel's managers.
    managers: Vec<UserId>,
    rate_limited: u32,
    fail_replies: bool,
    /// `conversations.info` answer; the default echoes the id with `is_archived` set.
    info: Option<Conversation>,
    /// `conversations.info` asks, in order.
    info_calls: Vec<ChannelId>,
    /// Every `conversations.info` call fails with `channel_not_found`.
    fail_info: bool,
    /// `conversations.members` answer for every channel.
    members: Vec<UserId>,
    /// Every `conversations.members` call fails with `channel_not_found`.
    fail_members: bool,
    /// `conversations.members` asks, in order.
    member_calls: Vec<ChannelId>,
    /// `conversations.history` limit-1 asks (DM previews), in order.
    latest_calls: Vec<ChannelId>,
    /// Single-page `conversations.history` asks (`FetchLatestHistory`): (channel, limit).
    recent_calls: Vec<(ChannelId, u32)>,
    /// `conversations.mark` asks, in order.
    mark_calls: Vec<(ChannelId, Ts)>,
    /// `subscriptions.thread.mark` asks, in order.
    thread_mark_calls: Vec<(ChannelId, Ts, Ts)>,
    /// `chat.postMessage` asks, in order.
    posted_messages: Vec<(ChannelId, String)>,
    /// When true, `post_message` fails with a Slack error (for offline retry tests).
    fail_post_message: bool,
    /// `reactions.add` asks, in order.
    reaction_adds: Vec<(ChannelId, Ts, String)>,
    /// `client.dms` answer.
    dms: Vec<Conversation>,
    /// How often `client.dms` was asked.
    dm_calls: usize,
    /// `users.conversations` answer (ids).
    memberships: Vec<ChannelId>,
    /// How often `users.conversations` was asked.
    membership_calls: usize,
    /// `dnd.info` answers keyed by user; an unconfigured user is never paused.
    dnd: HashMap<UserId, DndStatus>,
    presence: HashMap<UserId, UserPresence>,
    dnd_calls: Vec<UserId>,
    presence_calls: Vec<UserId>,
    user_info_calls: Vec<UserId>,
    /// `users.info` answers' organizations by user; others answer without one.
    user_teams: HashMap<UserId, TeamId>,
    leave_calls: Vec<ChannelId>,
    /// `conversations.setTopic` asks: (channel, topic).
    topic_calls: Vec<(ChannelId, String)>,
    /// `conversations.setPurpose` asks: (channel, purpose).
    purpose_calls: Vec<(ChannelId, String)>,
    /// `team.info` answers keyed by team id; an unconfigured team fails.
    teams: HashMap<TeamId, Team>,
    /// `team.info` asks, in order.
    team_info_calls: Vec<TeamId>,
    notification_calls: Vec<(ChannelId, &'static str, &'static str)>,
    /// `pins.list` answer.
    pins: Vec<PinnedItem>,
    /// `bookmarks.list` answer; `bookmarks.add` appends to it.
    bookmarks: Vec<ChannelBookmark>,
    /// `files.list` answer.
    files: Vec<File>,
    bookmark_adds: Vec<(ChannelId, String, String)>,
    /// `pins.add` / `pins.remove` asks: (channel, ts, pinned?).
    pin_calls: Vec<(ChannelId, Ts, bool)>,
    canvas_creations: Vec<ChannelId>,
    snooze_calls: Vec<Option<u32>>,
    scheduled: Vec<api::ScheduledMessage>,
    /// Every tab load (`pins.list`, `bookmarks.list`, `files.list`) fails.
    fail_tab_loads: bool,
    /// The `activity.feed` answer.
    activity: api::ActivityFeed,
    /// `messages.list` bodies by channel.
    activity_bodies: HashMap<ChannelId, Vec<Message>>,
    /// `activity.markRead`/`markUnread` asks: (read?, entries).
    activity_marks: Vec<(bool, Vec<api::ActivityEntry>)>,
    /// `users.channelSections.set` asks: (section id, name, emoji).
    section_renames: Vec<(String, String, String)>,
    /// `users.channelSections.delete` asks, in order.
    section_deletes: Vec<String>,
    /// `users.channelSections.channels.bulkUpdate` asks: (insert, remove).
    section_moves: Vec<SectionMove>,
    /// `client.counts` answer and call count.
    counts: ClientCounts,
    counts_calls: usize,
    /// Per-channel `recent_history` override; falls back to `history`.
    recent_by_channel: HashMap<ChannelId, Vec<Message>>,
    /// When true, `history_page` and `replies_page` always return the same cursor `"stuck"`.
    stuck_cursor: bool,
    /// Transient `conversations.info` failures (`ApiError::AuthExpired`) per channel.
    fail_info_transient: HashSet<ChannelId>,
    /// `bots.info` responses and call log.
    bots: HashMap<BotId, BotProfile>,
    bot_info_calls: Vec<BotId>,
    fail_bots: HashSet<BotId>,
    bot_info_gate: Option<Arc<tokio::sync::Notify>>,
    /// When true, remote search / app home / list / files extras return `ApiError::AuthExpired`.
    auth_expired_collab: bool,
    /// When true, `mark_read` succeeds instead of returning `AuthExpired`.
    mark_read_ok: bool,
    /// When true, `mark_all_read` succeeds instead of returning `AuthExpired`.
    mark_all_read_ok: bool,
    mark_all_read_calls: usize,
    /// `reactions.remove` asks, in order.
    reaction_removes: Vec<(ChannelId, Ts, String)>,
    /// Huddle backend hooks & call logs.
    rooms_join_calls: Vec<ChannelId>,
    rooms_join_gate: Option<Arc<tokio::sync::Notify>>,
    rooms_join_response: Option<api::RoomJoinResponse>,
    rooms_get_link_override: Option<String>,
    rooms_notify_calls: Vec<(ChannelId, UserId)>,
    rooms_invite_responses: Vec<(ChannelId, String, String)>,
    rooms_rename_calls: Vec<(String, String)>,
    huddles_bg_calls: Vec<(String, String)>,
    transcription_calls: Vec<(String, bool)>,
    /// Cloud draft hooks & call logs.
    cloud_drafts: Vec<api::CloudDraft>,
    draft_creates: Vec<(ChannelId, String)>,
    draft_updates: Vec<(String, ChannelId, String)>,
    draft_deletes: Vec<String>,
    fail_draft_delete: bool,
    draft_create_gate: Option<Arc<tokio::sync::Notify>>,
    /// Saved / Later hooks & call logs.
    saved_by_filter: HashMap<&'static str, Vec<api::SavedItem>>,
    fail_saved_filters: HashSet<&'static str>,
    saved_add_error: Option<String>,
    saved_add_calls: Vec<(ChannelId, Ts, Option<u64>)>,
    saved_update_calls: Vec<SavedUpdateCall>,
    saved_delete_calls: Vec<(ChannelId, Ts)>,
    saved_clear_completed_calls: usize,
    save_file_calls: Vec<(String, Option<u64>)>,
    save_file_error: Option<String>,
    /// Hydrate overrides & partial failure flags.
    user_boot_override: Option<UserBoot>,
    fail_dms: bool,
    fail_emoji: bool,
    custom_emoji_override: Option<HashMap<String, String>>,
    usergroups: Vec<crate::types::Usergroup>,
    usergroup_disable_calls: Vec<crate::types::UsergroupId>,
    usergroup_enable_calls: Vec<crate::types::UsergroupId>,
    usergroup_users: HashMap<crate::types::UsergroupId, Vec<UserId>>,
    usergroup_users_update_calls: Vec<(crate::types::UsergroupId, Vec<UserId>)>,
    app_commands: Vec<api::AppCommand>,
    fail_activity_feed: bool,
    fail_messages_list: bool,
    /// Additional command group hooks & call logs.
    run_command_error: Option<String>,
    block_action_calls: Vec<(BotId, String, TeamId)>,
    rename_channel_calls: Vec<(ChannelId, String)>,
    archive_calls: Vec<ChannelId>,
    unarchive_calls: Vec<ChannelId>,
    delete_channel_calls: Vec<ChannelId>,
    convert_private_calls: Vec<ChannelId>,
    invite_calls: Vec<(ChannelId, Vec<UserId>)>,
    profile_update_response: Option<crate::types::Profile>,
    user_profile_response: Option<crate::types::Profile>,
    team_profile_fields_response: HashMap<String, String>,
    gif_featured_calls: usize,
    gif_search_calls: Vec<String>,
    edge_search_calls: Vec<(TeamId, String)>,
    canvases_create_calls: Vec<(Option<String>, Option<String>, Option<ChannelId>)>,
    canvases_edit_calls: Vec<String>,
    canvases_delete_calls: Vec<String>,
    canvases_delete_error: Option<String>,
    canvases_access_set_calls: Vec<(String, api::CanvasAccessLevel, Vec<ChannelId>, Vec<UserId>)>,
    canvases_access_delete_calls: Vec<(String, Vec<ChannelId>, Vec<UserId>)>,
    canvases_access_error: Option<String>,
    canvas_comments: Vec<api::CanvasCommentThread>,
    canvas_comments_error: Option<String>,
    canvas_comment_posts: Vec<(String, String, Option<Ts>)>,
    quip_archive_calls: Vec<(ChannelId, Ts, bool)>,
    quip_archive_all_calls: Vec<ChannelId>,
    quip_archive_error: Option<String>,
    fail_canvas_html: bool,
    list_data: api::SlackListData,
    list_meta: Option<api::ListMeta>,
    list_create_item_calls: Vec<String>,
    list_access_set_calls: Vec<(String, api::ListAccessLevel, Vec<ChannelId>, Vec<UserId>)>,
    list_access_delete_calls: Vec<(String, Vec<ChannelId>, Vec<UserId>)>,
    list_access_delete_result: api::ListAccessDeleteResult,
    list_access_error: Option<String>,
    list_record_threads: Vec<Message>,
    files_favorites: Vec<String>,
    files_collections: Vec<api::FileCollection>,
    files_deleted: Vec<api::DeletedFile>,
}

/// One `users.channelSections.channels.bulkUpdate` ask: its insert and remove sides.
type SectionMove = (Option<(String, ChannelId)>, Option<(String, ChannelId)>);
/// One `saved.update` ask: channel, ts, mark, and date_due.
type SavedUpdateCall = (ChannelId, Ts, Option<api::SavedMark>, Option<Option<u64>>);

/// The fake's canned sections: stored order is deliberately not display order (L2
/// points at L1), and "Priority" already holds C1.
fn base_sections() -> Vec<ChannelSection> {
    vec![
        ChannelSection {
            id: "L1".to_owned(),
            name: "Channels".to_owned(),
            kind: SectionKind::Channels,
            emoji: String::new(),
            next_id: None,
            channel_ids: Vec::new(),
        },
        ChannelSection {
            id: "L2".to_owned(),
            name: "Priority".to_owned(),
            kind: SectionKind::Standard,
            emoji: "rocket".to_owned(),
            next_id: Some("L1".to_owned()),
            channel_ids: vec![ChannelId::from("C1")],
        },
    ]
}

/// The mutable section `id` in `state`, seeding the canned sections first so writes to
/// them stick (an empty override means "not yet written", so it falls back to base).
fn effective_section<'a>(state: &'a mut FakeState, id: &str) -> &'a mut ChannelSection {
    if state.sections.is_empty() {
        state.sections = base_sections();
    }
    state
        .sections
        .iter_mut()
        .find(|section| section.id == id)
        .expect("the canned sections hold the asked id")
}

impl FakeBackend {
    pub fn with_history(range: RangeInclusive<u32>) -> Self {
        let fake = Self::default();
        fake.push_history(range);
        fake
    }

    /// The `subscriptions.thread.getView` answer.
    pub fn set_threads_view(&self, view: api::ThreadsView) {
        self.state.lock().unwrap().threads_view = view;
    }

    /// The `activity.feed` answer, and the `messages.list` bodies by channel.
    pub fn set_activity(&self, feed: api::ActivityFeed, bodies: HashMap<ChannelId, Vec<Message>>) {
        let mut state = self.state.lock().unwrap();
        state.activity = feed;
        state.activity_bodies = bodies;
    }

    /// `activity.markRead`/`markUnread` asks so far: (read?, entries).
    pub fn activity_marks(&self) -> Vec<(bool, Vec<api::ActivityEntry>)> {
        self.state.lock().unwrap().activity_marks.clone()
    }

    pub fn push_history(&self, range: RangeInclusive<u32>) {
        self.state
            .lock()
            .unwrap()
            .history
            .extend(range.map(|i| Message {
                ts: Ts::from(format!("{i}.0")),
                text: format!("m{i}"),
                ..Default::default()
            }));
    }

    /// A thread rooted at `thread_ts` with one reply per item of `range` (reply `i` gets
    /// ts `{thread_ts}.{i}`), served whole or in pages by `replies_page`.
    pub fn push_thread(&self, thread_ts: &str, range: RangeInclusive<u32>) {
        let mut state = self.state.lock().unwrap();
        state.thread.push(Message {
            ts: Ts::from(thread_ts),
            text: format!("root {thread_ts}"),
            ..Default::default()
        });
        state.thread.extend(range.map(|i| Message {
            ts: Ts::from(format!("{thread_ts}.{i}")),
            thread_ts: Some(Ts::from(thread_ts)),
            text: format!("reply {i}"),
            ..Default::default()
        }));
    }

    /// Make every `replies_page` call fail with `invalid_auth`.
    pub fn fail_replies(&self) {
        self.state.lock().unwrap().fail_replies = true;
    }

    /// The `conversations.info` answer for every channel.
    pub fn set_info(&self, conversation: Conversation) {
        self.state.lock().unwrap().info = Some(conversation);
    }

    /// Make every `conversations.info` call fail with `channel_not_found`.
    pub fn fail_info(&self) {
        self.state.lock().unwrap().fail_info = true;
    }

    /// `conversations.info` asks, in order.
    pub fn info_calls(&self) -> Vec<ChannelId> {
        self.state.lock().unwrap().info_calls.clone()
    }

    /// The `conversations.members` answer for every channel.
    pub fn set_members(&self, members: Vec<UserId>) {
        self.state.lock().unwrap().members = members;
    }

    /// Make every `conversations.members` call fail with `channel_not_found`.
    pub fn fail_members(&self) {
        self.state.lock().unwrap().fail_members = true;
    }

    /// `conversations.members` asks, in order.
    pub fn member_calls(&self) -> Vec<ChannelId> {
        self.state.lock().unwrap().member_calls.clone()
    }

    /// `conversations.history` limit-1 asks (DM previews), in order.
    pub fn latest_calls(&self) -> Vec<ChannelId> {
        self.state.lock().unwrap().latest_calls.clone()
    }

    /// The `client.dms` answer.
    pub fn set_dms(&self, dms: Vec<Conversation>) {
        self.state.lock().unwrap().dms = dms;
    }

    /// How often `client.dms` was asked.
    pub fn dm_calls(&self) -> usize {
        self.state.lock().unwrap().dm_calls
    }

    /// The `users.conversations` answer: the ids of the user's channel memberships.
    pub fn set_memberships(&self, members: Vec<ChannelId>) {
        self.state.lock().unwrap().memberships = members;
    }

    /// How often `users.conversations` was asked.
    pub fn membership_calls(&self) -> usize {
        self.state.lock().unwrap().membership_calls
    }

    /// Pushes one message into the shared history (unfurl attachments included).
    pub fn push_message(&self, message: Message) {
        self.state.lock().unwrap().history.push(message);
    }

    /// The `pins.list` answer.
    pub fn set_pins(&self, pins: Vec<PinnedItem>) {
        self.state.lock().unwrap().pins = pins;
    }

    /// The `bookmarks.list` answer.
    pub fn set_bookmarks(&self, bookmarks: Vec<ChannelBookmark>) {
        self.state.lock().unwrap().bookmarks = bookmarks;
    }

    /// The `files.list` answer.
    pub fn set_files(&self, files: Vec<File>) {
        self.state.lock().unwrap().files = files;
    }

    /// Make every tab load (`pins.list`, `bookmarks.list`, `files.list`) fail.
    pub fn fail_tab_loads(&self) {
        self.state.lock().unwrap().fail_tab_loads = true;
    }

    /// One tab load's answer: `pick`'s list, or a Slack error once `fail_tab_loads`.
    fn tab_load<T: Clone>(&self, pick: impl FnOnce(&FakeState) -> &Vec<T>) -> api::Result<Vec<T>> {
        let state = self.state.lock().unwrap();
        if state.fail_tab_loads {
            return Err(ApiError::Slack("channel_not_found".into()));
        }
        Ok(pick(&state).clone())
    }

    /// The recorded `bookmarks.add` calls.
    pub fn bookmark_adds(&self) -> Vec<(ChannelId, String, String)> {
        self.state.lock().unwrap().bookmark_adds.clone()
    }

    /// The recorded `pins.add` / `pins.remove` calls.
    pub fn pin_calls(&self) -> Vec<(ChannelId, Ts, bool)> {
        self.state.lock().unwrap().pin_calls.clone()
    }

    /// The recorded `conversations.canvases.create` calls.
    pub fn canvas_creations(&self) -> Vec<ChannelId> {
        self.state.lock().unwrap().canvas_creations.clone()
    }

    /// The `dnd.info` answer for `user`.
    pub fn set_dnd(&self, user: UserId, status: DndStatus) {
        self.state.lock().unwrap().dnd.insert(user, status);
    }

    /// The `users.getPresence` answer for `user`.
    pub fn set_presence(&self, user: UserId, presence: UserPresence) {
        self.state.lock().unwrap().presence.insert(user, presence);
    }

    /// Users asked for `dnd.info`, in order.
    pub fn dnd_calls(&self) -> Vec<UserId> {
        self.state.lock().unwrap().dnd_calls.clone()
    }

    /// Users asked for `users.getPresence`, in order.
    pub fn presence_calls(&self) -> Vec<UserId> {
        self.state.lock().unwrap().presence_calls.clone()
    }

    /// Users asked for `users.info`, in order.
    pub fn user_info_calls(&self) -> Vec<UserId> {
        self.state.lock().unwrap().user_info_calls.clone()
    }

    /// Channels left through `conversations.leave`, in order.
    pub fn leave_calls(&self) -> Vec<ChannelId> {
        self.state.lock().unwrap().leave_calls.clone()
    }

    /// `users.channelSections.set` asks so far: (section id, name, emoji).
    pub fn section_renames(&self) -> Vec<(String, String, String)> {
        self.state.lock().unwrap().section_renames.clone()
    }

    /// `users.channelSections.delete` asks so far.
    pub fn section_deletes(&self) -> Vec<String> {
        self.state.lock().unwrap().section_deletes.clone()
    }

    /// `users.channelSections.channels.bulkUpdate` asks so far: (insert, remove).
    pub fn section_moves(&self) -> Vec<SectionMove> {
        self.state.lock().unwrap().section_moves.clone()
    }

    /// The recorded `conversations.setTopic` calls.
    pub fn topic_calls(&self) -> Vec<(ChannelId, String)> {
        self.state.lock().unwrap().topic_calls.clone()
    }

    /// The recorded `conversations.setPurpose` calls.
    pub fn purpose_calls(&self) -> Vec<(ChannelId, String)> {
        self.state.lock().unwrap().purpose_calls.clone()
    }

    /// The `team.info` answer for `team` (an unconfigured team still fails).
    pub fn set_team(&self, team: Team) {
        self.state
            .lock()
            .unwrap()
            .teams
            .insert(team.id.clone(), team);
    }

    /// Teams asked for `team.info`, in order.
    pub fn team_info_calls(&self) -> Vec<TeamId> {
        self.state.lock().unwrap().team_info_calls.clone()
    }

    /// The organization `users.info` answers for `user`.
    pub fn set_user_team(&self, user: UserId, team: TeamId) {
        self.state.lock().unwrap().user_teams.insert(user, team);
    }

    /// `users.prefs.setNotifications` writes, in order.
    pub fn notification_calls(&self) -> Vec<(ChannelId, &'static str, &'static str)> {
        self.state.lock().unwrap().notification_calls.clone()
    }

    pub fn messages(&self) -> Vec<Message> {
        self.state.lock().unwrap().history.clone()
    }

    /// `(latest, cursor)` of every history request.
    pub fn calls(&self) -> Vec<(Option<Ts>, Option<String>)> {
        self.state.lock().unwrap().calls.clone()
    }

    /// `(thread_ts, cursor)` of every `conversations.replies` request.
    pub fn reply_calls(&self) -> Vec<(Ts, Option<String>)> {
        self.state.lock().unwrap().reply_calls.clone()
    }

    pub fn clear_calls(&self) {
        self.state.lock().unwrap().calls.clear();
    }

    pub fn rate_limit_next(&self, n: u32) {
        self.state.lock().unwrap().rate_limited = n;
    }

    pub fn set_stuck_cursor(&self, stuck: bool) {
        self.state.lock().unwrap().stuck_cursor = stuck;
    }

    pub fn fail_info_transient_for(&self, channel: ChannelId) {
        self.state
            .lock()
            .unwrap()
            .fail_info_transient
            .insert(channel);
    }

    pub fn clear_fail_info_transient(&self) {
        self.state.lock().unwrap().fail_info_transient.clear();
    }

    pub fn set_counts(&self, counts: ClientCounts) {
        self.state.lock().unwrap().counts = counts;
    }

    pub fn counts_calls(&self) -> usize {
        self.state.lock().unwrap().counts_calls
    }
}

impl crate::backend::WorkspaceBackend for FakeBackend {
    async fn user_boot(&self) -> api::Result<UserBoot> {
        if let Some(boot) = self.state.lock().unwrap().user_boot_override.clone() {
            return Ok(boot);
        }
        Ok(UserBoot {
            channels: vec![Conversation {
                id: ChannelId::from("C1"),
                ..Default::default()
            }],
            team: Some(Team {
                id: TeamId::from("T1"),
                name: "Acme".into(),
                domain: "acme".into(),
                ..Default::default()
            }),
            prefs: Some(serde_json::json!({"display_real_names": true})),
            ..Default::default()
        })
    }

    async fn counts(&self) -> api::Result<ClientCounts> {
        let mut state = self.state.lock().unwrap();
        state.counts_calls += 1;
        Ok(state.counts.clone())
    }

    async fn users(&self) -> api::Result<Vec<User>> {
        Ok(vec![User {
            id: UserId::from("U1"),
            ..Default::default()
        }])
    }

    async fn channel_sections(&self) -> api::Result<Vec<ChannelSection>> {
        let state = self.state.lock().unwrap();
        Ok(if state.sections.is_empty() {
            base_sections()
        } else {
            state.sections.clone()
        })
    }

    async fn custom_emoji(&self) -> api::Result<HashMap<String, String>> {
        let state = self.state.lock().unwrap();
        if state.fail_emoji {
            return Err(ApiError::Slack("emoji_unavailable".into()));
        }
        if let Some(ref map) = state.custom_emoji_override {
            return Ok(map.clone());
        }
        Ok(HashMap::from([(
            "party".to_owned(),
            "https://emoji.example/party.png".to_owned(),
        )]))
    }

    async fn dm_list(&self) -> api::Result<Vec<Conversation>> {
        let mut state = self.state.lock().unwrap();
        state.dm_calls += 1;
        if state.fail_dms {
            return Err(ApiError::Slack("dms_unavailable".into()));
        }
        Ok(state.dms.clone())
    }

    async fn channel_memberships(&self) -> api::Result<Vec<Conversation>> {
        let mut state = self.state.lock().unwrap();
        state.membership_calls += 1;
        Ok(state
            .memberships
            .iter()
            .map(|id| Conversation {
                id: id.clone(),
                ..Default::default()
            })
            .collect())
    }

    async fn team_info(&self, team: TeamId) -> api::Result<Team> {
        let mut state = self.state.lock().unwrap();
        state.team_info_calls.push(team.clone());
        state
            .teams
            .get(&team)
            .cloned()
            .ok_or_else(|| ApiError::Slack("team_not_found".into()))
    }

    async fn app_commands(&self) -> api::Result<Vec<api::AppCommand>> {
        Ok(self.state.lock().unwrap().app_commands.clone())
    }
}

impl crate::backend::MessagingBackend for FakeBackend {
    async fn history_page(
        &self,
        _channel: ChannelId,
        latest: Option<Ts>,
        cursor: Option<String>,
    ) -> api::Result<Page<Message>> {
        let mut state = self.state.lock().unwrap();
        state.calls.push((latest.clone(), cursor.clone()));
        if state.rate_limited > 0 {
            state.rate_limited -= 1;
            return Err(ApiError::RateLimited {
                retry_after: Duration::from_secs(5),
            });
        }
        let mut messages: Vec<Message> = state
            .history
            .iter()
            .filter(|m| latest.as_ref().is_none_or(|l| m.ts < *l))
            .cloned()
            .collect();
        messages.sort_by(|a, b| b.ts.cmp(&a.ts));
        if state.stuck_cursor {
            let end = PAGE.min(messages.len());
            return Ok(Page {
                items: messages[..end].to_vec(),
                next_cursor: Some("stuck".to_owned()),
            });
        }
        let start: usize = cursor.map_or(0, |c| c.parse().unwrap());
        let end = (start + PAGE).min(messages.len());
        Ok(Page {
            items: messages[start..end].to_vec(),
            next_cursor: (end < messages.len()).then(|| end.to_string()),
        })
    }

    async fn replies_page(
        &self,
        _channel: ChannelId,
        thread_ts: Ts,
        cursor: Option<String>,
    ) -> api::Result<Page<Message>> {
        let mut state = self.state.lock().unwrap();
        state.reply_calls.push((thread_ts, cursor.clone()));
        if state.fail_replies {
            return Err(ApiError::AuthExpired("invalid_auth".into()));
        }
        if state.stuck_cursor {
            let end = PAGE.min(state.thread.len());
            return Ok(Page {
                items: state.thread[..end].to_vec(),
                next_cursor: Some("stuck".to_owned()),
            });
        }
        let start: usize = cursor.map_or(0, |c| c.parse().unwrap());
        let end = (start + PAGE).min(state.thread.len());
        Ok(Page {
            items: state.thread[start..end].to_vec(),
            next_cursor: (end < state.thread.len()).then(|| end.to_string()),
        })
    }

    async fn latest_message(&self, channel: ChannelId) -> api::Result<Option<Message>> {
        let mut state = self.state.lock().unwrap();
        state.latest_calls.push(channel);
        Ok(state.history.iter().max_by(|a, b| a.ts.cmp(&b.ts)).cloned())
    }

    async fn recent_history(&self, channel: ChannelId, limit: u32) -> api::Result<Vec<Message>> {
        let mut state = self.state.lock().unwrap();
        state.recent_calls.push((channel.clone(), limit));
        let mut items = state
            .recent_by_channel
            .get(&channel)
            .cloned()
            .unwrap_or_else(|| state.history.clone());
        items.sort_by(|a, b| b.ts.cmp(&a.ts));
        items.truncate(limit as usize);
        Ok(items)
    }

    async fn post_message(
        &self,
        channel: ChannelId,
        content: MessageContent,
        _thread_ts: Option<Ts>,
        _broadcast: bool,
    ) -> api::Result<PostedMessage> {
        {
            let mut state = self.state.lock().unwrap();
            if state.fail_post_message {
                return Err(ApiError::Slack("ratelimited".into()));
            }
            state
                .posted_messages
                .push((channel.clone(), content.text.clone()));
        }
        let ts = Ts::from("100.0");
        let message = Message {
            ts: ts.clone(),
            text: content.text,
            blocks: content.blocks,
            attachments: content.attachments,
            ..Default::default()
        };
        Ok(PostedMessage {
            channel,
            ts,
            message: Some(message),
            client_msg_id: None,
        })
    }

    async fn add_reaction(&self, ch: ChannelId, ts: Ts, name: String) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .reaction_adds
            .push((ch, ts, name));
        Ok(())
    }

    async fn remove_reaction(&self, ch: ChannelId, ts: Ts, name: String) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .reaction_removes
            .push((ch, ts, name));
        Ok(())
    }

    async fn run_command(
        &self,
        _channel: ChannelId,
        _command: String,
        _args: String,
        _thread_ts: Option<Ts>,
        _team: TeamId,
    ) -> api::Result<()> {
        if let Some(err) = self.state.lock().unwrap().run_command_error.clone() {
            return Err(ApiError::Slack(err));
        }
        Ok(())
    }

    async fn block_action(
        &self,
        bot: BotId,
        app_id: String,
        team: TeamId,
        _action: serde_json::Value,
        _container: api::ActionContainer,
    ) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .block_action_calls
            .push((bot, app_id, team));
        Ok(())
    }

    async fn threads_view(&self) -> api::Result<api::ThreadsView> {
        Ok(self.state.lock().unwrap().threads_view.clone())
    }

    async fn activity_feed(&self) -> api::Result<api::ActivityFeed> {
        let state = self.state.lock().unwrap();
        if state.fail_activity_feed {
            return Err(ApiError::Slack("activity_unavailable".into()));
        }
        Ok(state.activity.clone())
    }

    async fn messages_list(
        &self,
        ids: Vec<(ChannelId, Vec<Ts>)>,
    ) -> api::Result<HashMap<ChannelId, Vec<Message>>> {
        let state = self.state.lock().unwrap();
        if state.fail_messages_list {
            return Err(ApiError::Slack("messages_list_failed".into()));
        }
        Ok(ids
            .into_iter()
            .map(|(channel, _)| {
                let messages = state
                    .activity_bodies
                    .get(&channel)
                    .cloned()
                    .unwrap_or_default();
                (channel, messages)
            })
            .collect())
    }

    async fn activity_mark_read(&self, entries: Vec<api::ActivityEntry>) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .activity_marks
            .push((true, entries));
        Ok(())
    }

    async fn activity_mark_unread(&self, entries: Vec<api::ActivityEntry>) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .activity_marks
            .push((false, entries));
        Ok(())
    }

    async fn mark_read(&self, channel: ChannelId, ts: Ts) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.mark_calls.push((channel, ts));
        if state.mark_read_ok {
            Ok(())
        } else {
            Err(ApiError::AuthExpired("invalid_auth".into()))
        }
    }

    async fn mark_thread_read(&self, channel: ChannelId, thread_ts: Ts, ts: Ts) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .thread_mark_calls
            .push((channel, thread_ts, ts));
        Ok(())
    }

    async fn mark_all_read(&self) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.mark_all_read_calls += 1;
        if state.mark_all_read_ok {
            Ok(())
        } else {
            Err(ApiError::AuthExpired("invalid_auth".into()))
        }
    }

    async fn set_thread_subscribed(
        &self,
        _: ChannelId,
        thread_ts: Ts,
        last_read: Ts,
        subscribed: bool,
    ) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state
            .subscription_calls
            .push((thread_ts, last_read, subscribed));
        Ok(())
    }

    async fn pins(&self, _: ChannelId) -> api::Result<Vec<PinnedItem>> {
        self.tab_load(|state| &state.pins)
    }

    async fn set_pinned(&self, channel: ChannelId, ts: Ts, pinned: bool) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .pin_calls
            .push((channel, ts, pinned));
        Ok(())
    }

    async fn bookmarks(&self, _: ChannelId) -> api::Result<Vec<ChannelBookmark>> {
        self.tab_load(|state| &state.bookmarks)
    }

    async fn add_bookmark(
        &self,
        channel: ChannelId,
        title: String,
        url: String,
    ) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state
            .bookmark_adds
            .push((channel, title.clone(), url.clone()));
        state.bookmarks.push(ChannelBookmark {
            id: "BkNew".to_owned(),
            title,
            link: Some(url),
            kind: Some("link".to_owned()),
            ..Default::default()
        });
        Ok(())
    }

    async fn schedule_message(
        &self,
        channel: ChannelId,
        post_at: u64,
        content: MessageContent,
        _thread_ts: Option<Ts>,
    ) -> api::Result<api::ScheduledMessageCreated> {
        let mut state = self.state.lock().unwrap();
        let id = format!("Q{}", state.scheduled.len() + 1);
        state.scheduled.push(api::ScheduledMessage {
            id: id.clone(),
            channel_id: channel.clone(),
            post_at,
            date_created: 1_760_000_000,
            text: content.text,
        });
        Ok(api::ScheduledMessageCreated {
            scheduled_message_id: id,
            channel,
            post_at,
        })
    }

    async fn scheduled_messages(&self) -> api::Result<Vec<api::ScheduledMessage>> {
        Ok(self.state.lock().unwrap().scheduled.clone())
    }

    async fn delete_scheduled_message(
        &self,
        _channel: ChannelId,
        scheduled_message_id: String,
    ) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .scheduled
            .retain(|m| m.id != scheduled_message_id);
        Ok(())
    }

    async fn drafts_list(&self) -> api::Result<Vec<api::CloudDraft>> {
        Ok(self.state.lock().unwrap().cloud_drafts.clone())
    }

    async fn drafts_create(&self, channel: ChannelId, text: String) -> api::Result<Option<String>> {
        let gate = {
            let mut state = self.state.lock().unwrap();
            state.draft_creates.push((channel, text));
            state.draft_create_gate.clone()
        };
        if let Some(gate) = gate {
            gate.notified().await;
        }
        let n = self.state.lock().unwrap().draft_creates.len();
        Ok(Some(format!("Dr{n:02}")))
    }

    async fn drafts_update(
        &self,
        draft_id: String,
        channel: ChannelId,
        text: String,
    ) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .draft_updates
            .push((draft_id, channel, text));
        Ok(())
    }

    async fn drafts_delete(&self, draft_id: String) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.draft_deletes.push(draft_id);
        if state.fail_draft_delete {
            return Err(ApiError::Slack("draft_delete_failed".into()));
        }
        Ok(())
    }

    async fn saved_list(&self, filter: &'static str) -> api::Result<Vec<api::SavedItem>> {
        let state = self.state.lock().unwrap();
        if state.fail_saved_filters.contains(filter) {
            return Err(ApiError::Slack(format!("saved_{filter}_failed")));
        }
        Ok(state
            .saved_by_filter
            .get(filter)
            .cloned()
            .unwrap_or_default())
    }

    async fn saved_add(
        &self,
        channel: ChannelId,
        ts: Ts,
        date_due: Option<u64>,
    ) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.saved_add_calls.push((channel, ts, date_due));
        if let Some(err) = state.saved_add_error.clone() {
            return Err(ApiError::Slack(err));
        }
        Ok(())
    }

    async fn saved_update(
        &self,
        channel: ChannelId,
        ts: Ts,
        mark: Option<api::SavedMark>,
        date_due: Option<Option<u64>>,
    ) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .saved_update_calls
            .push((channel, ts, mark, date_due));
        Ok(())
    }

    async fn saved_delete(&self, channel: ChannelId, ts: Ts) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .saved_delete_calls
            .push((channel, ts));
        Ok(())
    }

    async fn saved_clear_completed(&self) -> api::Result<()> {
        self.state.lock().unwrap().saved_clear_completed_calls += 1;
        Ok(())
    }

    async fn save_file(&self, file_id: String, date_due: Option<u64>) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.save_file_calls.push((file_id, date_due));
        if let Some(err) = state.save_file_error.clone() {
            return Err(ApiError::Slack(err));
        }
        Ok(())
    }
}

impl crate::backend::ChannelsBackend for FakeBackend {
    async fn conversation_info(&self, channel: ChannelId) -> api::Result<Conversation> {
        let mut state = self.state.lock().unwrap();
        state.info_calls.push(channel.clone());
        if state.fail_info_transient.contains(&channel) {
            return Err(ApiError::RateLimited {
                retry_after: Duration::from_secs(1),
            });
        }
        if state.fail_info {
            return Err(ApiError::Slack("channel_not_found".into()));
        }
        let info = state.info.clone();
        Ok(match info {
            Some(mut conversation) => {
                conversation.id = channel;
                conversation
            }
            None => Conversation {
                id: channel,
                is_archived: true,
                ..Default::default()
            },
        })
    }

    async fn conversation_members(&self, channel: ChannelId) -> api::Result<Vec<UserId>> {
        let mut state = self.state.lock().unwrap();
        state.member_calls.push(channel);
        if state.fail_members {
            return Err(ApiError::Slack("channel_not_found".into()));
        }
        Ok(state.members.clone())
    }

    async fn leave_conversation(&self, channel: ChannelId) -> api::Result<()> {
        self.state.lock().unwrap().leave_calls.push(channel);
        Ok(())
    }

    async fn join_conversation(&self, channel: ChannelId) -> api::Result<Conversation> {
        Ok(Conversation {
            id: channel,
            ..Default::default()
        })
    }

    async fn set_topic(&self, channel: ChannelId, topic: String) -> api::Result<Conversation> {
        self.state
            .lock()
            .unwrap()
            .topic_calls
            .push((channel.clone(), topic.clone()));
        Ok(Conversation {
            id: channel,
            topic: Some(crate::types::TopicPurpose {
                value: topic,
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    async fn set_purpose(&self, channel: ChannelId, purpose: String) -> api::Result<Conversation> {
        self.state
            .lock()
            .unwrap()
            .purpose_calls
            .push((channel.clone(), purpose.clone()));
        Ok(Conversation {
            id: channel,
            purpose: Some(crate::types::TopicPurpose {
                value: purpose,
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    async fn channel_managers(&self, _: ChannelId) -> api::Result<Vec<UserId>> {
        Ok(self.state.lock().unwrap().managers.clone())
    }

    async fn open_dm(&self, users: Vec<UserId>) -> api::Result<Conversation> {
        if users.len() > 1 {
            Ok(Conversation {
                id: ChannelId::from("G9"),
                is_mpim: true,
                members: users,
                ..Default::default()
            })
        } else {
            Ok(Conversation {
                id: ChannelId::from("D9"),
                is_im: true,
                user: users.into_iter().next(),
                ..Default::default()
            })
        }
    }

    async fn create_section(&self, name: String) -> api::Result<api::SectionCreated> {
        self.state.lock().unwrap().sections.push(ChannelSection {
            id: "LNEW".to_owned(),
            name,
            kind: SectionKind::Standard,
            emoji: String::new(),
            next_id: None,
            channel_ids: Vec::new(),
        });
        Ok(api::SectionCreated {
            channel_section_id: "LNEW".to_owned(),
        })
    }

    async fn rename_section(
        &self,
        section: String,
        name: String,
        emoji: String,
    ) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state
            .section_renames
            .push((section.clone(), name.clone(), emoji));
        let target = effective_section(&mut state, &section);
        target.name = name;
        Ok(())
    }

    async fn delete_section(&self, section: String) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.section_deletes.push(section.clone());
        if state.sections.is_empty() {
            state.sections = base_sections();
        }
        state.sections.retain(|known| known.id != section);
        Ok(())
    }

    async fn move_channel(
        &self,
        insert: Option<(String, ChannelId)>,
        remove: Option<(String, ChannelId)>,
    ) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.section_moves.push((insert.clone(), remove.clone()));
        if state.sections.is_empty() {
            state.sections = base_sections();
        }
        if let Some((from, channel)) = remove
            && let Some(source) = state.sections.iter_mut().find(|s| s.id == from)
        {
            source.channel_ids.retain(|id| *id != channel);
        }
        if let Some((to, channel)) = insert
            && let Some(target) = state.sections.iter_mut().find(|s| s.id == to)
            && !target.channel_ids.contains(&channel)
        {
            target.channel_ids.push(channel);
        }
        Ok(())
    }

    async fn create_channel(&self, name: String, is_private: bool) -> api::Result<Conversation> {
        Ok(Conversation {
            id: ChannelId::from("CNEW1"),
            name: Some(name),
            is_channel: !is_private,
            is_group: is_private,
            is_private,
            is_member: true,
            ..Default::default()
        })
    }

    async fn rename_channel(&self, channel: ChannelId, name: String) -> api::Result<Conversation> {
        self.state
            .lock()
            .unwrap()
            .rename_channel_calls
            .push((channel.clone(), name.clone()));
        Ok(Conversation {
            id: channel,
            name: Some(name),
            ..Default::default()
        })
    }

    async fn archive_channel(&self, channel: ChannelId) -> api::Result<()> {
        self.state.lock().unwrap().archive_calls.push(channel);
        Ok(())
    }

    async fn unarchive_channel(&self, channel: ChannelId) -> api::Result<()> {
        self.state.lock().unwrap().unarchive_calls.push(channel);
        Ok(())
    }

    async fn delete_channel(&self, channel: ChannelId) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .delete_channel_calls
            .push(channel);
        Ok(())
    }

    async fn convert_channel_to_private(&self, channel: ChannelId) -> api::Result<Conversation> {
        self.state
            .lock()
            .unwrap()
            .convert_private_calls
            .push(channel.clone());
        Ok(Conversation {
            id: channel,
            is_private: true,
            is_group: true,
            ..Default::default()
        })
    }

    async fn invite_to_channel(
        &self,
        channel: ChannelId,
        users: Vec<UserId>,
    ) -> api::Result<Conversation> {
        self.state
            .lock()
            .unwrap()
            .invite_calls
            .push((channel.clone(), users));
        Ok(Conversation {
            id: channel,
            ..Default::default()
        })
    }
}

impl crate::backend::UsersBackend for FakeBackend {
    /// Every id resolves, to a profile named `ext-<id>`.
    async fn user_info(&self, user: UserId) -> api::Result<User> {
        let mut state = self.state.lock().unwrap();
        state.user_info_calls.push(user.clone());
        Ok(User {
            name: format!("ext-{}", user.as_str()),
            team_id: state.user_teams.get(&user).cloned(),
            id: user,
            ..Default::default()
        })
    }

    async fn bot_info(&self, bot: BotId) -> api::Result<BotProfile> {
        let gate = {
            let mut state = self.state.lock().unwrap();
            state.bot_info_calls.push(bot.clone());
            state.bot_info_gate.clone()
        };
        if let Some(gate) = gate {
            gate.notified().await;
        }
        let state = self.state.lock().unwrap();
        if state.fail_bots.contains(&bot) {
            return Err(ApiError::Slack("bot_not_found".into()));
        }
        Ok(state.bots.get(&bot).cloned().unwrap_or(BotProfile {
            id: bot.clone(),
            name: format!("bot-{}", bot.as_str()),
            ..Default::default()
        }))
    }

    async fn user_presence(&self, user: UserId) -> api::Result<UserPresence> {
        let mut state = self.state.lock().unwrap();
        state.presence_calls.push(user.clone());
        Ok(state.presence.get(&user).cloned().unwrap_or(UserPresence {
            presence: Presence::Away,
            online: None,
            auto_away: None,
            manual_away: None,
            connection_count: None,
            last_activity: None,
        }))
    }

    async fn update_profile(
        &self,
        _update: api::ProfileUpdate,
    ) -> api::Result<slackless_types::Profile> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .profile_update_response
            .clone()
            .unwrap_or_default())
    }

    async fn user_profile(&self, _user: UserId) -> api::Result<slackless_types::Profile> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .user_profile_response
            .clone()
            .unwrap_or_default())
    }

    async fn team_profile_fields(&self) -> api::Result<HashMap<String, String>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .team_profile_fields_response
            .clone())
    }

    async fn set_notification_pref(
        &self,
        channel: ChannelId,
        name: &'static str,
        value: &'static str,
    ) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.notification_calls.push((channel, name, value));
        Ok(())
    }

    async fn dnd_info(&self, user: UserId) -> api::Result<DndStatus> {
        let mut state = self.state.lock().unwrap();
        state.dnd_calls.push(user.clone());
        Ok(state.dnd.get(&user).cloned().unwrap_or_default())
    }

    async fn set_dnd_snooze(&self, minutes: u32) -> api::Result<DndStatus> {
        self.state.lock().unwrap().snooze_calls.push(Some(minutes));
        Ok(DndStatus {
            snooze_enabled: true,
            snooze_endtime: Some(minutes as i64 * 60),
            ..Default::default()
        })
    }

    async fn end_dnd_snooze(&self) -> api::Result<DndStatus> {
        self.state.lock().unwrap().snooze_calls.push(None);
        Ok(DndStatus {
            snooze_enabled: false,
            ..Default::default()
        })
    }

    async fn usergroups(&self) -> api::Result<Vec<crate::types::Usergroup>> {
        Ok(self.state.lock().unwrap().usergroups.clone())
    }

    async fn disable_usergroup(
        &self,
        usergroup: crate::types::UsergroupId,
    ) -> api::Result<crate::types::Usergroup> {
        let mut state = self.state.lock().unwrap();
        state.usergroup_disable_calls.push(usergroup.clone());
        state.usergroups.retain(|g| g.id != usergroup);
        Ok(crate::types::Usergroup {
            id: usergroup,
            date_delete: 1_700_000_000,
            ..Default::default()
        })
    }

    async fn enable_usergroup(
        &self,
        usergroup: crate::types::UsergroupId,
    ) -> api::Result<crate::types::Usergroup> {
        let mut state = self.state.lock().unwrap();
        state.usergroup_enable_calls.push(usergroup.clone());
        let ug = crate::types::Usergroup {
            id: usergroup.clone(),
            name: "Engineering".to_owned(),
            handle: "eng".to_owned(),
            ..Default::default()
        };
        if !state.usergroups.iter().any(|g| g.id == usergroup) {
            state.usergroups.push(ug.clone());
        }
        Ok(ug)
    }

    async fn usergroup_users(
        &self,
        usergroup: crate::types::UsergroupId,
    ) -> api::Result<Vec<UserId>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .usergroup_users
            .get(&usergroup)
            .cloned()
            .unwrap_or_default())
    }

    async fn update_usergroup_users(
        &self,
        usergroup: crate::types::UsergroupId,
        users: Vec<UserId>,
    ) -> api::Result<crate::types::Usergroup> {
        let mut state = self.state.lock().unwrap();
        state
            .usergroup_users_update_calls
            .push((usergroup.clone(), users.clone()));
        let count = users.len() as u32;
        state.usergroup_users.insert(usergroup.clone(), users);
        if let Some(existing) = state.usergroups.iter_mut().find(|g| g.id == usergroup) {
            existing.user_count = Some(count);
            return Ok(existing.clone());
        }
        Ok(crate::types::Usergroup {
            id: usergroup,
            user_count: Some(count),
            ..Default::default()
        })
    }
}

impl crate::backend::FilesBackend for FakeBackend {
    async fn files(&self, _: FilesFilter) -> api::Result<Vec<File>> {
        self.tab_load(|state| &state.files)
    }

    async fn upload_files(
        &self,
        _channel: ChannelId,
        _thread_ts: Option<Ts>,
        _content: Option<MessageContent>,
        files: Vec<api::UploadFile>,
    ) -> api::Result<Vec<File>> {
        Ok(files
            .into_iter()
            .enumerate()
            .map(|(i, f)| File {
                id: crate::types::FileId::from(format!("FUP{i}")),
                name: Some(f.filename),
                size: Some(f.bytes.len() as i64),
                ..Default::default()
            })
            .collect())
    }

    async fn files_favorites_list(&self) -> api::Result<Vec<String>> {
        let state = self.state.lock().unwrap();
        if state.auth_expired_collab {
            return Err(ApiError::AuthExpired("token_revoked".into()));
        }
        Ok(state.files_favorites.clone())
    }

    async fn files_collections_list(&self) -> api::Result<Vec<api::FileCollection>> {
        Ok(self.state.lock().unwrap().files_collections.clone())
    }

    async fn files_recently_deleted(&self) -> api::Result<Vec<api::DeletedFile>> {
        Ok(self.state.lock().unwrap().files_deleted.clone())
    }
}

impl crate::backend::CollabBackend for FakeBackend {
    async fn search(&self, _: String) -> api::Result<SearchMessages> {
        Ok(SearchMessages {
            total: 7,
            ..Default::default()
        })
    }

    async fn edge_search(
        &self,
        team_id: TeamId,
        query: String,
    ) -> api::Result<api::EdgeSearchResults> {
        let mut state = self.state.lock().unwrap();
        state.edge_search_calls.push((team_id, query));
        if state.auth_expired_collab {
            return Err(ApiError::AuthExpired("token_revoked".into()));
        }
        Ok(api::EdgeSearchResults {
            users: vec![User {
                id: UserId::from("U9"),
                name: "bob".into(),
                ..Default::default()
            }],
            channels: Vec::new(),
        })
    }

    async fn search_remote_messages(
        &self,
        _query: String,
        _sort: api::SearchSort,
        page: usize,
    ) -> api::Result<api::RemoteMessagePage> {
        if self.state.lock().unwrap().auth_expired_collab {
            return Err(ApiError::AuthExpired("token_revoked".into()));
        }
        Ok(api::RemoteMessagePage {
            page,
            ..Default::default()
        })
    }

    async fn search_remote_files(
        &self,
        _query: String,
        _sort: api::SearchSort,
        page: usize,
    ) -> api::Result<api::RemoteFilePage> {
        if self.state.lock().unwrap().auth_expired_collab {
            return Err(ApiError::AuthExpired("token_revoked".into()));
        }
        Ok(api::RemoteFilePage {
            page,
            ..Default::default()
        })
    }

    async fn gif_featured(&self, _limit: u32) -> api::Result<Vec<api::GifItem>> {
        self.state.lock().unwrap().gif_featured_calls += 1;
        Ok(vec![api::GifItem {
            id: "g1".into(),
            title: "featured".into(),
            still_url: "https://gif.example/1s.gif".into(),
            preview_url: "https://gif.example/1p.gif".into(),
            url: "https://gif.example/1".into(),
        }])
    }

    async fn gif_search(&self, query: String, _limit: u32) -> api::Result<Vec<api::GifItem>> {
        self.state.lock().unwrap().gif_search_calls.push(query);
        Ok(vec![api::GifItem {
            id: "g2".into(),
            title: "cats".into(),
            still_url: "https://gif.example/2s.gif".into(),
            preview_url: "https://gif.example/2p.gif".into(),
            url: "https://gif.example/2".into(),
        }])
    }

    async fn create_canvas(&self, channel: ChannelId) -> api::Result<String> {
        self.state.lock().unwrap().canvas_creations.push(channel);
        Ok("F07NEW".to_owned())
    }

    async fn fetch_canvas_html(&self, _file_id: String) -> api::Result<(String, String)> {
        if self.state.lock().unwrap().fail_canvas_html {
            return Err(ApiError::Slack("canvas_not_found".into()));
        }
        Ok((
            "Sample canvas".to_owned(),
            "<h1>Sample canvas</h1><p>Hello</p>".to_owned(),
        ))
    }

    async fn canvases_create(
        &self,
        title: Option<String>,
        markdown: Option<String>,
        channel: Option<ChannelId>,
    ) -> api::Result<api::CanvasCreated> {
        self.state
            .lock()
            .unwrap()
            .canvases_create_calls
            .push((title, markdown, channel));
        Ok(api::CanvasCreated {
            canvas_id: "FCANVAS1".to_owned(),
        })
    }

    async fn canvases_edit(&self, file_id: String, _op: api::CanvasEditOp) -> api::Result<()> {
        self.state.lock().unwrap().canvases_edit_calls.push(file_id);
        Ok(())
    }

    async fn canvases_delete(&self, canvas_id: String) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.canvases_delete_calls.push(canvas_id);
        if let Some(err) = state.canvases_delete_error.clone() {
            return Err(ApiError::Slack(err));
        }
        Ok(())
    }

    async fn canvases_access_set(
        &self,
        canvas_id: String,
        access_level: api::CanvasAccessLevel,
        channel_ids: Vec<ChannelId>,
        user_ids: Vec<UserId>,
    ) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state
            .canvases_access_set_calls
            .push((canvas_id, access_level, channel_ids, user_ids));
        if let Some(err) = state.canvases_access_error.clone() {
            return Err(ApiError::Slack(err));
        }
        Ok(())
    }

    async fn canvases_access_delete(
        &self,
        canvas_id: String,
        channel_ids: Vec<ChannelId>,
        user_ids: Vec<UserId>,
    ) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state
            .canvases_access_delete_calls
            .push((canvas_id, channel_ids, user_ids));
        if let Some(err) = state.canvases_access_error.clone() {
            return Err(ApiError::Slack(err));
        }
        Ok(())
    }

    async fn canvas_comments_list(
        &self,
        _canvas_id: String,
    ) -> api::Result<Vec<api::CanvasCommentThread>> {
        let state = self.state.lock().unwrap();
        if let Some(err) = state.canvas_comments_error.clone() {
            return Err(ApiError::Slack(err));
        }
        Ok(state.canvas_comments.clone())
    }

    async fn canvas_comment_post(
        &self,
        canvas_id: String,
        text: String,
        thread_ts: Option<Ts>,
    ) -> api::Result<PostedMessage> {
        let mut state = self.state.lock().unwrap();
        state
            .canvas_comment_posts
            .push((canvas_id, text, thread_ts));
        if let Some(err) = state.canvas_comments_error.clone() {
            return Err(ApiError::Slack(err));
        }
        Ok(PostedMessage::default())
    }

    async fn quip_thread_archive(&self, channel_id: ChannelId, thread_ts: Ts) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.quip_archive_calls.push((channel_id, thread_ts, true));
        if let Some(err) = state.quip_archive_error.clone() {
            return Err(ApiError::Slack(err));
        }
        Ok(())
    }

    async fn quip_thread_unarchive(&self, channel_id: ChannelId, thread_ts: Ts) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state
            .quip_archive_calls
            .push((channel_id, thread_ts, false));
        if let Some(err) = state.quip_archive_error.clone() {
            return Err(ApiError::Slack(err));
        }
        Ok(())
    }

    async fn quip_thread_archive_all(&self, channel_id: ChannelId) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.quip_archive_all_calls.push(channel_id);
        if let Some(err) = state.quip_archive_error.clone() {
            return Err(ApiError::Slack(err));
        }
        Ok(())
    }

    async fn fetch_list(&self, _file_id: String) -> api::Result<api::SlackListData> {
        let state = self.state.lock().unwrap();
        if state.auth_expired_collab {
            return Err(ApiError::AuthExpired("token_revoked".into()));
        }
        Ok(state.list_data.clone())
    }

    async fn fetch_list_with_meta(
        &self,
        file_id: String,
    ) -> api::Result<(api::SlackListData, api::ListMeta)> {
        let state = self.state.lock().unwrap();
        if state.auth_expired_collab {
            return Err(ApiError::AuthExpired("token_revoked".into()));
        }
        let meta = state.list_meta.clone().unwrap_or_else(|| api::ListMeta {
            file_id: file_id.clone(),
            title: state.list_data.title.clone(),
            editable: true,
            comment_channel_id: api::list_comment_channel_id(&file_id),
            ..Default::default()
        });
        Ok((state.list_data.clone(), meta))
    }

    async fn lists_access_set(
        &self,
        list_id: String,
        access_level: api::ListAccessLevel,
        channel_ids: Vec<ChannelId>,
        user_ids: Vec<UserId>,
    ) -> api::Result<()> {
        let mut state = self.state.lock().unwrap();
        state
            .list_access_set_calls
            .push((list_id, access_level, channel_ids, user_ids));
        if let Some(err) = state.list_access_error.clone() {
            return Err(ApiError::Slack(err));
        }
        Ok(())
    }

    async fn lists_access_delete(
        &self,
        list_id: String,
        channel_ids: Vec<ChannelId>,
        user_ids: Vec<UserId>,
    ) -> api::Result<api::ListAccessDeleteResult> {
        let mut state = self.state.lock().unwrap();
        state
            .list_access_delete_calls
            .push((list_id, channel_ids, user_ids));
        if let Some(err) = state.list_access_error.clone() {
            return Err(ApiError::Slack(err));
        }
        Ok(state.list_access_delete_result.clone())
    }

    async fn lists_records_create(
        &self,
        list_id: String,
        _initial_fields: Vec<api::ListCellInput>,
    ) -> api::Result<api::SlackListRow> {
        self.state
            .lock()
            .unwrap()
            .list_create_item_calls
            .push(list_id);
        Ok(api::SlackListRow::default())
    }

    async fn lists_get_threads_for_record(
        &self,
        _list_id: String,
        _record_id: String,
    ) -> api::Result<Vec<Message>> {
        let state = self.state.lock().unwrap();
        if state.auth_expired_collab {
            return Err(ApiError::AuthExpired("token_revoked".into()));
        }
        Ok(state.list_record_threads.clone())
    }

    async fn load_app_home(&self, _channel: ChannelId) -> api::Result<api::AppHomeResponse> {
        if self.state.lock().unwrap().auth_expired_collab {
            return Err(ApiError::AuthExpired("token_revoked".into()));
        }
        Ok(api::AppHomeResponse::default())
    }
}

impl crate::backend::HuddleBackend for FakeBackend {
    async fn rooms_join(&self, channel: ChannelId) -> api::Result<api::RoomJoinResponse> {
        let (gate, response) = {
            let mut state = self.state.lock().unwrap();
            state.rooms_join_calls.push(channel);
            (
                state.rooms_join_gate.clone(),
                state.rooms_join_response.clone(),
            )
        };
        if let Some(gate) = gate {
            gate.notified().await;
        }
        response.ok_or_else(|| ApiError::UnsupportedHuddleBackend("huddles unavailable".to_owned()))
    }

    async fn rooms_notify_member(&self, channel: ChannelId, user: UserId) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .rooms_notify_calls
            .push((channel, user));
        Ok(())
    }

    async fn rooms_get_link(&self, workspace: TeamId, channel: ChannelId) -> api::Result<String> {
        if let Some(url) = self.state.lock().unwrap().rooms_get_link_override.clone() {
            return Ok(url);
        }
        Ok(crate::types::net::huddle_fallback_url(
            workspace.as_str(),
            channel.as_str(),
        ))
    }

    async fn rooms_rename(&self, room_id: String, name: String) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .rooms_rename_calls
            .push((room_id, name));
        Ok(())
    }

    async fn rooms_start_transcription(&self, call_id: String) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .transcription_calls
            .push((call_id, true));
        Ok(())
    }

    async fn rooms_stop_transcription(&self, call_id: String) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .transcription_calls
            .push((call_id, false));
        Ok(())
    }

    async fn rooms_invite_response(
        &self,
        channel: ChannelId,
        room_id: String,
        response: String,
    ) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .rooms_invite_responses
            .push((channel, room_id, response));
        Ok(())
    }

    async fn huddles_update_background(
        &self,
        huddle_id: String,
        background_id: String,
    ) -> api::Result<()> {
        self.state
            .lock()
            .unwrap()
            .huddles_bg_calls
            .push((huddle_id, background_id));
        Ok(())
    }
}

pub(crate) fn fake_core_parts() -> (SharedStore, Emitter, UnboundedReceiver<Event>) {
    let (tx, rx) = mpsc::unbounded();
    (
        Arc::new(Mutex::new(Store::open_in_memory().unwrap())),
        Emitter(tx),
        rx,
    )
}

fn c1() -> ChannelId {
    ChannelId::from("C1")
}

async fn next(rx: &mut UnboundedReceiver<Event>) -> Event {
    tokio::time::timeout(Duration::from_secs(5), rx.next())
        .await
        .expect("event within 5s")
        .expect("event stream open")
}

#[tokio::test]
async fn tab_loads_fetch_their_data_and_emit_it() {
    let fake = FakeBackend::default();
    fake.set_pins(vec![PinnedItem {
        kind: Some("message".to_owned()),
        message: Some(Message {
            ts: Ts::from("9.0"),
            text: "pinned".into(),
            ..Default::default()
        }),
        ..Default::default()
    }]);
    fake.set_bookmarks(vec![ChannelBookmark {
        id: "Bk1".to_owned(),
        title: "Runbook".to_owned(),
        link: Some("https://acme.test/runbook".to_owned()),
        kind: Some("link".to_owned()),
        ..Default::default()
    }]);
    fake.set_files(vec![File {
        id: FileId::from("F1"),
        name: Some("spec.pdf".to_owned()),
        ..Default::default()
    }]);
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake,
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Messaging(MessagingCommand::LoadPins(c1())))
        .await;
    let Event::PinsLoaded { channel, pins } = next(&mut rx).await else {
        panic!("expected pins");
    };
    assert_eq!(channel, c1());
    let pins = pins.expect("pins load");
    assert_eq!(
        pins[0].message.as_ref().map(|m| m.text.as_str()),
        Some("pinned")
    );

    core.handle(Command::Messaging(MessagingCommand::LoadBookmarks(c1())))
        .await;
    let Event::BookmarksLoaded { channel, bookmarks } = next(&mut rx).await else {
        panic!("expected bookmarks");
    };
    let bookmarks = bookmarks.expect("bookmarks load");
    assert_eq!((channel, bookmarks.len()), (c1(), 1));
    assert_eq!(bookmarks[0].title, "Runbook");

    core.handle(Command::Collab(CollabCommand::LoadFiles(
        FilesFilter::channel(c1()),
    )))
    .await;
    let Event::FilesLoaded { filter, files } = next(&mut rx).await else {
        panic!("expected files");
    };
    let files = files.expect("files load");
    assert_eq!(filter, FilesFilter::channel(c1()));
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].id.as_str(), "F1");

    // A workspace-wide load (the Files view) keeps its filter on the answer too.
    core.handle(Command::Collab(CollabCommand::LoadFiles(
        FilesFilter::workspace(),
    )))
    .await;
    let Event::FilesLoaded { filter, files } = next(&mut rx).await else {
        panic!("expected workspace files");
    };
    assert_eq!(filter, FilesFilter::workspace());
    assert_eq!(files.expect("files load").len(), 1);
}

#[tokio::test]
async fn failed_tab_load_answers_the_tab_with_the_error_and_reports_it() {
    let fake = FakeBackend::default();
    fake.fail_tab_loads();
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake,
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Collab(CollabCommand::LoadFiles(
        FilesFilter::channel(c1()),
    )))
    .await;

    let Event::FilesLoaded { filter, files } = next(&mut rx).await else {
        panic!("a failed load still answers its tab");
    };
    assert_eq!(filter, FilesFilter::channel(c1()));
    assert!(
        files.is_err_and(|error| error.contains("channel_not_found")),
        "the tab gets Slack's error instead of loading forever"
    );
    assert!(
        matches!(next(&mut rx).await, Event::Error(error) if error.starts_with("files.list")),
        "the failure is reported as usual"
    );
}

#[tokio::test]
async fn add_bookmark_adds_then_refetches_the_list() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Messaging(MessagingCommand::AddBookmark {
        channel: c1(),
        title: "Roadmap".into(),
        url: "https://acme.test/roadmap".into(),
    }))
    .await;

    let Event::BookmarksLoaded { channel, bookmarks } = next(&mut rx).await else {
        panic!("expected the refreshed list");
    };
    assert_eq!(channel, c1());
    let bookmarks = bookmarks.expect("the refreshed list");
    assert_eq!(
        bookmarks.last().map(|b| b.title.as_str()),
        Some("Roadmap"),
        "the refreshed list carries the added bookmark"
    );
    assert_eq!(
        fake.bookmark_adds(),
        vec![(
            c1(),
            "Roadmap".to_owned(),
            "https://acme.test/roadmap".to_owned()
        )]
    );
}

#[tokio::test]
async fn set_pinned_calls_the_pin_method_then_refetches_the_list() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;
    let ts = Ts::from("1508197641.000151");

    core.handle(Command::Messaging(MessagingCommand::SetPinned {
        channel: c1(),
        ts: ts.clone(),
        pinned: true,
    }))
    .await;

    let Event::PinsLoaded { channel, pins } = next(&mut rx).await else {
        panic!("expected the refreshed list");
    };
    assert_eq!(channel, c1());
    assert!(pins.is_ok(), "the refreshed list lands");
    assert_eq!(fake.pin_calls(), vec![(c1(), ts, true)]);
}

#[tokio::test]
async fn dnd_snooze_and_scheduled_messages_commands_emit_updates() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        store,
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Workspace(WorkspaceCommand::SetDndSnooze {
        minutes: 30,
    }))
    .await;
    let Event::SelfDndUpdated(status) = next(&mut rx).await else {
        panic!("expected SelfDndUpdated");
    };
    assert!(status.snooze_enabled);

    core.handle(Command::Workspace(WorkspaceCommand::EndDndSnooze))
        .await;
    let Event::SelfDndUpdated(status) = next(&mut rx).await else {
        panic!("expected SelfDndUpdated");
    };
    assert!(!status.snooze_enabled);

    core.handle(Command::Messaging(MessagingCommand::ScheduleMessage {
        channel: c1(),
        post_at: 1_760_010_000,
        content: MessageContent::plain("later"),
        thread_ts: None,
    }))
    .await;
    let Event::MessageScheduled {
        channel,
        scheduled_message_id,
        post_at,
    } = next(&mut rx).await
    else {
        panic!("expected MessageScheduled");
    };
    assert_eq!(channel, c1());
    assert_eq!(post_at, 1_760_010_000);
    let Event::ScheduledMessagesLoaded(Ok(list)) = next(&mut rx).await else {
        panic!("expected ScheduledMessagesLoaded");
    };
    assert_eq!(list.len(), 1);

    core.handle(Command::Messaging(
        MessagingCommand::DeleteScheduledMessage {
            channel: c1(),
            scheduled_message_id,
        },
    ))
    .await;
    let Event::ScheduledMessagesLoaded(Ok(list)) = next(&mut rx).await else {
        panic!("expected refreshed empty ScheduledMessagesLoaded");
    };
    assert!(list.is_empty());
}

#[tokio::test]
async fn create_channel_canvas_refreshes_the_conversation_and_announces_the_id() {
    let fake = FakeBackend::default();
    // The post-create `conversations.info` refresh carries the new canvas.
    fake.set_info(Conversation {
        properties: Some(ConversationProperties {
            canvas: Some(ChannelCanvas {
                file_id: Some(FileId::from("F07NEW")),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    });
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;
    let ws = TeamId::from("T1");
    store
        .lock()
        .unwrap()
        .upsert_conversation(
            &ws,
            &Conversation {
                id: c1(),
                name: Some("general".into()),
                ..Default::default()
            },
        )
        .unwrap();

    core.handle(Command::Collab(CollabCommand::CreateChannelCanvas(c1())))
        .await;

    assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
    assert_eq!(
        next(&mut rx).await,
        Event::ChannelCanvasCreated {
            channel: c1(),
            canvas_id: "F07NEW".to_owned(),
        }
    );
    assert_eq!(fake.canvas_creations(), vec![c1()]);
    let stored = store.lock().unwrap().conversation(&ws, &c1()).unwrap();
    let stored = stored.expect("the conversation stays stored");
    assert_eq!(
        stored
            .properties
            .and_then(|p| p.canvas)
            .and_then(|c| c.file_id),
        Some(FileId::from("F07NEW")),
        "the canvas id lands in the stored properties"
    );
    assert_eq!(
        stored.name.as_deref(),
        Some("general"),
        "merge keeps other fields"
    );
}

#[tokio::test]
async fn hydrate_fills_conversations() {
    let (store, events, mut rx) = fake_core_parts();
    let core = Core::new(
        FakeBackend::default(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;
    core.hydrate().await;
    assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
    assert_eq!(
        next(&mut rx).await,
        Event::UsersChanged(vec![UserId::from("U1")])
    );
    assert!(matches!(next(&mut rx).await, Event::Counts(_)));
    assert_eq!(next(&mut rx).await, Event::SidebarChanged);
    let store = store.lock().unwrap();
    let ws = TeamId::from("T1");
    assert_eq!(store.users(&ws).unwrap().len(), 1);
    assert_eq!(store.team(&ws).unwrap().unwrap().name, "Acme");
    let sections = store.sections(&ws).unwrap();
    let ids: Vec<_> = sections.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["L2", "L1"], "sections come back in display order");
    assert_eq!(
        store.prefs(&ws).unwrap(),
        Some(serde_json::json!({"display_real_names": true}))
    );
    assert_eq!(
        store.custom_emoji(&ws).unwrap(),
        vec![(
            "party".to_owned(),
            "https://emoji.example/party.png".to_owned()
        )]
    );
}

#[tokio::test]
async fn commands_update_the_store_and_report_back() {
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        FakeBackend::default(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Messaging(MessagingCommand::SendMessage {
        channel: c1(),
        content: MessageContent::plain("hi there"),
        thread_ts: None,
        broadcast: false,
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
    let history = store
        .lock()
        .unwrap()
        .channel_history(&TeamId::from("T1"), &c1(), None, 10)
        .unwrap();
    assert_eq!(history[0].message.text, "hi there");

    core.handle(Command::Collab(CollabCommand::Search("there".into())))
        .await;
    let Event::SearchResults { query, messages } = next(&mut rx).await else {
        panic!("expected local results first");
    };
    assert_eq!((query.as_str(), messages.len()), ("there", 1));
    assert!(
        matches!(next(&mut rx).await, Event::RemoteSearchResults { results, .. } if results.total == 7)
    );

    core.handle(Command::Messaging(MessagingCommand::MarkRead {
        channel: c1(),
        ts: Ts::from("1.0"),
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::AuthExpired("invalid_auth".into())
    );
}

#[tokio::test]
async fn open_thread_fetches_the_full_thread_into_the_store() {
    let fake = FakeBackend::default();
    fake.push_history(1..=5);
    fake.push_thread("5.0", 1..=3);
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Messaging(MessagingCommand::OpenThread {
        channel: c1(),
        thread_ts: Ts::from("5.0"),
    }))
    .await;
    // Root + 3 replies at PAGE messages per page: two committed, announced pages.
    assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
    assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });

    let thread = store
        .lock()
        .unwrap()
        .thread_replies(&TeamId::from("T1"), &c1(), &Ts::from("5.0"))
        .unwrap();
    let ts: Vec<_> = thread.iter().map(|m| m.message.ts.0.clone()).collect();
    assert_eq!(ts, ["5.0", "5.0.1", "5.0.2", "5.0.3"]);
    assert_eq!(
        fake.reply_calls(),
        vec![(Ts::from("5.0"), None), (Ts::from("5.0"), Some("2".into())),]
    );
}

#[tokio::test]
async fn following_a_thread_tells_slack_then_rereads_the_thread() {
    let fake = FakeBackend::default();
    fake.push_thread("5.0", 1..=1);
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Messaging(MessagingCommand::SetThreadSubscribed {
        channel: c1(),
        thread_ts: Ts::from("5.0"),
        last_read: Ts::from("5.0.1"),
        subscribed: true,
    }))
    .await;
    // The re-read commits the thread, which is what refreshes the root's follow flag.
    assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
    assert_eq!(
        fake.state.lock().unwrap().subscription_calls,
        vec![(Ts::from("5.0"), Ts::from("5.0.1"), true)]
    );
    assert_eq!(fake.reply_calls(), vec![(Ts::from("5.0"), None)]);
}

#[tokio::test]
async fn mark_thread_read_posts_thread_cursor() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Messaging(MessagingCommand::MarkThreadRead {
        channel: c1(),
        thread_ts: Ts::from("5.0"),
        ts: Ts::from("5.0.3"),
    }))
    .await;
    // Flush the spawned task by following with a synchronous-event command.
    core.handle(Command::Channel(ChannelCommand::FetchChannelManagers {
        channel: c1(),
    }))
    .await;
    let _ = next(&mut rx).await;

    assert_eq!(
        fake.state.lock().unwrap().thread_mark_calls,
        vec![(c1(), Ts::from("5.0"), Ts::from("5.0.3"))]
    );
}

#[tokio::test]
async fn fetching_channel_managers_announces_them() {
    let fake = FakeBackend::default();
    fake.state.lock().unwrap().managers = vec![UserId::from("U9")];
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake,
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Channel(ChannelCommand::FetchChannelManagers {
        channel: c1(),
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::ChannelManagers {
            channel: c1(),
            users: vec![UserId::from("U9")],
        }
    );
}

#[tokio::test]
async fn open_thread_reports_api_failures() {
    let fake = FakeBackend::default();
    fake.fail_replies();
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake,
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Messaging(MessagingCommand::OpenThread {
        channel: c1(),
        thread_ts: Ts::from("5.0"),
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::AuthExpired("invalid_auth".into())
    );
}

#[tokio::test]
async fn loop_applies_socket_events_in_order_and_stops_on_shutdown() {
    let (store, events, mut rx) = fake_core_parts();
    let core = Core::new(
        FakeBackend::default(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;
    let (command_tx, command_rx) = mpsc::unbounded();
    let (rtm_tx, rtm_rx) = tokio::sync::mpsc::channel(8);
    let task = tokio::spawn(run(core, command_rx, Some(rtm_rx)));

    rtm_tx
        .send(RtmEvent::State(ConnectionState::Connecting))
        .await
        .unwrap();
    let message: SlackEvent = serde_json::from_str(
        r#"{"type":"message","channel":"C1","user":"U1","text":"live","ts":"5.0"}"#,
    )
    .unwrap();
    rtm_tx.send(RtmEvent::Event(message)).await.unwrap();
    let archived: SlackEvent =
        serde_json::from_str(r#"{"type":"channel_archive","channel":"C1"}"#).unwrap();
    rtm_tx.send(RtmEvent::Event(archived)).await.unwrap();
    drop(rtm_tx);

    assert_eq!(
        next(&mut rx).await,
        Event::Connection(ConnectionState::Connecting)
    );
    assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
    // The live post also goes out as itself, for notifications.
    assert!(matches!(
        next(&mut rx).await,
        Event::MessagePosted { channel, message } if channel == c1() && message.text == "live"
    ));
    assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));

    // The socket stream ended; commands are still served.
    command_tx
        .unbounded_send(Command::Channel(ChannelCommand::OpenConversation(c1())))
        .unwrap();
    // Opening also spawns the open-time `conversations.info` merge, so its
    // `ConversationInfo` event may interleave with the backfill's `StoreChanged`.
    loop {
        match next(&mut rx).await {
            Event::StoreChanged { channel: c } if c == c1() => break,
            Event::ConversationInfo { .. }
            | Event::HistoryComplete { .. }
            | Event::StoreChanged { .. } => {}
            other => panic!("unexpected event after open: {other:?}"),
        }
    }
    command_tx.unbounded_send(Command::Shutdown).unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn open_conversation_merges_member_count_without_clobbering_stored_fields() {
    let fake = FakeBackend::default();
    fake.set_info(Conversation {
        num_members: Some(42),
        ..Default::default()
    });
    // 42 members with no stored profiles: all of them humans, so the count survives the
    // human-only re-count.
    fake.set_members((1..=42).map(|i| UserId::from(format!("U{i}"))).collect());
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake,
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;
    let ws = TeamId::from("T1");
    // Stored from `client.userBoot`: no member count, but richer unread and name state.
    store
        .lock()
        .unwrap()
        .upsert_conversation(
            &ws,
            &Conversation {
                id: c1(),
                name: Some("general".into()),
                is_channel: true,
                unread_count_display: Some(7),
                ..Default::default()
            },
        )
        .unwrap();

    core.handle(Command::Channel(ChannelCommand::OpenConversation(c1())))
        .await;

    loop {
        match next(&mut rx).await {
            Event::ConversationInfo {
                channel,
                num_members,
                ..
            } if channel == c1() => {
                assert_eq!(num_members, Some(42), "the event carries the count");
                break;
            }
            Event::StoreChanged { .. } | Event::HistoryComplete { .. } => {}
            other => panic!("unexpected event after open: {other:?}"),
        }
    }
    let stored = store.lock().unwrap().conversation(&ws, &c1()).unwrap();
    let stored = stored.expect("the conversation stays stored");
    assert_eq!(stored.num_members, Some(42), "member count lands from info");
    assert_eq!(stored.unread_count_display, Some(7), "boot fields survive");
    assert_eq!(stored.name.as_deref(), Some("general"));
}

#[tokio::test]
async fn open_conversation_counts_humans_and_apps_for_the_header() {
    let fake = FakeBackend::default();
    // Slack's raw count (17) includes apps and deactivated people; the header must not.
    fake.set_info(Conversation {
        num_members: Some(17),
        ..Default::default()
    });
    fake.set_members(vec![
        UserId::from("U1"),
        UserId::from("UBOT"),
        UserId::from("UGONE"),
        UserId::from("USLACKBOT"),
        UserId::from("UEXT"),
    ]);
    let (store, events, mut rx) = fake_core_parts();
    let ws = TeamId::from("T1");
    {
        let store = &mut *store.lock().unwrap();
        for (id, is_bot, deleted) in [
            ("U1", false, false),
            ("UBOT", true, false),
            ("UGONE", true, true),
        ] {
            store
                .upsert_user(
                    &ws,
                    &User {
                        id: UserId::from(id),
                        is_bot,
                        deleted,
                        ..Default::default()
                    },
                )
                .unwrap();
        }
    }
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        ws.clone(),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Channel(ChannelCommand::OpenConversation(c1())))
        .await;

    loop {
        match next(&mut rx).await {
            Event::ConversationInfo {
                channel,
                num_members,
                num_apps,
                members,
            } if channel == c1() => {
                assert_eq!(
                    num_members,
                    Some(2),
                    "U1 and the unknown UEXT are the humans"
                );
                assert_eq!(
                    num_apps,
                    Some(1),
                    "the live bot; UGONE is deactivated and USLACKBOT built-in"
                );
                assert_eq!(members.len(), 5, "the member id list rides along");
                break;
            }
            Event::StoreChanged { .. } | Event::HistoryComplete { .. } => {}
            other => panic!("unexpected event after open: {other:?}"),
        }
    }
    assert_eq!(fake.member_calls(), vec![c1()]);
    let stored = store
        .lock()
        .unwrap()
        .conversation(&ws, &c1())
        .unwrap()
        .expect("the conversation stays stored");
    assert_eq!(
        stored.num_members,
        Some(2),
        "the human count lands in the stored conversation"
    );
}

#[tokio::test]
async fn open_conversation_falls_back_to_infos_count_when_members_fail() {
    let fake = FakeBackend::default();
    fake.set_info(Conversation {
        num_members: Some(17),
        ..Default::default()
    });
    fake.fail_members();
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Channel(ChannelCommand::OpenConversation(c1())))
        .await;

    loop {
        match next(&mut rx).await {
            Event::ConversationInfo {
                channel,
                num_members,
                num_apps,
                members,
            } if channel == c1() => {
                assert_eq!(num_members, Some(17), "the raw count stands in");
                assert_eq!(num_apps, None, "no member list, no app count");
                assert!(members.is_empty(), "no member list, no ids either");
                break;
            }
            Event::StoreChanged { .. } | Event::HistoryComplete { .. } => {}
            other => panic!("unexpected event after open: {other:?}"),
        }
    }
    assert_eq!(fake.member_calls(), vec![c1()]);
}

#[tokio::test]
async fn open_conversation_of_connect_channel_resolves_the_connected_teams() {
    let fake = FakeBackend::default();
    fake.set_info(Conversation {
        is_ext_shared: true,
        num_members: Some(5),
        ..Default::default()
    });
    let members = vec![
        UserId::from("U1"),
        UserId::from("UEXT1"),
        UserId::from("UEXT2"),
    ];
    fake.set_members(members.clone());
    fake.set_team(Team {
        id: TeamId::from("T0EXT"),
        name: "Klarna".into(),
        domain: "klarna".into(),
        ..Default::default()
    });
    let (store, events, mut rx) = fake_core_parts();
    let ws = TeamId::from("T1");
    {
        let store = &mut *store.lock().unwrap();
        store
            .upsert_conversation(
                &ws,
                &Conversation {
                    id: c1(),
                    is_channel: true,
                    ..Default::default()
                },
            )
            .unwrap();
        for (id, team) in [("U1", "T1"), ("UEXT1", "T0EXT"), ("UEXT2", "T0EXT")] {
            store
                .upsert_user(
                    &ws,
                    &User {
                        id: UserId::from(id),
                        team_id: Some(TeamId::from(team)),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
    }
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        ws.clone(),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Channel(ChannelCommand::OpenConversation(c1())))
        .await;

    let mut announced = Vec::new();
    loop {
        match next(&mut rx).await {
            Event::SidebarChanged => break,
            Event::ConversationInfo { members, .. } => announced = members,
            Event::StoreChanged { .. } | Event::HistoryComplete { .. } => {}
            other => panic!("unexpected event after open: {other:?}"),
        }
    }
    assert_eq!(announced, members, "the member ids ride the announcement");
    assert_eq!(fake.team_info_calls(), vec![TeamId::from("T0EXT")]);
    let teams = store.lock().unwrap().teams(&ws).unwrap();
    assert_eq!(
        teams
            .iter()
            .map(|team| team.name.as_str())
            .collect::<Vec<_>>(),
        ["Klarna"],
        "the connected org's record lands beside the workspace's own"
    );
}

#[tokio::test]
async fn set_topic_and_purpose_merge_into_the_stored_conversation() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    let ws = TeamId::from("T1");
    store
        .lock()
        .unwrap()
        .upsert_conversation(
            &ws,
            &Conversation {
                id: c1(),
                name: Some("general".into()),
                is_channel: true,
                ..Default::default()
            },
        )
        .unwrap();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        ws.clone(),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Channel(ChannelCommand::SetChannelTopic {
        channel: c1(),
        topic: "Ship it".into(),
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
    core.handle(Command::Channel(ChannelCommand::SetChannelPurpose {
        channel: c1(),
        purpose: "Deploys at 14:00".into(),
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
    let stored = store
        .lock()
        .unwrap()
        .conversation(&ws, &c1())
        .unwrap()
        .expect("the conversation stays stored");
    assert_eq!(
        stored.topic.as_ref().map(|t| t.value.as_str()),
        Some("Ship it")
    );
    assert_eq!(
        stored.purpose.as_ref().map(|p| p.value.as_str()),
        Some("Deploys at 14:00")
    );
    assert_eq!(fake.topic_calls(), vec![(c1(), "Ship it".to_owned())]);
    assert_eq!(
        fake.purpose_calls(),
        vec![(c1(), "Deploys at 14:00".to_owned())]
    );
}

#[tokio::test]
async fn hydrate_backfills_previews_only_for_dms_without_stored_messages() {
    let fake = FakeBackend::with_history(1..=3);
    let (store, events, mut rx) = fake_core_parts();
    let ws = TeamId::from("T1");
    let (dm1, dm2) = (ChannelId::from("D1"), ChannelId::from("D2"));
    {
        let store = &mut *store.lock().unwrap();
        for id in [&dm1, &dm2] {
            store
                .upsert_conversation(
                    &ws,
                    &Conversation {
                        id: id.clone(),
                        is_im: true,
                        is_open: true,
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        store
            .upsert_message(
                &ws,
                &dm1,
                &Message {
                    ts: Ts::from("1700000000.000100"),
                    text: "already stored".into(),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        ws.clone(),
        events,
        test_media_tx(),
    )
    .await;
    core.hydrate().await;

    // The preview lands for the message-less DM through the message-store write path.
    loop {
        match next(&mut rx).await {
            Event::StoreChanged { channel } if channel == dm2 => break,
            Event::Conversations(_)
            | Event::UsersChanged(_)
            | Event::Counts(_)
            | Event::SidebarChanged
            | Event::ActivityView { .. }
            | Event::SavedLoaded(_)
            | Event::AppCommands(_) => {}
            other => panic!("unexpected hydrate event: {other:?}"),
        }
    }
    assert_eq!(
        fake.latest_calls(),
        vec![dm2.clone()],
        "only the DM with nothing stored is asked; dm1 and the C1 channel are skipped"
    );
    let latest = store
        .lock()
        .unwrap()
        .latest_messages(&ws, &[dm1.clone(), dm2.clone()])
        .unwrap();
    assert_eq!(
        latest[&dm2].message.text, "m3",
        "the newest message previews"
    );
    assert_eq!(latest[&dm1].message.text, "already stored");
    assert!(
        fake.calls().is_empty(),
        "the preview never walks full history pages"
    );
    assert!(
        !core.backfills.contains_key(&dm2),
        "no backfill was started"
    );

    // Opening the DM still walks the full history from the newest page: one stored
    // preview message neither satisfies nor completes it.
    core.handle(Command::Channel(ChannelCommand::OpenConversation(
        dm2.clone(),
    )))
    .await;
    loop {
        match next(&mut rx).await {
            Event::HistoryComplete { channel } if channel == dm2 => break,
            Event::StoreChanged { .. } | Event::ConversationInfo { .. } => {}
            other => panic!("unexpected event after open: {other:?}"),
        }
    }
    let calls = fake.calls();
    assert_eq!(
        calls.first(),
        Some(&(None, None)),
        "the walk starts at the newest page"
    );
    assert_eq!(
        calls.len(),
        2,
        "1..=3 at 2 per page: a full walk, not a limit-1 fetch"
    );
    let history = store
        .lock()
        .unwrap()
        .channel_history(&ws, &dm2, None, 10)
        .unwrap();
    assert_eq!(history.len(), 3, "the whole history is stored");
}

#[tokio::test]
async fn hydrate_lists_dms_and_previews_only_open_conversations() {
    let fake = FakeBackend::default();
    fake.push_history(1..=2);
    // A group DM too new for `client.userBoot`'s capped channel list, plus a closed DM.
    fake.set_dms(vec![
        Conversation {
            id: ChannelId::from("M1"),
            name: Some("mpdm-fredrik--linus--jakob-1".into()),
            is_mpim: true,
            is_open: true,
            ..Default::default()
        },
        Conversation {
            id: ChannelId::from("D3"),
            is_im: true,
            is_open: false,
            user: Some(UserId::from("U9")),
            ..Default::default()
        },
    ]);
    let (store, events, mut rx) = fake_core_parts();
    let ws = TeamId::from("T1");
    // A closed group DM that is stored: listed by neither Slack nor this preview fetch.
    store
        .lock()
        .unwrap()
        .upsert_conversation(
            &ws,
            &Conversation {
                id: ChannelId::from("M2"),
                is_mpim: true,
                ..Default::default()
            },
        )
        .unwrap();
    let core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        ws.clone(),
        events,
        test_media_tx(),
    )
    .await;
    core.hydrate().await;

    loop {
        match next(&mut rx).await {
            Event::StoreChanged { channel } if channel == ChannelId::from("M1") => break,
            Event::Conversations(_)
            | Event::UsersChanged(_)
            | Event::Counts(_)
            | Event::SidebarChanged
            | Event::ActivityView { .. }
            | Event::SavedLoaded(_)
            | Event::AppCommands(_) => {}
            other => panic!("unexpected hydrate event: {other:?}"),
        }
    }
    assert_eq!(fake.dm_calls(), 1);
    assert_eq!(
        fake.latest_calls(),
        vec![ChannelId::from("M1")],
        "only the open group DM is asked; the closed DM and the closed stored one are skipped"
    );
    let asked = [
        ChannelId::from("M1"),
        ChannelId::from("D3"),
        ChannelId::from("M2"),
    ];
    let latest = store.lock().unwrap().latest_messages(&ws, &asked).unwrap();
    assert!(latest.contains_key(&asked[0]), "the new group DM previews");
    assert!(!latest.contains_key(&asked[1]) && !latest.contains_key(&asked[2]));
    let m1 = store
        .lock()
        .unwrap()
        .conversation(&ws, &asked[0])
        .unwrap()
        .expect("the group DM userBoot never delivered lands in the store");
    assert_eq!(m1.name.as_deref(), Some("mpdm-fredrik--linus--jakob-1"));
    assert!(m1.is_open);
    assert!(
        store
            .lock()
            .unwrap()
            .conversation(&ws, &asked[1])
            .unwrap()
            .is_some(),
        "closed DMs are still stored for naming"
    );
}

#[tokio::test]
async fn hydrate_records_channel_memberships_from_users_conversations() {
    let fake = FakeBackend::default();
    // userBoot delivers C1 (its channel objects omit `is_member`); the externally hosted
    // Slack Connect channel and a previously-left one only exist in the store.
    fake.set_memberships(vec![c1(), ChannelId::from("CEXT")]);
    let (store, events, mut rx) = fake_core_parts();
    let ws = TeamId::from("T1");
    {
        let store = &mut *store.lock().unwrap();
        store
            .upsert_conversation(
                &ws,
                &Conversation {
                    id: ChannelId::from("CEXT"),
                    name: Some("partner_support_klarna".into()),
                    is_channel: true,
                    is_ext_shared: true,
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .upsert_conversation(
                &ws,
                &Conversation {
                    id: ChannelId::from("CLEFT"),
                    name: Some("avarda-checkout".into()),
                    is_channel: true,
                    is_member: true,
                    is_ext_shared: true,
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .upsert_conversation(
                &ws,
                &Conversation {
                    id: ChannelId::from("M1"),
                    is_mpim: true,
                    is_member: true,
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        ws.clone(),
        events,
        test_media_tx(),
    )
    .await;
    core.hydrate().await;

    async fn run_to_sidebar(rx: &mut UnboundedReceiver<Event>) -> Vec<ChannelId> {
        let mut conversations = Vec::new();
        loop {
            match next(rx).await {
                Event::SidebarChanged => break conversations,
                Event::Conversations(ids) => conversations.extend(ids),
                Event::UsersChanged(_)
                | Event::Counts(_)
                | Event::ActivityView { .. }
                | Event::SavedLoaded(_)
                | Event::AppCommands(_) => {}
                other => panic!("unexpected hydrate event: {other:?}"),
            }
        }
    }
    let mut announced = run_to_sidebar(&mut rx).await;
    let mut sorted = std::mem::take(&mut announced);
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted,
        vec![c1(), ChannelId::from("CEXT"), ChannelId::from("CLEFT")],
        "exactly the flipped channels are announced"
    );
    let stored = |id: &str| {
        store
            .lock()
            .unwrap()
            .conversation(&ws, &ChannelId::from(id))
            .unwrap()
            .unwrap_or_else(|| panic!("{id} stored"))
    };
    assert!(stored("C1").is_member, "boot's omitted flag filled in");
    assert!(
        stored("CEXT").is_member,
        "the externally hosted channel counts"
    );
    assert!(!stored("CLEFT").is_member, "left channels drop out");
    assert!(stored("M1").is_member, "group DM rows are never touched");
    assert_eq!(fake.membership_calls(), 1);

    // A re-hydrate replays userBoot before syncing memberships again. The boot upsert
    // preserves stored flags now, so they never dip and the second membership sync
    // flips nothing.
    core.hydrate().await;
    let announced_again = run_to_sidebar(&mut rx).await;
    assert_eq!(
        announced_again,
        vec![c1()],
        "boot still announces its rows; no membership flips a second time"
    );
    assert_eq!(fake.membership_calls(), 2);
    assert!(
        stored("C1").is_member,
        "the boot overwrite is re-synced after it"
    );
    assert!(stored("CEXT").is_member);
    assert!(!stored("CLEFT").is_member);
}

#[tokio::test]
async fn set_notify_level_writes_slack_prefs_then_mirrors_them() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    let ws = TeamId::from("T1");
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        ws.clone(),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Channel(ChannelCommand::SetNotifyLevel {
        channel: c1(),
        level: crate::NotifyLevel::Mentions,
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::SidebarChanged);
    assert_eq!(
        fake.notification_calls(),
        vec![(c1(), "muted", "false"), (c1(), "desktop", "mention")]
    );
    let prefs = store
        .lock()
        .unwrap()
        .prefs(&ws)
        .unwrap()
        .expect("prefs stored");
    assert_eq!(
        crate::notify_levels(&prefs).get(&c1()),
        Some(&crate::NotifyLevel::Mentions)
    );
}

#[tokio::test]
async fn leave_conversation_stores_it_as_left_once_slack_confirms() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    let ws = TeamId::from("T1");
    store
        .lock()
        .unwrap()
        .upsert_conversation(
            &ws,
            &Conversation {
                id: c1(),
                is_member: true,
                ..Default::default()
            },
        )
        .unwrap();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        ws.clone(),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Channel(ChannelCommand::LeaveConversation(c1())))
        .await;
    assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
    let stored = store.lock().unwrap().conversation(&ws, &c1()).unwrap();
    assert!(!stored.expect("stays stored").is_member, "stored as left");
    assert_eq!(fake.leave_calls(), vec![c1()]);
}

#[tokio::test]
async fn join_conversation_stores_it_as_joined_once_slack_confirms() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    let ws = TeamId::from("T1");
    let mut core = Core::new(
        fake,
        Arc::clone(&store),
        ws.clone(),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Channel(ChannelCommand::JoinConversation(c1())))
        .await;
    assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
    let stored = store.lock().unwrap().conversation(&ws, &c1()).unwrap();
    assert!(stored.expect("stored").is_member, "stored as joined");
}

#[tokio::test]
async fn resolve_users_stores_each_missing_profile_once() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;
    let ext = UserId::from("U9");

    core.handle(Command::Workspace(WorkspaceCommand::ResolveUsers(vec![
        ext.clone(),
        ext.clone(),
    ])))
    .await;
    assert_eq!(next(&mut rx).await, Event::UsersChanged(vec![ext.clone()]));
    let users = store.lock().unwrap().users(&TeamId::from("T1")).unwrap();
    let names: Vec<&str> = users.iter().map(|user| user.name.as_str()).collect();
    assert_eq!(names, ["ext-U9"], "the profile lands in the store");

    // Asked once per session: a repeat never reaches `users.info` again.
    core.handle(Command::Workspace(WorkspaceCommand::ResolveUsers(vec![
        ext.clone(),
    ])))
    .await;
    assert_eq!(fake.user_info_calls(), vec![ext]);
}

#[tokio::test]
async fn resolve_users_skips_profiles_the_store_already_has() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    let ws = TeamId::from("T1");
    // A full record already in the store (a login-time snapshot): passing its id again -
    // a member list names everyone - asks for nothing.
    store
        .lock()
        .unwrap()
        .upsert_user(
            &ws,
            &User {
                id: UserId::from("U1"),
                name: "stored".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        ws,
        events,
        test_media_tx(),
    )
    .await;

    let ext = UserId::from("U9");
    core.handle(Command::Workspace(WorkspaceCommand::ResolveUsers(vec![
        UserId::from("U1"),
        ext.clone(),
    ])))
    .await;
    assert_eq!(next(&mut rx).await, Event::UsersChanged(vec![ext.clone()]));
    assert_eq!(
        fake.user_info_calls(),
        vec![ext],
        "only the unknown id is asked"
    );
}

#[tokio::test]
async fn resolve_users_learns_the_organizations_of_the_people_found() {
    let fake = FakeBackend::default();
    let ext = UserId::from("UEXT");
    fake.set_user_team(ext.clone(), TeamId::from("T0EXT"));
    fake.set_team(Team {
        id: TeamId::from("T0EXT"),
        name: "Klarna".into(),
        ..Default::default()
    });
    let (store, events, mut rx) = fake_core_parts();
    let ws = TeamId::from("T1");
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        ws.clone(),
        events,
        test_media_tx(),
    )
    .await;

    // The external person's profile lands first, then their organization's record, so a
    // Slack Connect channel can name it on its first visit.
    core.handle(Command::Workspace(WorkspaceCommand::ResolveUsers(vec![
        ext.clone(),
    ])))
    .await;
    assert_eq!(next(&mut rx).await, Event::UsersChanged(vec![ext]));
    assert_eq!(next(&mut rx).await, Event::SidebarChanged);
    assert_eq!(fake.team_info_calls(), vec![TeamId::from("T0EXT")]);
    let names: Vec<String> = store
        .lock()
        .unwrap()
        .teams(&ws)
        .unwrap()
        .into_iter()
        .map(|team| team.name)
        .collect();
    assert_eq!(names, ["Klarna"]);
}

#[tokio::test]
async fn open_conversation_announces_an_empty_member_list_when_info_fails() {
    let fake = FakeBackend::default();
    fake.fail_info();
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Channel(ChannelCommand::OpenConversation(c1())))
        .await;

    loop {
        match next(&mut rx).await {
            Event::ConversationInfo {
                channel,
                num_members,
                num_apps,
                members,
            } if channel == c1() => {
                assert_eq!((num_members, num_apps), (None, None), "nothing is known");
                assert!(
                    members.is_empty(),
                    "front-ends stop waiting on a member list"
                );
                break;
            }
            Event::StoreChanged { .. } | Event::HistoryComplete { .. } | Event::Error(_) => {}
            other => panic!("unexpected event after open: {other:?}"),
        }
    }
    assert!(
        fake.member_calls().is_empty(),
        "no member list without the channel's info"
    );
}

#[tokio::test]
async fn open_dm_fetches_peer_dnd_and_presence_once_each() {
    let fake = FakeBackend::default();
    fake.set_dnd(
        UserId::from("U2"),
        DndStatus {
            dnd_enabled: true,
            next_dnd_start_ts: Some(0),
            next_dnd_end_ts: Some(i64::MAX),
            ..Default::default()
        },
    );
    fake.set_presence(
        UserId::from("U2"),
        UserPresence {
            presence: Presence::Active,
            online: Some(true),
            auto_away: None,
            manual_away: None,
            connection_count: None,
            last_activity: None,
        },
    );
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;
    let ws = TeamId::from("T1");
    let dm = ChannelId::from("D1");
    store
        .lock()
        .unwrap()
        .upsert_conversation(
            &ws,
            &Conversation {
                id: dm.clone(),
                is_im: true,
                is_open: true,
                user: Some(UserId::from("U2")),
                ..Default::default()
            },
        )
        .unwrap();

    core.handle(Command::Channel(ChannelCommand::OpenConversation(
        dm.clone(),
    )))
    .await;
    let (mut dnd, mut presence) = (false, false);
    for _ in 0..8 {
        match next(&mut rx).await {
            Event::DndPaused { user, paused } => {
                assert_eq!(user.as_str(), "U2");
                assert!(paused, "inside the enabled window");
                dnd = true;
            }
            Event::Presence { users, presence: p } => {
                assert_eq!(users, vec![UserId::from("U2")]);
                assert_eq!(p, Presence::Active);
                presence = true;
            }
            Event::StoreChanged { .. }
            | Event::HistoryComplete { .. }
            | Event::Conversations(_)
            | Event::ConversationInfo { .. } => {}
            other => panic!("unexpected event after open: {other:?}"),
        }
        if dnd && presence {
            break;
        }
    }
    assert!(dnd && presence, "both peer states arrive");

    // Re-opening re-checks DND (transient by design) but never re-polls a presence the
    // websocket subscription already covers.
    core.handle(Command::Channel(ChannelCommand::OpenConversation(dm)))
        .await;
    loop {
        match next(&mut rx).await {
            Event::DndPaused { paused, .. } if paused => break,
            Event::StoreChanged { .. }
            | Event::HistoryComplete { .. }
            | Event::Conversations(_)
            | Event::ConversationInfo { .. } => {}
            other => panic!("unexpected event after reopen: {other:?}"),
        }
    }
    assert_eq!(fake.presence_calls(), vec![UserId::from("U2")]);
    assert_eq!(fake.dnd_calls().len(), 2, "dnd re-fetched per open");
}

#[tokio::test]
async fn fetch_threads_emits_the_view() {
    let fake = FakeBackend::default();
    fake.set_threads_view(api::ThreadsView {
        total_unread_replies: 2,
        threads: vec![api::ThreadSubscription {
            root: Message {
                ts: Ts::from("5.0"),
                text: "root".into(),
                ..Default::default()
            },
            latest_replies: vec![Message {
                ts: Ts::from("5.0.1"),
                text: "reply".into(),
                ..Default::default()
            }],
            unread_replies: Vec::new(),
        }],
        ..Default::default()
    });
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake,
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Messaging(MessagingCommand::FetchThreads))
        .await;

    let Event::ThreadsView(view) = next(&mut rx).await else {
        panic!("expected the threads view");
    };
    assert_eq!(view.total_unread_replies, 2);
    assert_eq!(view.threads[0].root.ts.as_str(), "5.0");
    assert_eq!(view.threads[0].latest_replies[0].text, "reply");
}

#[tokio::test]
async fn fetch_activity_hydrates_rows_and_marks_route() {
    let fake = FakeBackend::default();
    let feed = api::ActivityFeed {
        items: vec![
            api::ActivityItem {
                kind: api::ActivityKind::Mention,
                unread: true,
                feed_ts: Ts::from("9.1"),
                key: "at_user-C1-9.1".into(),
                channel: ChannelId::from("C1"),
                ts: Ts::from("9.1"),
                actor: Some(UserId::from("U2")),
                ..Default::default()
            },
            api::ActivityItem {
                kind: api::ActivityKind::Dm,
                feed_ts: Ts::from("8.1"),
                key: "dm-D2".into(),
                channel: ChannelId::from("D2"),
                ts: Ts::from("8.1"),
                message: Some(Message {
                    ts: Ts::from("8.1"),
                    text: "inline".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ],
        next_cursor: None,
    };
    let mut bodies = HashMap::new();
    bodies.insert(
        ChannelId::from("C1"),
        vec![Message {
            ts: Ts::from("9.1"),
            text: "hydrated <@U1>".into(),
            ..Default::default()
        }],
    );
    fake.set_activity(feed, bodies);
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake,
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    // Without bodies (the boot fetch): the ref row stays empty, the inline one keeps its
    // body, and the badge state still lands.
    core.handle(Command::Messaging(MessagingCommand::FetchActivity {
        with_bodies: false,
    }))
    .await;
    let Event::ActivityView { feed } = next(&mut rx).await else {
        panic!("expected the activity view");
    };
    let feed = feed.expect("the fake feed answers");
    assert!(feed.items[0].message.is_none());
    assert_eq!(feed.items[1].message.as_ref().unwrap().text, "inline");

    // With bodies (the view opened): the ref row is hydrated by (channel, ts).
    core.handle(Command::Messaging(MessagingCommand::FetchActivity {
        with_bodies: true,
    }))
    .await;
    let Event::ActivityView { feed } = next(&mut rx).await else {
        panic!("expected the hydrated activity view");
    };
    let feed = feed.expect("the fake feed answers");
    assert_eq!(
        feed.items[0].message.as_ref().unwrap().text,
        "hydrated <@U1>"
    );

    core.handle(Command::Messaging(MessagingCommand::MarkActivity {
        entries: vec![feed.items[0].entry()],
        read: true,
    }))
    .await;
    // The mark runs detached; a further fetch's event (same single-threaded runtime,
    // FIFO polling) only lands after the earlier spawn completed.
    core.handle(Command::Messaging(MessagingCommand::FetchActivity {
        with_bodies: false,
    }))
    .await;
    assert!(matches!(
        next(&mut rx).await,
        Event::ActivityView { feed: Ok(_) }
    ));
    let marks = core.backend.activity_marks();
    assert_eq!(marks.len(), 1);
    assert!(marks[0].0);
    assert_eq!(marks[0].1[0].kind, "at_user");
}

#[tokio::test]
async fn open_dm_stores_and_announces_the_conversation() {
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        FakeBackend::default(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Channel(ChannelCommand::OpenDm {
        users: vec![UserId::from("U2")],
    }))
    .await;

    assert_eq!(
        next(&mut rx).await,
        Event::Conversations(vec![ChannelId::from("D9")])
    );
    assert_eq!(
        next(&mut rx).await,
        Event::ConversationOpened(ChannelId::from("D9"))
    );
    let stored = store
        .lock()
        .unwrap()
        .conversation(&TeamId::from("T1"), &ChannelId::from("D9"))
        .unwrap()
        .expect("the DM lands in the store");
    assert_eq!(stored.user, Some(UserId::from("U2")));
}

#[tokio::test]
async fn open_group_dm_stores_mpim_conversation() {
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        FakeBackend::default(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Channel(ChannelCommand::OpenDm {
        users: vec![UserId::from("U2"), UserId::from("U3")],
    }))
    .await;

    assert_eq!(
        next(&mut rx).await,
        Event::Conversations(vec![ChannelId::from("G9")])
    );
    assert_eq!(
        next(&mut rx).await,
        Event::ConversationOpened(ChannelId::from("G9"))
    );
    let stored = store
        .lock()
        .unwrap()
        .conversation(&TeamId::from("T1"), &ChannelId::from("G9"))
        .unwrap()
        .expect("the group DM lands in the store");
    assert!(stored.is_mpim);
    assert_eq!(stored.members, vec![UserId::from("U2"), UserId::from("U3")]);
}

#[tokio::test]
async fn upload_files_calls_backend_and_emits_uploaded_count() {
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        FakeBackend::default(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Collab(CollabCommand::UploadFiles {
        channel: ChannelId::from("C1"),
        thread_ts: None,
        content: Some(MessageContent::plain("attached")),
        files: vec![
            api::UploadFile {
                filename: "a.png".into(),
                bytes: vec![1, 2, 3],
            },
            api::UploadFile {
                filename: "b.txt".into(),
                bytes: vec![4, 5],
            },
        ],
    }))
    .await;

    assert_eq!(
        next(&mut rx).await,
        Event::FilesUploaded {
            channel: ChannelId::from("C1"),
            count: 2,
        }
    );
}

#[tokio::test]
async fn create_section_reloads_the_stored_sections() {
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        FakeBackend::default(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Channel(ChannelCommand::CreateSection {
        name: "Quiet".into(),
    }))
    .await;

    assert_eq!(next(&mut rx).await, Event::SidebarChanged);
    let sections = store.lock().unwrap().sections(&TeamId::from("T1")).unwrap();
    assert!(
        sections
            .iter()
            .any(|section| section.id == "LNEW" && section.name == "Quiet"),
        "the created section is back from the section re-read"
    );
}

#[tokio::test]
async fn rename_section_resends_the_stored_emoji_and_reloads() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    store
        .lock()
        .unwrap()
        .replace_sections(&TeamId::from("T1"), &base_sections())
        .unwrap();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Channel(ChannelCommand::RenameSection {
        section: "L2".into(),
        name: "Roadmap".into(),
    }))
    .await;

    assert_eq!(next(&mut rx).await, Event::SidebarChanged);
    // `.set` clears every omitted key, so the stored emoji rides along with the name.
    assert_eq!(
        fake.section_renames(),
        vec![("L2".to_owned(), "Roadmap".to_owned(), "rocket".to_owned())]
    );
    let sections = store.lock().unwrap().sections(&TeamId::from("T1")).unwrap();
    let renamed = sections.iter().find(|section| section.id == "L2").unwrap();
    assert_eq!(
        (renamed.name.as_str(), renamed.emoji.as_str()),
        ("Roadmap", "rocket")
    );
}

#[tokio::test]
async fn delete_section_removes_it_and_reloads() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Channel(ChannelCommand::DeleteSection {
        section: "L2".into(),
    }))
    .await;

    assert_eq!(next(&mut rx).await, Event::SidebarChanged);
    assert_eq!(fake.section_deletes(), vec!["L2".to_owned()]);
    let sections = store.lock().unwrap().sections(&TeamId::from("T1")).unwrap();
    assert!(sections.iter().all(|section| section.id != "L2"));
}

#[tokio::test]
async fn move_channel_sends_the_remove_side_and_reloads() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    store
        .lock()
        .unwrap()
        .replace_sections(&TeamId::from("T1"), &base_sections())
        .unwrap();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    // Out of the custom section, back to the Channels bucket: a remove only.
    core.handle(Command::Channel(ChannelCommand::MoveChannel {
        channel: c1(),
        from: Some("L2".into()),
        to: None,
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::SidebarChanged);
    assert_eq!(
        fake.section_moves(),
        vec![(None, Some(("L2".to_owned(), c1())))]
    );
    let sections = store.lock().unwrap().sections(&TeamId::from("T1")).unwrap();
    assert!(
        sections
            .iter()
            .find(|section| section.id == "L2")
            .unwrap()
            .channel_ids
            .is_empty(),
        "the re-read no longer lists the channel in the section"
    );

    // Into a custom section: an insert only ("Priority" lost C1 above).
    core.handle(Command::Channel(ChannelCommand::MoveChannel {
        channel: c1(),
        from: None,
        to: Some("L2".into()),
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::SidebarChanged);
    assert_eq!(
        fake.section_moves(),
        vec![
            (None, Some(("L2".to_owned(), c1()))),
            (Some(("L2".to_owned(), c1())), None),
        ]
    );
}

#[tokio::test]
async fn create_section_with_channel_moves_the_channel_into_it() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    store
        .lock()
        .unwrap()
        .replace_sections(&TeamId::from("T1"), &base_sections())
        .unwrap();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Channel(ChannelCommand::CreateSectionWithChannel {
        name: "Quiet".into(),
        channel: c1(),
    }))
    .await;

    assert_eq!(next(&mut rx).await, Event::SidebarChanged);
    // The channel sat in "Priority": the move carries a remove beside the insert.
    assert_eq!(
        fake.section_moves(),
        vec![(
            Some(("LNEW".to_owned(), c1())),
            Some(("L2".to_owned(), c1()))
        )]
    );
    let sections = store.lock().unwrap().sections(&TeamId::from("T1")).unwrap();
    let fresh = sections
        .iter()
        .find(|section| section.id == "LNEW")
        .unwrap();
    assert_eq!(fresh.channel_ids, vec![c1()]);
}

#[tokio::test]
async fn fetch_latest_history_stores_one_page_without_marking_read() {
    let fake = FakeBackend::with_history(1..=60);
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Channel(ChannelCommand::FetchLatestHistory(c1())))
        .await;

    assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
    let state = fake.state.lock().unwrap();
    assert_eq!(state.recent_calls, vec![(c1(), 50)]);
    assert!(state.mark_calls.is_empty());
    assert!(state.calls.is_empty(), "no multi-page backfill walk");
    drop(state);

    let stored = store
        .lock()
        .unwrap()
        .channel_history(&TeamId::from("T1"), &c1(), None, 100)
        .unwrap();
    assert_eq!(stored.len(), 50);
}

#[tokio::test]
async fn offline_queue_buffers_and_flushes_on_reconnect() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    // Drop offline via RTM reconnecting state.
    let reconnecting = ConnectionState::Reconnecting {
        attempt: 1,
        delay: Duration::from_secs(2),
        reason: "network down".into(),
    };
    core.on_rtm(RtmEvent::State(reconnecting.clone())).await;
    assert_eq!(next(&mut rx).await, Event::Connection(reconnecting));

    // Queue a message, reaction, and read-mark while offline.
    core.handle(Command::Messaging(MessagingCommand::SendMessage {
        channel: c1(),
        content: MessageContent::plain("first offline"),
        thread_ts: None,
        broadcast: false,
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::OfflineQueueChanged(1));

    core.handle(Command::Messaging(MessagingCommand::AddReaction {
        channel: c1(),
        ts: Ts::from("100.0"),
        name: "thumbsup".into(),
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::OfflineQueueChanged(2));

    core.handle(Command::Messaging(MessagingCommand::MarkRead {
        channel: c1(),
        ts: Ts::from("100.0"),
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::OfflineQueueChanged(3));

    // Nothing should have hit the backend yet.
    {
        let state = fake.state.lock().unwrap();
        assert!(state.posted_messages.is_empty());
        assert!(state.reaction_adds.is_empty());
        assert!(state.mark_calls.is_empty());
    }

    // Reconnect: queue drains in FIFO order and notifies 0 queued.
    let connected = ConnectionState::Connected {
        fast_reconnect: false,
    };
    core.on_rtm(RtmEvent::State(connected.clone())).await;
    assert_eq!(next(&mut rx).await, Event::OfflineQueueChanged(0));
    assert_eq!(next(&mut rx).await, Event::Connection(connected));
    // Wait for the spawned SendMessage (`StoreChanged`) and `MarkRead` (`AuthExpired`).
    let mut saw_store_changed = false;
    let mut saw_auth_expired = false;
    for _ in 0..2 {
        match next(&mut rx).await {
            Event::StoreChanged { channel } if channel == c1() => saw_store_changed = true,
            Event::AuthExpired(code) if code == "invalid_auth" => saw_auth_expired = true,
            _ => {}
        }
    }
    assert!(saw_store_changed);
    assert!(saw_auth_expired);
    wait_until("offline flush tasks complete", || {
        let state = fake.state.lock().unwrap();
        state.posted_messages.len() == 1
            && state.reaction_adds.len() == 1
            && state.mark_calls.len() == 1
    })
    .await;

    let state = fake.state.lock().unwrap();
    assert_eq!(
        state.posted_messages,
        vec![(c1(), "first offline".to_owned())]
    );
    assert_eq!(
        state.reaction_adds,
        vec![(c1(), Ts::from("100.0"), "thumbsup".to_owned())]
    );
    assert_eq!(state.mark_calls, vec![(c1(), Ts::from("100.0"))]);
}

#[tokio::test]
async fn offline_queue_persists_across_restart_and_retries_or_discards() {
    let fake = FakeBackend::default();
    fake.state.lock().unwrap().fail_post_message = true;
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    let offline = ConnectionState::Disconnected {
        reason: "offline".into(),
    };
    core.on_rtm(RtmEvent::State(offline.clone())).await;
    assert_eq!(next(&mut rx).await, Event::Connection(offline));

    core.handle(Command::Messaging(MessagingCommand::SendMessage {
        channel: c1(),
        content: MessageContent::plain("survives restart"),
        thread_ts: None,
        broadcast: false,
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::OfflineQueueChanged(1));

    // Simulate app restart sharing the same SQLite store.
    drop(core);
    let (tx2, mut rx2) = mpsc::unbounded();
    let mut core2 = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        Emitter(tx2),
        test_media_tx(),
    )
    .await;
    // Core::new announces the restored persisted queue count immediately.
    assert_eq!(next(&mut rx2).await, Event::OfflineQueueChanged(1));

    // Reconnect while post_message is still failing -> emits OfflineMutationFailed and keeps row.
    let connected = ConnectionState::Connected {
        fast_reconnect: false,
    };
    core2.on_rtm(RtmEvent::State(connected.clone())).await;
    assert_eq!(next(&mut rx2).await, Event::OfflineQueueChanged(0));
    assert_eq!(next(&mut rx2).await, Event::Connection(connected));
    let failed = match next(&mut rx2).await {
        Event::OfflineMutationFailed(f) => f,
        other => panic!("expected OfflineMutationFailed, got {other:?}"),
    };
    assert!(failed.summary.contains("survives restart"));
    assert_eq!(
        store
            .lock()
            .unwrap()
            .list_offline(&TeamId::from("T1"))
            .unwrap()
            .len(),
        1
    );

    // Now backend recovers -> RetryOfflineMutation succeeds and removes the SQLite row.
    fake.state.lock().unwrap().fail_post_message = false;
    core2
        .handle(Command::Messaging(MessagingCommand::RetryOfflineMutation(
            failed.id,
        )))
        .await;
    assert_eq!(next(&mut rx2).await, Event::StoreChanged { channel: c1() });
    wait_until("offline queue row removed after retry", || {
        store
            .lock()
            .unwrap()
            .list_offline(&TeamId::from("T1"))
            .unwrap()
            .is_empty()
    })
    .await;
}

/// Poll `pred` until true (5ms cadence, 2s ceiling) to avoid flaky fixed sleeps on busy CI runners.
async fn wait_until(what: &str, mut pred: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if pred() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for: {what}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test(start_paused = true)]
async fn hydrate_prefetches_unread_dms_before_channels_and_skips_up_to_date_store() {
    use crate::api::ConversationCounts;
    use crate::types::Attachment;

    let fake = FakeBackend::default();
    let ims = vec![
        // DM1 is already up-to-date in local SQLite (`latest == 50.0`).
        ConversationCounts {
            id: ChannelId::from("D_UPTODATE"),
            latest: Some(Ts::from("50.0")),
            dm_count: 1,
            has_unreads: true,
            ..Default::default()
        },
        // DM2 has 2 mentions (highest priority DM).
        ConversationCounts {
            id: ChannelId::from("D_HOT"),
            latest: Some(Ts::from("40.0")),
            dm_count: 2,
            has_unreads: true,
            ..Default::default()
        },
        // DM3 has 1 mention (second priority DM).
        ConversationCounts {
            id: ChannelId::from("D_WARM"),
            latest: Some(Ts::from("30.0")),
            dm_count: 1,
            has_unreads: true,
            ..Default::default()
        },
    ];

    let mut channels = Vec::new();
    // Mentioned channel must precede plain channels.
    channels.push(ConversationCounts {
        id: ChannelId::from("C_MENTION"),
        latest: Some(Ts::from("10.0")),
        mention_count: 3,
        has_unreads: true,
        ..Default::default()
    });
    // 25 plain unread channels with ascending timestamps -> total unread = 29 (>25 cap).
    // Plain channels C_PLAIN_25 down to C_PLAIN_05 should be in the top 25, while the oldest 4
    // (C_PLAIN_01..=04) are truncated by the 25-item cap.
    for i in 1..=25u32 {
        channels.push(ConversationCounts {
            id: ChannelId::from(format!("C_PLAIN_{i:02}")),
            latest: Some(Ts::from(format!("{:04}.0", i))),
            has_unreads: true,
            ..Default::default()
        });
    }
    fake.set_counts(ClientCounts {
        ims,
        channels,
        ..Default::default()
    });
    // Put a message with an unfurl in D_HOT so prefetch also triggers unfurl resolution.
    {
        let mut state = fake.state.lock().unwrap();
        state.history.push(Message {
            ts: Ts::from("99.0"),
            text: "fresh unread".into(),
            ..Default::default()
        });
        state.recent_by_channel.insert(
            ChannelId::from("D_HOT"),
            vec![Message {
                ts: Ts::from("40.0"),
                text: "dm with unfurl".into(),
                attachments: vec![Attachment {
                    is_msg_unfurl: true,
                    channel_id: Some(ChannelId::from("CEXT_PREF")),
                    ..Default::default()
                }],
                ..Default::default()
            }],
        );
        state.info = Some(Conversation {
            name: Some("foreign-prefetched".into()),
            ..Default::default()
        });
    }

    let (store, events, mut rx) = fake_core_parts();
    // Pre-seed D_UPTODATE with ts 50.0 so it is skipped.
    store
        .lock()
        .unwrap()
        .upsert_message(
            &TeamId::from("T1"),
            &ChannelId::from("D_UPTODATE"),
            &Message {
                ts: Ts::from("50.0"),
                text: "already stored".into(),
                ..Default::default()
            },
        )
        .unwrap();

    let core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;
    core.hydrate().await;

    // Wait until all 24 prefetched channels (25 cap minus 1 skipped up-to-date DM) emit
    // StoreChanged AND the unfurl conversation CEXT_PREF is announced.
    let mut store_changed = Vec::new();
    let mut saw_unfurl = false;
    while store_changed.len() < 24 || !saw_unfurl {
        match next(&mut rx).await {
            Event::StoreChanged { channel } => store_changed.push(channel),
            Event::Conversations(ids) if ids.contains(&ChannelId::from("CEXT_PREF")) => {
                saw_unfurl = true;
            }
            _ => {}
        }
    }

    let fetched: Vec<ChannelId> = fake
        .state
        .lock()
        .unwrap()
        .recent_calls
        .iter()
        .map(|(ch, _)| ch.clone())
        .collect();
    assert_eq!(fetched.len(), 24, "25-cap minus 1 already up-to-date DM");
    assert!(
        !fetched.contains(&ChannelId::from("D_UPTODATE")),
        "up-to-date DM skipped"
    );
    assert_eq!(fetched[0], ChannelId::from("D_HOT"));
    assert_eq!(fetched[1], ChannelId::from("D_WARM"));
    assert_eq!(fetched[2], ChannelId::from("C_MENTION"));
    assert_eq!(fetched[3], ChannelId::from("C_PLAIN_25"));
    assert!(!fetched.contains(&ChannelId::from("C_PLAIN_01")));
    assert!(!fetched.contains(&ChannelId::from("C_PLAIN_04")));
    assert!(saw_unfurl);
}

#[tokio::test(start_paused = true)]
async fn reconnect_refreshes_counts_and_prefetches_missed_messages() {
    use crate::api::ConversationCounts;

    let fake = FakeBackend::with_history(1..=2);
    fake.set_counts(ClientCounts {
        ims: vec![ConversationCounts {
            id: ChannelId::from("D_LAG"),
            latest: Some(Ts::from("20.0")),
            dm_count: 1,
            has_unreads: true,
            ..Default::default()
        }],
        channels: vec![
            ConversationCounts {
                id: ChannelId::from("C_SYNCED"),
                latest: Some(Ts::from("10.0")),
                has_unreads: true,
                ..Default::default()
            },
            ConversationCounts {
                id: ChannelId::from("C_LAG"),
                latest: Some(Ts::from("15.0")),
                has_unreads: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    let (store, events, mut rx) = fake_core_parts();
    // C_SYNCED already has ts 10.0 stored.
    store
        .lock()
        .unwrap()
        .upsert_message(
            &TeamId::from("T1"),
            &ChannelId::from("C_SYNCED"),
            &Message {
                ts: Ts::from("10.0"),
                text: "caught up".into(),
                ..Default::default()
            },
        )
        .unwrap();

    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    // 1st Connected: no reconnect counts refresh yet.
    core.on_rtm(RtmEvent::State(ConnectionState::Connected {
        fast_reconnect: false,
    }))
    .await;
    assert!(matches!(next(&mut rx).await, Event::Connection(_)));
    tokio::task::yield_now().await;
    assert_eq!(fake.counts_calls(), 0);

    // Disconnected -> 2nd Connected: triggers counts() and prefetches lagging channels only.
    core.on_rtm(RtmEvent::State(ConnectionState::Disconnected {
        reason: "sleep".into(),
    }))
    .await;
    assert!(matches!(next(&mut rx).await, Event::Connection(_)));

    core.on_rtm(RtmEvent::State(ConnectionState::Connected {
        fast_reconnect: true,
    }))
    .await;
    assert!(matches!(next(&mut rx).await, Event::Connection(_)));

    let mut saw_counts = false;
    let mut changed = Vec::new();
    while !saw_counts || changed.len() < 2 {
        match next(&mut rx).await {
            Event::Counts(_) => saw_counts = true,
            Event::StoreChanged { channel } => changed.push(channel),
            _ => {}
        }
    }
    assert_eq!(fake.counts_calls(), 1);
    assert_eq!(
        changed,
        vec![ChannelId::from("D_LAG"), ChannelId::from("C_LAG")]
    );
    let fetched: Vec<ChannelId> = fake
        .state
        .lock()
        .unwrap()
        .recent_calls
        .iter()
        .map(|(ch, _)| ch.clone())
        .collect();
    assert!(!fetched.contains(&ChannelId::from("C_SYNCED")));
}

#[tokio::test]
async fn bot_profile_caching_shares_cache_dedups_and_persists_across_paths() {
    let fake = FakeBackend::default();
    {
        let mut state = fake.state.lock().unwrap();
        state.bots.insert(
            BotId::from("B1"),
            BotProfile {
                id: BotId::from("B1"),
                name: "Giphy".into(),
                ..Default::default()
            },
        );
        state.bots.insert(
            BotId::from("B2"),
            BotProfile {
                id: BotId::from("B2"),
                name: "DeployBot".into(),
                ..Default::default()
            },
        );
        state.fail_bots.insert(BotId::from("BFAIL"));
    }
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    // 1. RTM Message does NOT block on `bots.info`: StoreChanged + MessagePosted land immediately,
    //    then the background `bots.info` task persists B1 to SQLite, updates the stored message,
    //    and emits StoreChanged + BotProfileLoaded.
    let ev1: SlackEvent = serde_json::from_str(
        r#"{"type":"message","channel":"C1","ts":"1.0","text":"hi","bot_id":"B1"}"#,
    )
    .unwrap();
    core.on_rtm(RtmEvent::Event(ev1)).await;
    assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
    assert!(matches!(next(&mut rx).await, Event::MessagePosted { .. }));
    assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
    assert!(matches!(
        next(&mut rx).await,
        Event::BotProfileLoaded { bot_id, profile }
            if bot_id == BotId::from("B1") && profile.name == "Giphy"
    ));
    let stored_b1 = store
        .lock()
        .unwrap()
        .message(&TeamId::from("T1"), &c1(), &Ts::from("1.0"))
        .unwrap()
        .unwrap();
    assert_eq!(
        stored_b1
            .message
            .bot_profile
            .as_ref()
            .map(|p| p.name.as_str()),
        Some("Giphy")
    );
    // Persisted in SQLite!
    assert_eq!(
        store
            .lock()
            .unwrap()
            .bot_profiles(&TeamId::from("T1"))
            .unwrap()
            .get(&BotId::from("B1"))
            .map(|p| p.name.as_str()),
        Some("Giphy")
    );

    // 2. ResolveBots(B1) hits memory cache immediately with 0 extra `bots.info` calls.
    core.handle(Command::Workspace(WorkspaceCommand::ResolveBots(vec![
        BotId::from("B1"),
    ])))
    .await;
    assert!(matches!(
        next(&mut rx).await,
        Event::BotProfileLoaded { bot_id, profile }
            if bot_id == BotId::from("B1") && profile.name == "Giphy"
    ));
    assert_eq!(
        fake.state.lock().unwrap().bot_info_calls,
        vec![BotId::from("B1")]
    );

    // 3. ResolveBots(B2) fetches B2 AND populates `core.bots`, so subsequent RTM Changed
    //    attaches B2 from cache synchronously without another `bots.info` call.
    core.handle(Command::Workspace(WorkspaceCommand::ResolveBots(vec![
        BotId::from("B2"),
    ])))
    .await;
    assert!(matches!(
        next(&mut rx).await,
        Event::BotProfileLoaded { bot_id, profile }
            if bot_id == BotId::from("B2") && profile.name == "DeployBot"
    ));
    let ev_changed: SlackEvent = serde_json::from_str(
        r#"{"type":"message","subtype":"message_changed","channel":"C1","event_ts":"2.0",
            "message":{"ts":"1.0","text":"edited","bot_id":"B2"}}"#,
    )
    .unwrap();
    core.on_rtm(RtmEvent::Event(ev_changed)).await;
    assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
    let stored_msg = store
        .lock()
        .unwrap()
        .message(&TeamId::from("T1"), &c1(), &Ts::from("1.0"))
        .unwrap()
        .unwrap();
    assert_eq!(
        stored_msg
            .message
            .bot_profile
            .as_ref()
            .map(|p| p.name.as_str()),
        Some("DeployBot")
    );
    assert_eq!(
        fake.state.lock().unwrap().bot_info_calls,
        vec![BotId::from("B1"), BotId::from("B2")]
    );

    // 4. Failed `bots.info` for BFAIL is recorded in `bots_asked` so a second RTM message does NOT retry.
    let ev_fail1: SlackEvent = serde_json::from_str(
        r#"{"type":"message","channel":"C1","ts":"3.0","text":"f1","bot_id":"BFAIL"}"#,
    )
    .unwrap();
    let ev_fail2: SlackEvent = serde_json::from_str(
        r#"{"type":"message","channel":"C1","ts":"4.0","text":"f2","bot_id":"BFAIL"}"#,
    )
    .unwrap();
    core.on_rtm(RtmEvent::Event(ev_fail1)).await;
    assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
    assert!(matches!(next(&mut rx).await, Event::MessagePosted { .. }));
    assert!(matches!(next(&mut rx).await, Event::Error(e) if e.contains("bots.info")));

    core.on_rtm(RtmEvent::Event(ev_fail2)).await;
    // No second Event::Error for BFAIL!
    assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
    assert!(matches!(next(&mut rx).await, Event::MessagePosted { .. }));
    assert_eq!(
        fake.state
            .lock()
            .unwrap()
            .bot_info_calls
            .iter()
            .filter(|id| **id == BotId::from("BFAIL"))
            .count(),
        1
    );

    // 5. Restarting Core on the same SQLite store preloads B1 & B2 so ResolveBots makes 0 API calls.
    let calls_before = fake.state.lock().unwrap().bot_info_calls.len();
    let (tx2, mut rx2) = mpsc::unbounded();
    let mut core2 = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        Emitter(tx2),
        test_media_tx(),
    )
    .await;
    core2
        .handle(Command::Workspace(WorkspaceCommand::ResolveBots(vec![
            BotId::from("B1"),
            BotId::from("B2"),
        ])))
        .await;
    assert!(
        matches!(next(&mut rx2).await, Event::BotProfileLoaded { bot_id, .. } if bot_id == BotId::from("B1"))
    );
    assert!(
        matches!(next(&mut rx2).await, Event::BotProfileLoaded { bot_id, .. } if bot_id == BotId::from("B2"))
    );
    assert_eq!(
        fake.state.lock().unwrap().bot_info_calls.len(),
        calls_before
    );
}

#[tokio::test]
async fn huddle_commands_and_lifecycle_races() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    // 1. JoinHuddle error (UnsupportedHuddleBackend) emits Event::Error and resets `joining`.
    core.handle(Command::Huddle(HuddleCommand::Join {
        channel: c1(),
        audio: crate::huddle::HuddleAudioConfig::default(),
    }))
    .await;
    assert!(matches!(next(&mut rx).await, Event::Error(e) if e.contains("rooms.join")));
    assert!(!core.huddle.lock().unwrap().joining);

    // 2. Concurrent duplicate JoinHuddle while rooms_join is in-flight only calls rooms_join once,
    //    and LeaveHuddle during in-flight rooms_join prevents activating the session.
    let gate = Arc::new(tokio::sync::Notify::new());
    {
        let mut state = fake.state.lock().unwrap();
        state.rooms_join_calls.clear();
        state.rooms_join_gate = Some(Arc::clone(&gate));
        state.rooms_join_response = Some(api::RoomJoinResponse {
            call_id: "R01".into(),
            ..Default::default()
        });
    }
    core.handle(Command::Huddle(HuddleCommand::Join {
        channel: c1(),
        audio: crate::huddle::HuddleAudioConfig::default(),
    }))
    .await;
    // Second concurrent JoinHuddle is ignored while `joining == true`.
    core.handle(Command::Huddle(HuddleCommand::Join {
        channel: ChannelId::from("C2"),
        audio: crate::huddle::HuddleAudioConfig::default(),
    }))
    .await;
    tokio::task::yield_now().await;
    assert_eq!(fake.state.lock().unwrap().rooms_join_calls, vec![c1()]);

    // LeaveHuddle cancels the in-flight join before rooms_join returns.
    core.handle(Command::Huddle(HuddleCommand::Leave)).await;
    gate.notify_waiters();
    wait_until("cancelled join clears joining flag", || {
        !core.huddle.lock().unwrap().joining
    })
    .await;
    {
        let slot = core.huddle.lock().unwrap();
        assert!(
            slot.handle.is_none(),
            "cancelled join must not install handle"
        );
        assert!(
            slot.call_id.is_none(),
            "cancelled join must not install call_id"
        );
    }

    // 3. CopyHuddleLink (fallback default & custom API URL), InviteToHuddle, DeclineHuddleInvite.
    core.handle(Command::Huddle(HuddleCommand::GetLink { channel: c1() }))
        .await;
    assert_eq!(
        next(&mut rx).await,
        Event::Huddle(HuddleEvent::LinkLoaded {
            channel: c1(),
            url: "https://app.slack.com/huddle/T1/C1".into(),
        })
    );
    fake.state.lock().unwrap().rooms_get_link_override =
        Some("https://app.slack.com/huddle/T1/C1/custom".into());
    core.handle(Command::Huddle(HuddleCommand::GetLink { channel: c1() }))
        .await;
    assert_eq!(
        next(&mut rx).await,
        Event::Huddle(HuddleEvent::LinkLoaded {
            channel: c1(),
            url: "https://app.slack.com/huddle/T1/C1/custom".into(),
        })
    );

    core.handle(Command::Huddle(HuddleCommand::Invite {
        channel: c1(),
        users: vec![UserId::from("U2"), UserId::from("U3")],
    }))
    .await;
    core.handle(Command::Huddle(HuddleCommand::DeclineInvite {
        channel: c1(),
        call_id: "R99".into(),
    }))
    .await;

    // 4. SetHuddleTopic / Background / Captions are no-ops when no active call_id.
    core.handle(Command::Huddle(HuddleCommand::SetTopic("No-op".into())))
        .await;
    core.handle(Command::Huddle(HuddleCommand::SetBackground(
        "aurora".into(),
    )))
    .await;
    core.handle(Command::Huddle(HuddleCommand::SetCaptions(true)))
        .await;
    wait_until("huddle invite and decline calls recorded", || {
        let state = fake.state.lock().unwrap();
        state.rooms_notify_calls.len() == 2 && state.rooms_invite_responses.len() == 1
    })
    .await;
    {
        let state = fake.state.lock().unwrap();
        assert_eq!(
            state.rooms_notify_calls,
            vec![(c1(), UserId::from("U2")), (c1(), UserId::from("U3"))]
        );
        assert_eq!(
            state.rooms_invite_responses,
            vec![(c1(), "R99".into(), "decline".into())]
        );
        assert!(state.rooms_rename_calls.is_empty());
        assert!(state.huddles_bg_calls.is_empty());
        assert!(state.transcription_calls.is_empty());
    }

    // With an active call_id, SetHuddleTopic / Background / Captions invoke REST endpoints.
    core.huddle.lock().unwrap().call_id = Some("RACTIVE".into());
    core.handle(Command::Huddle(HuddleCommand::SetTopic(
        "Sprint sync".into(),
    )))
    .await;
    core.handle(Command::Huddle(HuddleCommand::SetBackground(
        "aurora".into(),
    )))
    .await;
    core.handle(Command::Huddle(HuddleCommand::SetCaptions(true)))
        .await;
    core.handle(Command::Huddle(HuddleCommand::SetCaptions(false)))
        .await;
    wait_until("active huddle control calls recorded", || {
        let state = fake.state.lock().unwrap();
        state.rooms_rename_calls.len() == 1
            && state.huddles_bg_calls.len() == 1
            && state.transcription_calls.len() == 2
    })
    .await;
    {
        let state = fake.state.lock().unwrap();
        assert_eq!(
            state.rooms_rename_calls,
            vec![("RACTIVE".into(), "Sprint sync".into())]
        );
        assert_eq!(
            state.huddles_bg_calls,
            vec![("RACTIVE".into(), "aurora".into())]
        );
        assert_eq!(
            state.transcription_calls,
            vec![("RACTIVE".into(), true), ("RACTIVE".into(), false)]
        );
    }
}

#[tokio::test]
async fn drafts_sync_lifecycle_concurrency_delete_failure_and_fetch_reconciliation() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    // 1. Concurrent SyncDraft calls on C1 serialize so drafts_create runs ONCE, then drafts_update.
    let gate = Arc::new(tokio::sync::Notify::new());
    fake.state.lock().unwrap().draft_create_gate = Some(Arc::clone(&gate));
    core.handle(Command::Messaging(MessagingCommand::SyncDraft {
        channel: c1(),
        text: "first".into(),
    }))
    .await;
    wait_until("first SyncDraft entered drafts_create", || {
        fake.state.lock().unwrap().draft_creates.len() == 1
    })
    .await;
    core.handle(Command::Messaging(MessagingCommand::SyncDraft {
        channel: c1(),
        text: "second".into(),
    }))
    .await;
    tokio::task::yield_now().await;
    gate.notify_waiters();
    wait_until("second SyncDraft completed as drafts_update", || {
        fake.state.lock().unwrap().draft_updates.len() == 1
    })
    .await;
    fake.state.lock().unwrap().draft_create_gate = None;
    {
        let state = fake.state.lock().unwrap();
        assert_eq!(state.draft_creates.len(), 1, "no double create");
        assert_eq!(state.draft_updates.len(), 1);
        assert_eq!(state.draft_updates[0].0, "Dr01");
        assert_eq!(state.draft_updates[0].2, "second");
    }

    // 2. Delete failure retains the channel in cloud_draft_ids; subsequent delete succeeds and removes it.
    fake.state.lock().unwrap().fail_draft_delete = true;
    core.handle(Command::Messaging(MessagingCommand::SyncDraft {
        channel: c1(),
        text: "   ".into(),
    }))
    .await;
    wait_until("failed drafts_delete recorded", || {
        fake.state.lock().unwrap().draft_deletes.len() == 1
    })
    .await;
    // Allow post-delete cache update to run before asserting retention.
    tokio::task::yield_now().await;
    assert_eq!(
        core.cloud_draft_ids
            .lock()
            .unwrap()
            .get(&c1())
            .map(String::as_str),
        Some("Dr01"),
        "failed delete must keep draft id cached"
    );

    fake.state.lock().unwrap().fail_draft_delete = false;
    core.handle(Command::Messaging(MessagingCommand::SyncDraft {
        channel: c1(),
        text: "".into(),
    }))
    .await;
    wait_until("successful drafts_delete removes cached draft id", || {
        !core.cloud_draft_ids.lock().unwrap().contains_key(&c1())
    })
    .await;

    // 3. FetchCloudDrafts seeds cloud_draft_ids and reconciles/removes stale entries.
    core.cloud_draft_ids
        .lock()
        .unwrap()
        .insert(ChannelId::from("CSTALE"), "DrStale".into());
    fake.state.lock().unwrap().cloud_drafts = vec![api::CloudDraft {
        id: "DrFresh".into(),
        channel: ChannelId::from("C2"),
        text: "remote draft".into(),
        updated_secs: Some(100),
    }];
    core.handle(Command::Messaging(MessagingCommand::FetchCloudDrafts))
        .await;
    let Event::CloudDrafts(drafts) = next(&mut rx).await else {
        panic!("expected CloudDrafts");
    };
    assert_eq!(drafts.len(), 1);
    let map = core.cloud_draft_ids.lock().unwrap();
    assert!(
        !map.contains_key(&ChannelId::from("CSTALE")),
        "stale entry cleared"
    );
    assert_eq!(
        map.get(&ChannelId::from("C2")).map(String::as_str),
        Some("DrFresh")
    );
}

#[tokio::test]
async fn offline_queue_edge_cases_discard_remove_reaction_mark_read_summary_and_corrupt_row() {
    // 1. QueuedMutation::summary UTF-8 truncation and variants.
    let long_utf8 = "🎉".repeat(55);
    let m_long = QueuedMutation::SendMessage {
        channel: c1(),
        content: MessageContent::plain(long_utf8),
        thread_ts: None,
        broadcast: false,
    };
    let s_long = m_long.summary();
    assert!(s_long.ends_with("…\""));
    assert_eq!(s_long.chars().filter(|&c| c == '🎉').count(), 48);

    let m_empty = QueuedMutation::SendMessage {
        channel: c1(),
        content: MessageContent::plain("   "),
        thread_ts: None,
        broadcast: false,
    };
    assert_eq!(m_empty.summary(), "Message");
    assert_eq!(
        QueuedMutation::RemoveReaction {
            channel: c1(),
            ts: Ts::from("1.0"),
            name: "wave".into(),
        }
        .summary(),
        "Remove reaction :wave:"
    );
    assert_eq!(
        QueuedMutation::MarkRead {
            channel: c1(),
            ts: Ts::from("1.0"),
        }
        .summary(),
        "Mark read (C1)"
    );

    // 2. Corrupt JSON row in SQLite `offline_queue` is skipped cleanly on Core::new while valid rows load.
    let fake = FakeBackend::default();
    fake.state.lock().unwrap().mark_read_ok = true;
    let (store, events, mut rx) = fake_core_parts();
    {
        let mut s = store.lock().unwrap();
        s.upsert_conversation(
            &TeamId::from("T1"),
            &Conversation {
                id: c1(),
                last_read: Some(Ts::from("1.0")),
                ..Default::default()
            },
        )
        .unwrap();
        s.enqueue_offline(&TeamId::from("T1"), "{not-valid-json")
            .unwrap();
        let valid_payload = serde_json::to_string(&QueuedMutation::RemoveReaction {
            channel: c1(),
            ts: Ts::from("5.0"),
            name: "eyes".into(),
        })
        .unwrap();
        s.enqueue_offline(&TeamId::from("T1"), &valid_payload)
            .unwrap();
    }
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;
    assert_eq!(next(&mut rx).await, Event::OfflineQueueChanged(1));

    // Go offline: queue MarkRead and a SendMessage, then discard the SendMessage while offline.
    core.on_rtm(RtmEvent::State(ConnectionState::Disconnected {
        reason: "down".into(),
    }))
    .await;
    assert!(matches!(next(&mut rx).await, Event::Connection(_)));

    core.handle(Command::Messaging(MessagingCommand::MarkRead {
        channel: c1(),
        ts: Ts::from("99.0"),
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::OfflineQueueChanged(2));

    core.handle(Command::Messaging(MessagingCommand::SendMessage {
        channel: c1(),
        content: MessageContent::plain("discard me"),
        thread_ts: None,
        broadcast: false,
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::OfflineQueueChanged(3));
    let discard_id = core.offline_queue.back().unwrap().0;
    core.handle(Command::Messaging(
        MessagingCommand::DiscardOfflineMutation(discard_id),
    ))
    .await;
    assert_eq!(next(&mut rx).await, Event::OfflineQueueChanged(2));

    // RetryOfflineMutation while offline does NOT fire a network call immediately.
    let mark_id = core.offline_queue.back().unwrap().0;
    core.handle(Command::Messaging(MessagingCommand::RetryOfflineMutation(
        mark_id,
    )))
    .await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(fake.state.lock().unwrap().mark_calls.is_empty());

    // Reconnect: flushes RemoveReaction + MarkRead (and updates SQLite conv.last_read to 99.0!).
    core.on_rtm(RtmEvent::State(ConnectionState::Connected {
        fast_reconnect: false,
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::OfflineQueueChanged(0));
    assert!(matches!(next(&mut rx).await, Event::Connection(_)));
    wait_until("offline queue flush updates store last_read", || {
        store
            .lock()
            .unwrap()
            .conversation(&TeamId::from("T1"), &c1())
            .unwrap()
            .and_then(|c| c.last_read)
            .as_ref()
            .map(Ts::as_str)
            == Some("99.0")
    })
    .await;
    {
        let state = fake.state.lock().unwrap();
        assert_eq!(
            state.reaction_removes,
            vec![(c1(), Ts::from("5.0"), "eyes".into())]
        );
        assert_eq!(state.mark_calls, vec![(c1(), Ts::from("99.0"))]);
        assert!(state.posted_messages.is_empty());
    }
    let stored_conv = store
        .lock()
        .unwrap()
        .conversation(&TeamId::from("T1"), &c1())
        .unwrap()
        .unwrap();
    assert_eq!(stored_conv.last_read, Some(Ts::from("99.0")));

    // 3. DiscardOfflineMutation after a failed flush deletes the SQLite row.
    fake.state.lock().unwrap().fail_post_message = true;
    core.on_rtm(RtmEvent::State(ConnectionState::Disconnected {
        reason: "down2".into(),
    }))
    .await;
    assert!(matches!(next(&mut rx).await, Event::Connection(_)));
    core.handle(Command::Messaging(MessagingCommand::SendMessage {
        channel: c1(),
        content: MessageContent::plain("will fail"),
        thread_ts: None,
        broadcast: false,
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::OfflineQueueChanged(1));
    core.on_rtm(RtmEvent::State(ConnectionState::Connected {
        fast_reconnect: false,
    }))
    .await;
    let mut failed_id = None;
    for _ in 0..4 {
        if let Event::OfflineMutationFailed(f) = next(&mut rx).await {
            failed_id = Some(f.id);
            break;
        }
    }
    let failed_id = failed_id.expect("expected OfflineMutationFailed");
    core.handle(Command::Messaging(
        MessagingCommand::DiscardOfflineMutation(failed_id),
    ))
    .await;
    assert_eq!(next(&mut rx).await, Event::OfflineQueueChanged(0));
    assert!(
        store
            .lock()
            .unwrap()
            .list_offline(&TeamId::from("T1"))
            .unwrap()
            .iter()
            .all(|(id, _)| *id != failed_id)
    );
}

#[tokio::test]
async fn saved_later_workflows_merge_dedup_hydrate_and_mutate() {
    use crate::api::{SavedItem, SavedItemState, SavedMark};

    let fake = FakeBackend::default();
    {
        let mut state = fake.state.lock().unwrap();
        state.saved_by_filter.insert(
            "saved",
            vec![SavedItem {
                channel: c1(),
                ts: Ts::from("1.0"),
                state: SavedItemState::InProgress,
                date_due: None,
                date_created: Some(10),
                message: None,
            }],
        );
        // Completed has a duplicate of (C1, 1.0) plus a unique item (C1, 2.0).
        state.saved_by_filter.insert(
            "completed",
            vec![
                SavedItem {
                    channel: c1(),
                    ts: Ts::from("1.0"),
                    state: SavedItemState::Completed,
                    date_due: None,
                    date_created: Some(10),
                    message: None,
                },
                SavedItem {
                    channel: c1(),
                    ts: Ts::from("2.0"),
                    state: SavedItemState::Completed,
                    date_due: None,
                    date_created: Some(20),
                    message: None,
                },
            ],
        );
        // Archived fails; load_saved_items tolerates errors on completed/archived.
        state.fail_saved_filters.insert("archived");
        state.activity_bodies.insert(
            c1(),
            vec![
                Message {
                    ts: Ts::from("1.0"),
                    text: "saved body 1".into(),
                    ..Default::default()
                },
                Message {
                    ts: Ts::from("2.0"),
                    text: "saved body 2".into(),
                    ..Default::default()
                },
            ],
        );
    }
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    // 1. LoadSaved merges, deduplicates, tolerates archived failure, and hydrates bodies.
    core.handle(Command::Messaging(MessagingCommand::LoadSaved))
        .await;
    let Event::SavedLoaded(Ok(items)) = next(&mut rx).await else {
        panic!("expected SavedLoaded(Ok)");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0].message.as_ref().map(|m| m.text.as_str()),
        Some("saved body 1")
    );
    assert_eq!(
        items[1].message.as_ref().map(|m| m.text.as_str()),
        Some("saved body 2")
    );

    // 2. SaveItem with "already_saved" + Some(date_due) falls back to saved_update.
    fake.state.lock().unwrap().saved_add_error = Some("already_saved".into());
    core.handle(Command::Messaging(MessagingCommand::SaveItem {
        channel: c1(),
        ts: Ts::from("1.0"),
        date_due: Some(500),
    }))
    .await;
    assert!(matches!(next(&mut rx).await, Event::SavedLoaded(Ok(_))));
    assert_eq!(
        fake.state.lock().unwrap().saved_update_calls,
        vec![(c1(), Ts::from("1.0"), None, Some(Some(500)))]
    );

    // 3. SaveItem with "already_saved" + None does NOT call saved_update again.
    core.handle(Command::Messaging(MessagingCommand::SaveItem {
        channel: c1(),
        ts: Ts::from("1.0"),
        date_due: None,
    }))
    .await;
    assert!(matches!(next(&mut rx).await, Event::SavedLoaded(Ok(_))));
    assert_eq!(fake.state.lock().unwrap().saved_update_calls.len(), 1);

    // 4. SaveFile (normal & "already_exists"), UpdateSavedItem, DeleteSavedItem, ClearCompletedSaved.
    fake.state.lock().unwrap().save_file_error = Some("already_exists".into());
    core.handle(Command::Messaging(MessagingCommand::SaveFile {
        file_id: "F99".into(),
        date_due: Some(700),
    }))
    .await;
    assert!(matches!(next(&mut rx).await, Event::SavedLoaded(Ok(_))));

    core.handle(Command::Messaging(MessagingCommand::UpdateSavedItem {
        channel: c1(),
        ts: Ts::from("1.0"),
        mark: Some(SavedMark::Completed),
        date_due: Some(None),
    }))
    .await;
    assert!(matches!(next(&mut rx).await, Event::SavedLoaded(Ok(_))));

    core.handle(Command::Messaging(MessagingCommand::DeleteSavedItem {
        channel: c1(),
        ts: Ts::from("2.0"),
    }))
    .await;
    assert!(matches!(next(&mut rx).await, Event::SavedLoaded(Ok(_))));

    core.handle(Command::Messaging(MessagingCommand::ClearCompletedSaved))
        .await;
    assert!(matches!(next(&mut rx).await, Event::SavedLoaded(Ok(_))));

    let state = fake.state.lock().unwrap();
    assert_eq!(state.save_file_calls, vec![("F99".into(), Some(700))]);
    assert_eq!(state.saved_delete_calls, vec![(c1(), Ts::from("2.0"))]);
    assert_eq!(state.saved_clear_completed_calls, 1);
}

#[tokio::test]
async fn rtm_handlers_dnd_huddles_emoji_unknown_and_posted_subtypes() {
    let fake = FakeBackend::default();
    {
        let mut state = fake.state.lock().unwrap();
        state.custom_emoji_override = Some(HashMap::from([(
            "shipit".to_owned(),
            "https://emoji.example/shipit.png".to_owned(),
        )]));
        state.app_commands = vec![api::AppCommand {
            name: "deploy".into(),
            usage: "[env]".into(),
            description: "Deploy app".into(),
            app_name: Some("Deployer".into()),
            app_id: Some("A1".into()),
        }];
    }
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    // DndUpdated & DndUpdatedUser
    let dnd_self: SlackEvent = serde_json::from_str(
        r#"{"type":"dnd_updated","dnd_status":{"snooze_enabled":true,"snooze_endtime":9999999999}}"#,
    )
    .unwrap();
    core.on_rtm(RtmEvent::Event(dnd_self)).await;
    assert!(matches!(next(&mut rx).await, Event::SelfDndUpdated(s) if s.snooze_enabled));

    let dnd_user: SlackEvent = serde_json::from_str(
        r#"{"type":"dnd_updated_user","user":"U2","dnd_status":{"snooze_enabled":true,"snooze_endtime":9999999999}}"#,
    )
    .unwrap();
    core.on_rtm(RtmEvent::Event(dnd_user)).await;
    assert_eq!(
        next(&mut rx).await,
        Event::DndPaused {
            user: UserId::from("U2"),
            paused: true,
        }
    );

    // ShRoomJoin / ShRoomUpdate / ShRoomLeave
    let room_join: SlackEvent = serde_json::from_str(
        r#"{"type":"sh_room_join","room":{"id":"R1","channels":["C1"],"participants":["U1","U2"],"name":"Design","background_id":"bg1","thread_root_ts":"10.0","has_ended":false}}"#,
    )
    .unwrap();
    core.on_rtm(RtmEvent::Event(room_join)).await;
    assert_eq!(
        next(&mut rx).await,
        Event::Huddle(HuddleEvent::Updated {
            channel: c1(),
            participants: vec![UserId::from("U1"), UserId::from("U2")],
            ended: false,
            topic: Some("Design".into()),
            background_id: Some("bg1".into()),
            thread_root_ts: Some(Ts::from("10.0")),
        })
    );

    let room_leave: SlackEvent = serde_json::from_str(
        r#"{"type":"sh_room_leave","room":{"id":"R1","channels":["C1"],"participants":[],"has_ended":true}}"#,
    )
    .unwrap();
    core.on_rtm(RtmEvent::Event(room_leave)).await;
    assert!(matches!(
        next(&mut rx).await,
        Event::Huddle(HuddleEvent::Updated { channel, ended: true, .. }) if channel == c1()
    ));

    // HuddleInvite & HuddleInviteCancel
    let inv: SlackEvent = serde_json::from_str(
        r#"{"type":"huddle_invite","channel_id":"D1","sender_user_id":"U2","call_id":"R2"}"#,
    )
    .unwrap();
    core.on_rtm(RtmEvent::Event(inv)).await;
    assert_eq!(
        next(&mut rx).await,
        Event::Huddle(HuddleEvent::Invite {
            channel: ChannelId::from("D1"),
            inviter: UserId::from("U2"),
            call_id: "R2".into(),
        })
    );

    let cancel: SlackEvent =
        serde_json::from_str(r#"{"type":"huddle_invite_cancel","channel_id":"D1"}"#).unwrap();
    core.on_rtm(RtmEvent::Event(cancel)).await;
    assert_eq!(
        next(&mut rx).await,
        Event::Huddle(HuddleEvent::InviteCancelled {
            channel: Some(ChannelId::from("D1")),
            call_id: None,
        })
    );

    // EmojiChanged refreshes custom emoji in store and emits SidebarChanged.
    let emoji_ev: SlackEvent =
        serde_json::from_str(r#"{"type":"emoji_changed","subtype":"add","name":"shipit"}"#)
            .unwrap();
    core.on_rtm(RtmEvent::Event(emoji_ev)).await;
    assert_eq!(next(&mut rx).await, Event::SidebarChanged);
    let emoji = store
        .lock()
        .unwrap()
        .custom_emoji(&TeamId::from("T1"))
        .unwrap();
    assert!(emoji.iter().any(|(name, _)| name == "shipit"));

    // Unknown: saved_added, commands_changed, user_status_changed, view_opened/updated/closed.
    for ev_json in [
        r#"{"type":"saved_added"}"#,
        r#"{"type":"commands_changed"}"#,
        r#"{"type":"user_status_changed","user":{"id":"U5","name":"eve"}}"#,
        r#"{"type":"view_opened","view":{"id":"V1","type":"modal"}}"#,
        r#"{"type":"view_updated","view":{"id":"V2","type":"home"}}"#,
        r#"{"type":"view_closed","view":{"id":"V1"}}"#,
    ] {
        let ev: SlackEvent = serde_json::from_str(ev_json).unwrap();
        core.on_rtm(RtmEvent::Event(ev)).await;
    }
    let mut saw_saved = false;
    let mut saw_cmds = false;
    let mut saw_user = false;
    let mut saw_modal_open = false;
    let mut saw_home = false;
    let mut saw_modal_close = false;
    for _ in 0..6 {
        match next(&mut rx).await {
            Event::SavedLoaded(Ok(_)) => saw_saved = true,
            Event::AppCommands(cmds) if cmds.len() == 1 && cmds[0].name == "deploy" => {
                saw_cmds = true;
            }
            Event::UsersChanged(ids) if ids == vec![UserId::from("U5")] => saw_user = true,
            Event::ModalOpened(v) if v["id"] == "V1" => saw_modal_open = true,
            Event::AppHomeUpdated(v) if v["id"] == "V2" => saw_home = true,
            Event::ModalClosed(id) if id == "V1" => saw_modal_close = true,
            other => panic!("unexpected event: {other:?}"),
        }
    }
    assert!(saw_saved && saw_cmds && saw_user && saw_modal_open && saw_home && saw_modal_close);

    // posted_message subtypes: allowed subtypes emit MessagePosted, channel_join does NOT.
    for (ts, subtype, expect_posted) in [
        ("10.0", "bot_message", true),
        ("11.0", "me_message", true),
        ("12.0", "file_share", true),
        ("13.0", "thread_broadcast", true),
        ("14.0", "channel_join", false),
    ] {
        let ev: SlackEvent = serde_json::from_str(&format!(
            r#"{{"type":"message","subtype":"{subtype}","channel":"C1","ts":"{ts}","text":"hi"}}"#
        ))
        .unwrap();
        assert_eq!(
            rtm::posted_message(&ev).is_some(),
            expect_posted,
            "{subtype}"
        );
    }
}

#[tokio::test]
async fn hydrate_partial_failures_self_user_dnd_and_activity_errors() {
    let fake = FakeBackend::default();
    {
        let mut state = fake.state.lock().unwrap();
        state.fail_dms = true;
        state.fail_emoji = true;
        state.user_boot_override = Some(UserBoot {
            channels: vec![Conversation {
                id: c1(),
                ..Default::default()
            }],
            self_user: Some(User {
                id: UserId::from("UME"),
                name: "me".into(),
                ..Default::default()
            }),
            ..Default::default()
        });
        state.dnd.insert(
            UserId::from("UME"),
            DndStatus {
                snooze_enabled: true,
                snooze_endtime: Some(9_999_999_999),
                ..Default::default()
            },
        );
        state.fail_activity_feed = true;
    }
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.hydrate().await;
    let mut errors = Vec::new();
    let mut saw_self_user = false;
    let mut saw_self_dnd = false;
    let mut saw_activity_err = false;
    let mut saw_sidebar = false;
    while !saw_self_dnd || !saw_activity_err || !saw_sidebar || errors.len() < 3 {
        match next(&mut rx).await {
            Event::Error(e) => errors.push(e),
            Event::UsersChanged(ids) if ids.contains(&UserId::from("UME")) => {
                saw_self_user = true;
            }
            Event::SelfDndUpdated(s) if s.snooze_enabled => saw_self_dnd = true,
            Event::ActivityView { feed: Err(_) } => saw_activity_err = true,
            Event::SidebarChanged => saw_sidebar = true,
            _ => {}
        }
    }
    assert!(saw_self_user);
    assert!(errors.iter().any(|e| e.contains("client.dms")));
    assert!(errors.iter().any(|e| e.contains("emoji.list")));
    assert!(errors.iter().any(|e| e.contains("activity.feed")));
    assert_eq!(
        *core.self_user_id.lock().unwrap(),
        Some(UserId::from("UME"))
    );

    // hydrate_activity when activity_feed succeeds but messages_list fails: reports error and still emits feed.
    {
        let mut state = fake.state.lock().unwrap();
        state.fail_activity_feed = false;
        state.fail_messages_list = true;
        state.activity = api::ActivityFeed {
            items: vec![api::ActivityItem {
                kind: api::ActivityKind::Mention,
                unread: true,
                feed_ts: Ts::from("5.0"),
                key: "at_user-C1-5.0".into(),
                channel: c1(),
                ts: Ts::from("5.0"),
                ..Default::default()
            }],
            ..Default::default()
        };
    }
    core.handle(Command::Messaging(MessagingCommand::FetchActivity {
        with_bodies: true,
    }))
    .await;
    assert!(matches!(next(&mut rx).await, Event::Error(e) if e.contains("messages.list")));
    let Event::ActivityView { feed: Ok(feed) } = next(&mut rx).await else {
        panic!("expected ActivityView(Ok)");
    };
    assert!(feed.items[0].message.is_none());
}

#[tokio::test(start_paused = true)]
async fn command_groups_slash_block_mark_all_close_and_channel_admin() {
    let fake = FakeBackend::with_history(1..=20);
    {
        let mut state = fake.state.lock().unwrap();
        state.run_command_error = Some("unknown_command".into());
        state.mark_all_read_ok = true;
        state.members = vec![UserId::from("U1"), UserId::from("U2")];
    }
    let (store, events, mut rx) = fake_core_parts();
    store
        .lock()
        .unwrap()
        .upsert_conversation(
            &TeamId::from("T1"),
            &Conversation {
                id: c1(),
                name: Some("general".into()),
                is_channel: true,
                is_member: true,
                ..Default::default()
            },
        )
        .unwrap();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    // RunCommand -> Slack error becomes CommandFailed.
    core.handle(Command::Messaging(MessagingCommand::RunCommand {
        channel: c1(),
        command: "/nope".into(),
        args: "".into(),
        thread_ts: None,
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::CommandFailed {
            command: "/nope".into(),
            error: "unknown_command".into(),
        }
    );

    // BlockAction with team: None defaults to workspace TeamId("T1").
    core.handle(Command::Collab(CollabCommand::BlockAction {
        bot: BotId::from("B1"),
        app_id: "A1".into(),
        team: None,
        action: serde_json::json!({"action_id": "btn"}),
        container: api::ActionContainer {
            channel: c1(),
            message_ts: Ts::from("1.0"),
            attachment_id: None,
            is_ephemeral: false,
            view_id: None,
        },
    }))
    .await;
    // MarkAllRead calls mark_all_read + emits Counts.
    core.handle(Command::Messaging(MessagingCommand::MarkAllRead))
        .await;
    assert!(matches!(next(&mut rx).await, Event::Counts(_)));
    assert_eq!(
        fake.state.lock().unwrap().block_action_calls,
        vec![(BotId::from("B1"), "A1".into(), TeamId::from("T1"))]
    );
    assert_eq!(fake.state.lock().unwrap().mark_all_read_calls, 1);

    // CloseConversation aborts active backfill and allows re-opening later.
    core.open(c1());
    assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
    core.handle(Command::Channel(ChannelCommand::CloseConversation(c1())))
        .await;
    assert!(!core.backfills.contains_key(&c1()));
    core.open(c1());
    assert!(core.backfills.contains_key(&c1()));
    core.handle(Command::Channel(ChannelCommand::CloseConversation(c1())))
        .await;

    // CreateChannel (with purpose), RenameChannel, Archive/Unarchive, Delete, ConvertPrivate, Invite.
    core.handle(Command::Channel(ChannelCommand::CreateChannel {
        name: "launch".into(),
        is_private: true,
        purpose: Some("Launch coordination".into()),
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::Conversations(vec![ChannelId::from("CNEW1")])
    );
    assert_eq!(
        next(&mut rx).await,
        Event::ConversationOpened(ChannelId::from("CNEW1"))
    );

    core.handle(Command::Channel(ChannelCommand::RenameChannel {
        channel: c1(),
        name: "general-v2".into(),
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));

    core.handle(Command::Channel(ChannelCommand::ArchiveChannel(c1())))
        .await;
    assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
    assert!(
        store
            .lock()
            .unwrap()
            .conversation(&TeamId::from("T1"), &c1())
            .unwrap()
            .unwrap()
            .is_archived
    );

    core.handle(Command::Channel(ChannelCommand::UnarchiveChannel(c1())))
        .await;
    assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
    assert!(
        !store
            .lock()
            .unwrap()
            .conversation(&TeamId::from("T1"), &c1())
            .unwrap()
            .unwrap()
            .is_archived
    );

    core.handle(Command::Channel(ChannelCommand::DeleteChannel(c1())))
        .await;
    assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
    let deleted = store
        .lock()
        .unwrap()
        .conversation(&TeamId::from("T1"), &c1())
        .unwrap()
        .unwrap();
    assert!(deleted.is_archived && !deleted.is_member);

    core.handle(Command::Channel(ChannelCommand::ConvertChannelToPrivate(
        c1(),
    )))
    .await;
    assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
    assert_eq!(next(&mut rx).await, Event::SidebarChanged);

    core.handle(Command::Channel(ChannelCommand::InviteToChannel {
        channel: c1(),
        users: vec![UserId::from("U2")],
    }))
    .await;
    assert!(
        matches!(next(&mut rx).await, Event::ConversationInfo { channel, .. } if channel == c1())
    );
    assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));
}

#[tokio::test]
async fn command_groups_profile_gifs_quick_switch_canvas_lists_files_extras_and_drop_shutdown() {
    let fake = FakeBackend::default();
    {
        let mut state = fake.state.lock().unwrap();
        state.profile_update_response = Some(crate::types::Profile {
            display_name: Some("Updated Me".into()),
            ..Default::default()
        });
        state.user_profile_response = Some(crate::types::Profile {
            pronouns: Some("   ".into()),
            fields: Some(serde_json::json!({
                "Xf1": { "label": "Inline Label", "value": "  Val1  " },
                "Xf2": { "value": "Val2" },
                "Xf3": { "value": "Val3" },
                "XfEmpty": { "label": "Skip", "value": "   " }
            })),
            ..Default::default()
        });
        state.team_profile_fields_response =
            HashMap::from([("Xf2".to_owned(), "Team Label".to_owned())]);
        state.list_data = api::SlackListData {
            file_id: "FLIST1".into(),
            title: "Tasks".into(),
            ..Default::default()
        };
        state.files_favorites = vec!["F1".into()];
        state.files_collections = vec![api::FileCollection {
            id: "COL1".into(),
            name: "Specs".into(),
            emoji: "book".into(),
            file_ids: vec!["F1".into()],
        }];
        state.files_deleted = vec![api::DeletedFile {
            id: "FDEL1".into(),
            title: "Old Canvas".into(),
            date_delete: 123,
            file_type: "quip".into(),
        }];
    }
    let (store, events, mut rx) = fake_core_parts();
    store
        .lock()
        .unwrap()
        .upsert_user(
            &TeamId::from("T1"),
            &User {
                id: UserId::from("UME"),
                name: "me".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;
    *core.self_user_id.lock().unwrap() = Some(UserId::from("UME"));

    // UpdateProfile updates stored self user & emits UsersChanged.
    core.handle(Command::Workspace(WorkspaceCommand::UpdateProfile {
        update: api::ProfileUpdate {
            display_name: Some("Updated Me".into()),
            ..Default::default()
        },
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::UsersChanged(vec![UserId::from("UME")])
    );
    let me = store
        .lock()
        .unwrap()
        .users(&TeamId::from("T1"))
        .unwrap()
        .into_iter()
        .find(|u| u.id == UserId::from("UME"))
        .unwrap();
    assert_eq!(me.profile.display_name.as_deref(), Some("Updated Me"));

    // FetchUserProfile: inline label > team label > field id; blank value skipped; blank pronouns -> None.
    core.handle(Command::Workspace(WorkspaceCommand::FetchUserProfile(
        UserId::from("U2"),
    )))
    .await;
    let Event::UserProfileLoaded {
        user,
        pronouns,
        mut fields,
    } = next(&mut rx).await
    else {
        panic!("expected UserProfileLoaded");
    };
    assert_eq!(user, UserId::from("U2"));
    assert_eq!(pronouns, None);
    fields.sort();
    assert_eq!(
        fields,
        vec![
            ("Inline Label".into(), "Val1".into()),
            ("Team Label".into(), "Val2".into()),
            ("Xf3".into(), "Val3".into()),
        ]
    );

    // SearchGifs: blank -> gif_featured, non-blank -> gif_search.
    core.handle(Command::Collab(CollabCommand::SearchGifs {
        query: "   ".into(),
    }))
    .await;
    assert!(matches!(next(&mut rx).await, Event::GifResults { gifs, .. } if gifs[0].id == "g1"));
    core.handle(Command::Collab(CollabCommand::SearchGifs {
        query: "cats".into(),
    }))
    .await;
    assert!(matches!(next(&mut rx).await, Event::GifResults { gifs, .. } if gifs[0].id == "g2"));
    assert_eq!(fake.state.lock().unwrap().gif_featured_calls, 1);
    assert_eq!(
        fake.state.lock().unwrap().gif_search_calls,
        vec!["cats".to_owned()]
    );

    // QuickSwitchSearch: blank is no-op, non-blank calls edge_search.
    core.handle(Command::Collab(CollabCommand::QuickSwitchSearch {
        query: "  ".into(),
    }))
    .await;
    core.handle(Command::Collab(CollabCommand::QuickSwitchSearch {
        query: "bob".into(),
    }))
    .await;
    assert!(matches!(
        next(&mut rx).await,
        Event::QuickSwitchResults { query, results } if query == "bob" && results.users.len() == 1
    ));
    assert_eq!(
        fake.state.lock().unwrap().edge_search_calls,
        vec![(TeamId::from("T1"), "bob".to_owned())]
    );

    // LoadCanvas (ok + err), CreateCanvas, EditCanvas.
    core.handle(Command::Collab(CollabCommand::LoadCanvas {
        file_id: "FC1".into(),
    }))
    .await;
    assert!(matches!(
        next(&mut rx).await,
        Event::CanvasLoaded { file_id, html: Ok(_), .. } if file_id == "FC1"
    ));
    assert!(matches!(
        next(&mut rx).await,
        Event::CanvasMetaLoaded { file_id, meta }
            if file_id == "FC1" && meta.comment_channel_id == ChannelId::from("CC1")
    ));
    fake.state.lock().unwrap().fail_canvas_html = true;
    core.handle(Command::Collab(CollabCommand::LoadCanvas {
        file_id: "FC_BAD".into(),
    }))
    .await;
    assert!(matches!(
        next(&mut rx).await,
        Event::CanvasLoaded { file_id, title, html: Err(_) } if file_id == "FC_BAD" && title == "Canvas"
    ));
    fake.state.lock().unwrap().fail_canvas_html = false;

    core.handle(Command::Collab(CollabCommand::CreateCanvas {
        title: "Doc".into(),
        markdown: "# Hi".into(),
        channel: Some(c1()),
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::CanvasCreated {
            file_id: "FCANVAS1".into(),
            title: "Doc".into(),
            channel: Some(c1()),
        }
    );

    core.handle(Command::Collab(CollabCommand::EditCanvas {
        file_id: "FCANVAS1".into(),
        op: api::CanvasEditOp::InsertAtEnd {
            markdown: "More".into(),
        },
    }))
    .await;
    assert!(matches!(
        next(&mut rx).await,
        Event::CanvasLoaded { file_id, html: Ok(_), .. } if file_id == "FCANVAS1"
    ));
    assert!(matches!(
        next(&mut rx).await,
        Event::CanvasMetaLoaded { file_id, .. } if file_id == "FCANVAS1"
    ));

    // LoadList & CreateListItem.
    core.handle(Command::Collab(CollabCommand::LoadList {
        file_id: "FLIST1".into(),
    }))
    .await;
    assert!(matches!(
        next(&mut rx).await,
        Event::ListLoaded { file_id, list: Ok(ref l) } if file_id == "FLIST1" && l.title == "Tasks"
    ));
    assert!(matches!(
        next(&mut rx).await,
        Event::ListMetaLoaded { file_id, .. } if file_id == "FLIST1"
    ));
    core.handle(Command::Collab(CollabCommand::CreateListItem {
        list_id: "FLIST1".into(),
        initial_fields: vec![],
    }))
    .await;
    assert!(matches!(
        next(&mut rx).await,
        Event::ListLoaded { file_id, list: Ok(_) } if file_id == "FLIST1"
    ));
    assert!(matches!(
        next(&mut rx).await,
        Event::ListMetaLoaded { file_id, .. } if file_id == "FLIST1"
    ));

    // LoadFilesExtras.
    core.handle(Command::Collab(CollabCommand::LoadFilesExtras))
        .await;
    assert!(matches!(
        next(&mut rx).await,
        Event::FilesExtrasLoaded { favorites, collections, deleted }
            if favorites == vec!["F1".to_owned()] && collections.len() == 1 && deleted.len() == 1
    ));

    // Dropping the command sender shuts down `run` loop cleanly.
    let (cmd_tx, cmd_rx) = mpsc::unbounded();
    let run_task = tokio::spawn(run(core, cmd_rx, None));
    drop(cmd_tx);
    tokio::time::timeout(Duration::from_secs(2), run_task)
        .await
        .expect("run loop must exit when command sender drops")
        .unwrap();
}

#[tokio::test]
async fn huddle_watcher_ignores_stale_generation_idle_and_preserves_new_session() {
    let (_store, events, mut rx) = fake_core_parts();
    let huddle = Arc::new(Mutex::new(HuddleSlot {
        generation: Arc::new(AtomicU64::new(2)),
        joining: true,
        handle: None,
        call_id: Some("R_NEW".into()),
    }));

    // Old watcher (gen 1) emits Idle after gen 2 has started: must NOT clobber call_id or emit Idle.
    let (tx1, rx1) = tokio::sync::mpsc::unbounded_channel();
    commands::spawn_huddle_watcher(
        Arc::clone(&huddle),
        1,
        rx1,
        events.clone(),
        test_media_tx(),
        None,
    );
    tx1.send(crate::huddle::HuddleUpdate::State(Box::new(
        crate::huddle::HuddleSessionSnapshot {
            phase: crate::huddle::HuddlePhase::Idle,
            ..Default::default()
        },
    )))
    .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        huddle.lock().unwrap().call_id.as_deref(),
        Some("R_NEW"),
        "stale watcher must never clobber newer session call_id"
    );
    assert!(
        rx.try_recv().is_err(),
        "stale watcher must not emit Idle over active gen"
    );

    // Current watcher (gen 2) emits Idle: clears call_id and emits HuddleSessionChanged(Idle).
    let (tx2, rx2) = tokio::sync::mpsc::unbounded_channel();
    commands::spawn_huddle_watcher(Arc::clone(&huddle), 2, rx2, events, test_media_tx(), None);
    tx2.send(crate::huddle::HuddleUpdate::State(Box::new(
        crate::huddle::HuddleSessionSnapshot {
            phase: crate::huddle::HuddlePhase::Idle,
            ..Default::default()
        },
    )))
    .unwrap();
    assert!(matches!(
        next(&mut rx).await,
        Event::Huddle(HuddleEvent::SessionChanged(s)) if s.phase == crate::huddle::HuddlePhase::Idle
    ));
    assert!(huddle.lock().unwrap().call_id.is_none());
}

#[tokio::test]
async fn canvas_delete_access_and_comments_command_event_round_trips() {
    let fake = FakeBackend::default();
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        store,
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    // 1. DeleteCanvas (ok + formatted Slack error)
    core.handle(Command::Collab(CollabCommand::DeleteCanvas {
        canvas_id: "F0DEL".into(),
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::CanvasDeleted {
            canvas_id: "F0DEL".into(),
            result: Ok(()),
        }
    );
    assert_eq!(
        fake.state.lock().unwrap().canvases_delete_calls,
        vec!["F0DEL"]
    );

    fake.state.lock().unwrap().canvases_delete_error = Some("cant_delete_file".into());
    core.handle(Command::Collab(CollabCommand::DeleteCanvas {
        canvas_id: "F0LOCKED".into(),
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::CanvasDeleted {
            canvas_id: "F0LOCKED".into(),
            result: Err(api::format_canvas_error("cant_delete_file")),
        }
    );
    fake.state.lock().unwrap().canvases_delete_error = None;

    // 2. SetCanvasAccess with post_permalink shares permalink into each channel
    core.handle(Command::Collab(CollabCommand::SetCanvasAccess {
        canvas_id: "F0SHARE".into(),
        access_level: api::CanvasAccessLevel::Write,
        channel_ids: vec![ChannelId::from("C1"), ChannelId::from("C2")],
        user_ids: vec![],
        post_permalink: Some("https://acme.slack.com/docs/T1/F0SHARE".into()),
    }))
    .await;
    assert!(matches!(
        next(&mut rx).await,
        Event::CanvasAccessUpdated { canvas_id, result: Ok(()), .. } if canvas_id == "F0SHARE"
    ));
    {
        let st = fake.state.lock().unwrap();
        assert_eq!(st.canvases_access_set_calls.len(), 1);
        assert_eq!(
            st.posted_messages,
            vec![
                (
                    ChannelId::from("C1"),
                    "https://acme.slack.com/docs/T1/F0SHARE".to_owned()
                ),
                (
                    ChannelId::from("C2"),
                    "https://acme.slack.com/docs/T1/F0SHARE".to_owned()
                ),
            ]
        );
    }

    // 3. DeleteCanvasAccess (ok + formatted error)
    core.handle(Command::Collab(CollabCommand::DeleteCanvasAccess {
        canvas_id: "F0SHARE".into(),
        channel_ids: vec![],
        user_ids: vec![UserId::from("U7")],
    }))
    .await;
    assert!(matches!(
        next(&mut rx).await,
        Event::CanvasAccessUpdated { canvas_id, result: Ok(()), .. } if canvas_id == "F0SHARE"
    ));
    assert_eq!(
        fake.state
            .lock()
            .unwrap()
            .canvases_access_delete_calls
            .len(),
        1
    );

    fake.state.lock().unwrap().canvases_access_error = Some("restricted_action".into());
    core.handle(Command::Collab(CollabCommand::SetCanvasAccess {
        canvas_id: "F0SHARE".into(),
        access_level: api::CanvasAccessLevel::Owner,
        channel_ids: vec![],
        user_ids: vec![UserId::from("U8")],
        post_permalink: None,
    }))
    .await;
    assert!(matches!(
        next(&mut rx).await,
        Event::CanvasAccessUpdated { canvas_id, result: Err(ref msg), .. }
            if canvas_id == "F0SHARE" && msg == &api::format_canvas_error("restricted_action")
    ));
    fake.state.lock().unwrap().canvases_access_error = None;

    // 4. LoadCanvasComments & PostCanvasComment
    let sample_thread = api::CanvasCommentThread {
        root_ts: Ts::from("1700.1"),
        author: Some(UserId::from("U1")),
        text: "Check numbers".into(),
        reply_count: 1,
        replies: vec![api::CanvasCommentReply {
            ts: Ts::from("1700.2"),
            author: Some(UserId::from("U2")),
            text: "Verified".into(),
        }],
        resolved: false,
    };
    fake.state.lock().unwrap().canvas_comments = vec![sample_thread.clone()];
    core.handle(Command::Collab(CollabCommand::LoadCanvasComments {
        canvas_id: "F0CMT".into(),
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::CanvasCommentsLoaded {
            canvas_id: "F0CMT".into(),
            result: Ok(vec![sample_thread]),
        }
    );

    core.handle(Command::Collab(CollabCommand::PostCanvasComment {
        canvas_id: "F0CMT".into(),
        text: "Thanks!".into(),
        thread_ts: Some(Ts::from("1700.1")),
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::CanvasCommentPosted {
            canvas_id: "F0CMT".into(),
            result: Ok(()),
        }
    );
    assert_eq!(
        fake.state.lock().unwrap().canvas_comment_posts,
        vec![(
            "F0CMT".to_owned(),
            "Thanks!".to_owned(),
            Some(Ts::from("1700.1"))
        )]
    );

    // 5. ResolveCanvasComment (resolve + reopen) & ResolveAllCanvasComments
    core.handle(Command::Collab(CollabCommand::ResolveCanvasComment {
        canvas_id: "F0CMT".into(),
        thread_ts: Ts::from("1700.1"),
        resolved: true,
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::CanvasCommentResolved {
            canvas_id: "F0CMT".into(),
            thread_ts: Ts::from("1700.1"),
            resolved: true,
            result: Ok(()),
        }
    );

    core.handle(Command::Collab(CollabCommand::ResolveCanvasComment {
        canvas_id: "F0CMT".into(),
        thread_ts: Ts::from("1700.1"),
        resolved: false,
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::CanvasCommentResolved {
            canvas_id: "F0CMT".into(),
            thread_ts: Ts::from("1700.1"),
            resolved: false,
            result: Ok(()),
        }
    );

    core.handle(Command::Collab(CollabCommand::ResolveAllCanvasComments {
        canvas_id: "F0CMT".into(),
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::CanvasCommentResolved {
            canvas_id: "F0CMT".into(),
            thread_ts: Ts::default(),
            resolved: true,
            result: Ok(()),
        }
    );

    {
        let st = fake.state.lock().unwrap();
        assert_eq!(
            st.quip_archive_calls,
            vec![
                (ChannelId::from("C0CMT"), Ts::from("1700.1"), true),
                (ChannelId::from("C0CMT"), Ts::from("1700.1"), false),
            ]
        );
        assert_eq!(st.quip_archive_all_calls, vec![ChannelId::from("C0CMT")]);
    }
}

#[tokio::test]
async fn stealth_mode_suppresses_read_receipts_and_typing_until_disabled() {
    let fake = FakeBackend::default();
    fake.state.lock().unwrap().mark_read_ok = true;
    fake.state.lock().unwrap().mark_all_read_ok = true;
    let (store, events, _rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        store,
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Workspace(WorkspaceCommand::SetStealthMode(true)))
        .await;
    core.handle(Command::Messaging(MessagingCommand::MarkRead {
        channel: ChannelId::from("C1"),
        ts: Ts::from("1700.1"),
    }))
    .await;
    core.handle(Command::Messaging(MessagingCommand::MarkThreadRead {
        channel: ChannelId::from("C1"),
        thread_ts: Ts::from("1700.1"),
        ts: Ts::from("1700.2"),
    }))
    .await;
    core.handle(Command::Messaging(MessagingCommand::MarkAllRead))
        .await;
    core.handle(Command::Messaging(MessagingCommand::Typing {
        channel: ChannelId::from("C1"),
        thread_ts: None,
    }))
    .await;
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    {
        let st = fake.state.lock().unwrap();
        assert!(
            st.mark_calls.is_empty(),
            "stealth mode must suppress mark_read"
        );
        assert!(
            st.thread_mark_calls.is_empty(),
            "stealth mode must suppress mark_thread_read"
        );
        assert_eq!(
            st.mark_all_read_calls, 0,
            "stealth mode must suppress mark_all_read"
        );
    }

    // Disabling stealth mode resumes read receipts cleanly.
    core.handle(Command::Workspace(WorkspaceCommand::SetStealthMode(false)))
        .await;
    core.handle(Command::Messaging(MessagingCommand::MarkRead {
        channel: ChannelId::from("C1"),
        ts: Ts::from("1700.1"),
    }))
    .await;
    core.handle(Command::Messaging(MessagingCommand::MarkAllRead))
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    {
        let st = fake.state.lock().unwrap();
        assert_eq!(st.mark_calls.len(), 1);
        assert_eq!(st.mark_all_read_calls, 1);
    }
}

#[tokio::test]
async fn huddle_watcher_persists_finalized_multilingual_captions_and_skips_partials_and_dups() {
    let (store, events, mut rx) = fake_core_parts();
    let huddle = Arc::new(Mutex::new(HuddleSlot {
        generation: Arc::new(AtomicU64::new(1)),
        joining: false,
        call_id: Some("R0BILINGUAL".into()),
        handle: None,
    }));
    let enabled = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let persist = commands::huddle::TranscriptPersistCtx {
        store: Arc::clone(&store),
        workspace: TeamId::from("T1"),
        channel: ChannelId::from("C1"),
        call_id: "R0BILINGUAL".into(),
        thread_root_ts: Some(Ts::from("1700.1")),
        source: "chime",
        enabled: Arc::clone(&enabled),
    };
    let (tx, upd_rx) = tokio::sync::mpsc::unbounded_channel();
    commands::spawn_huddle_watcher(
        Arc::clone(&huddle),
        1,
        upd_rx,
        events,
        test_media_tx(),
        Some(persist),
    );

    // 1. Partial caption -> emitted to UI, NOT persisted to SQLite.
    tx.send(crate::huddle::HuddleUpdate::DataEvent(
        crate::huddle::HuddleDataEvent::Caption {
            user: UserId::from("U1"),
            text: "Vi ska...".into(),
            is_partial: true,
            at_ms: 1_700_000_001_000,
            lang: Some("sv".into()),
        },
    ))
    .unwrap();
    // 2. Final Swedish caption -> persisted.
    tx.send(crate::huddle::HuddleUpdate::DataEvent(
        crate::huddle::HuddleDataEvent::Caption {
            user: UserId::from("U1"),
            text: "Vi ska lansera idag".into(),
            is_partial: false,
            at_ms: 1_700_000_002_000,
            lang: Some("sv".into()),
        },
    ))
    .unwrap();
    // 3. Duplicate within 2.5s -> skipped.
    tx.send(crate::huddle::HuddleUpdate::DataEvent(
        crate::huddle::HuddleDataEvent::Caption {
            user: UserId::from("U1"),
            text: "Vi ska lansera idag".into(),
            is_partial: false,
            at_ms: 1_700_000_003_000,
            lang: Some("sv".into()),
        },
    ))
    .unwrap();
    // 4. Final English code-switched caption -> persisted.
    tx.send(crate::huddle::HuddleUpdate::DataEvent(
        crate::huddle::HuddleDataEvent::Caption {
            user: UserId::from("U2"),
            text: "Let's ship it after the standup".into(),
            is_partial: false,
            at_ms: 1_700_000_005_000,
            lang: Some("en".into()),
        },
    ))
    .unwrap();

    for _ in 0..4 {
        let _ = next(&mut rx).await;
    }

    let stored = store
        .lock()
        .unwrap()
        .huddle_transcript_for_call(&TeamId::from("T1"), "R0BILINGUAL")
        .unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].text, "Vi ska lansera idag");
    assert_eq!(stored[0].lang.as_deref(), Some("sv"));
    assert_eq!(stored[1].text, "Let's ship it after the standup");
    assert_eq!(stored[1].lang.as_deref(), Some("en"));
}

#[tokio::test]
async fn stealth_mode_blocks_mark_read_in_offline_queue_and_during_flush() {
    let fake = FakeBackend::default();
    fake.state.lock().expect("lock").mark_read_ok = true;
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    // Case 1: Stealth mode enabled BEFORE going offline -> MarkRead is never queued.
    core.handle(Command::Workspace(WorkspaceCommand::SetStealthMode(true)))
        .await;
    core.on_rtm(RtmEvent::State(ConnectionState::Disconnected {
        reason: "offline".into(),
    }))
    .await;
    assert!(matches!(next(&mut rx).await, Event::Connection(_)));

    core.handle(Command::Messaging(MessagingCommand::MarkRead {
        channel: c1(),
        ts: Ts::from("10.0"),
    }))
    .await;
    assert!(
        core.offline_queue.is_empty(),
        "stealth mode must prevent MarkRead from entering offline queue"
    );
    assert!(
        store
            .lock()
            .expect("lock")
            .list_offline(&TeamId::from("T1"))
            .expect("list")
            .is_empty()
    );

    // Case 2: MarkRead queued while stealth mode is OFF, then stealth mode is turned ON before
    // reconnect -> flushing must NOT call backend.mark_read!
    core.handle(Command::Workspace(WorkspaceCommand::SetStealthMode(false)))
        .await;
    core.handle(Command::Messaging(MessagingCommand::MarkRead {
        channel: c1(),
        ts: Ts::from("20.0"),
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::OfflineQueueChanged(1));

    core.handle(Command::Workspace(WorkspaceCommand::SetStealthMode(true)))
        .await;
    core.on_rtm(RtmEvent::State(ConnectionState::Connected {
        fast_reconnect: false,
    }))
    .await;
    assert_eq!(next(&mut rx).await, Event::OfflineQueueChanged(0));
    assert!(matches!(next(&mut rx).await, Event::Connection(_)));
    wait_until("suppressed queued MarkRead row deleted from SQLite", || {
        store
            .lock()
            .expect("lock")
            .list_offline(&TeamId::from("T1"))
            .expect("list")
            .is_empty()
    })
    .await;
    assert!(
        fake.state.lock().expect("lock").mark_calls.is_empty(),
        "stealth mode must suppress queued MarkRead during reconnect flush"
    );
}

#[tokio::test]
async fn refresh_conversation_merges_without_clobbering_existing_metadata() {
    let fake = FakeBackend::default();
    fake.set_info(Conversation {
        id: c1(),
        is_archived: true,
        ..Default::default()
    });
    let (store, events, mut rx) = fake_core_parts();
    store
        .lock()
        .expect("lock")
        .upsert_conversation(
            &TeamId::from("T1"),
            &Conversation {
                id: c1(),
                name: Some("general".into()),
                is_channel: true,
                is_member: true,
                topic: Some(crate::types::TopicPurpose {
                    value: "Company news".into(),
                    ..Default::default()
                }),
                last_read: Some(Ts::from("42.0")),
                ..Default::default()
            },
        )
        .expect("seed");

    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;
    let ev: SlackEvent =
        serde_json::from_str(r#"{"type":"channel_archive","channel":"C1"}"#).expect("event");
    core.on_rtm(RtmEvent::Event(ev)).await;
    assert_eq!(next(&mut rx).await, Event::Conversations(vec![c1()]));

    let stored = store
        .lock()
        .expect("lock")
        .conversation(&TeamId::from("T1"), &c1())
        .expect("query")
        .expect("row");
    assert!(stored.is_archived, "fresh archived flag applied");
    assert_eq!(stored.name.as_deref(), Some("general"));
    assert!(stored.is_member, "existing membership preserved");
    assert_eq!(
        stored.topic.as_ref().map(|t| t.value.as_str()),
        Some("Company news")
    );
    assert_eq!(stored.last_read, Some(Ts::from("42.0")));
}

#[tokio::test]
async fn rtm_bot_info_resolution_does_not_block_event_loop() {
    let fake = FakeBackend::default();
    let gate = Arc::new(tokio::sync::Notify::new());
    {
        let mut st = fake.state.lock().expect("lock");
        st.bot_info_gate = Some(Arc::clone(&gate));
        st.bots.insert(
            BotId::from("BSLOW"),
            BotProfile {
                id: BotId::from("BSLOW"),
                name: "SlowBot".into(),
                ..Default::default()
            },
        );
    }
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    let ev: SlackEvent = serde_json::from_str(
        r#"{"type":"message","channel":"C1","ts":"77.0","text":"hello","bot_id":"BSLOW"}"#,
    )
    .expect("event");
    // Must complete immediately without waiting for `gate`!
    tokio::time::timeout(Duration::from_millis(250), core.on_rtm(RtmEvent::Event(ev)))
        .await
        .expect("on_rtm must not block waiting on bots.info");
    assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
    assert!(matches!(next(&mut rx).await, Event::MessagePosted { .. }));

    // Release the slow bots.info call: profile lands asynchronously.
    gate.notify_one();
    assert_eq!(next(&mut rx).await, Event::StoreChanged { channel: c1() });
    assert!(matches!(
        next(&mut rx).await,
        Event::BotProfileLoaded { bot_id, profile }
            if bot_id == BotId::from("BSLOW") && profile.name == "SlowBot"
    ));
}

#[tokio::test]
async fn shutdown_aborts_tracked_background_tasks() {
    let fake = FakeBackend::default();
    let gate = Arc::new(tokio::sync::Notify::new());
    fake.state.lock().expect("lock").bot_info_gate = Some(Arc::clone(&gate));
    let (store, events, _rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        store,
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Workspace(WorkspaceCommand::ResolveBots(vec![
        BotId::from("BHANG"),
    ])))
    .await;
    tokio::task::yield_now().await;
    assert!(
        core.tasks.active_count() >= 1,
        "in-flight task must be tracked"
    );

    core.shutdown().await;
    assert_eq!(
        core.tasks.active_count(),
        0,
        "shutdown must abort all tracked tasks"
    );
}

#[tokio::test]
async fn collab_endpoints_propagate_auth_expired() {
    let fake = FakeBackend::default();
    fake.state.lock().expect("lock").auth_expired_collab = true;
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        store,
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    core.handle(Command::Collab(CollabCommand::SearchRemoteMessages {
        query: "hello".into(),
        sort: api::SearchSort::Score,
        page: 1,
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::AuthExpired("token_revoked".into())
    );
    assert!(matches!(
        next(&mut rx).await,
        Event::RemoteMessagesPage { result: Err(_), .. }
    ));

    core.handle(Command::Collab(CollabCommand::LoadAppHome(c1())))
        .await;
    assert_eq!(
        next(&mut rx).await,
        Event::AuthExpired("token_revoked".into())
    );
    assert!(matches!(
        next(&mut rx).await,
        Event::AppHomeLoaded { home: Err(_), .. }
    ));

    core.handle(Command::Collab(CollabCommand::LoadList {
        file_id: "FL1".into(),
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::AuthExpired("token_revoked".into())
    );
    assert!(matches!(
        next(&mut rx).await,
        Event::ListLoaded { list: Err(_), .. }
    ));

    core.handle(Command::Collab(CollabCommand::LoadFilesExtras))
        .await;
    assert_eq!(
        next(&mut rx).await,
        Event::AuthExpired("token_revoked".into())
    );

    core.handle(Command::Collab(CollabCommand::FetchListRecordComments {
        list_id: "FL1".into(),
        record_id: "Rec1".into(),
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::AuthExpired("token_revoked".into())
    );
}

#[tokio::test]
async fn huddle_media_uses_dedicated_bounded_channel_and_drops_overflow_without_blocking_events() {
    let huddle = Arc::new(Mutex::new(HuddleSlot::default()));
    huddle
        .lock()
        .unwrap()
        .generation
        .store(1, Ordering::Release);
    let (event_tx, mut event_rx) = mpsc::unbounded();
    let events = Emitter(event_tx);
    let (media_tx, mut media_rx) = mpsc::channel(HUDDLE_MEDIA_CHANNEL_CAPACITY);
    let (upd_tx, upd_rx) = tokio::sync::mpsc::unbounded_channel();

    commands::spawn_huddle_watcher(Arc::clone(&huddle), 1, upd_rx, events, media_tx, None);

    // Flood well past HUDDLE_MEDIA_CHANNEL_CAPACITY (64) without draining `media_rx`:
    // stale media frames must drop via `try_send` instead of stalling the watcher.
    let tile = crate::huddle::VideoTileKey::LocalCamera;
    for seq in 0..(HUDDLE_MEDIA_CHANNEL_CAPACITY * 2) {
        upd_tx
            .send(crate::huddle::HuddleUpdate::VideoFrame {
                tile: tile.clone(),
                frame: crate::huddle::VideoFrame {
                    width: 2,
                    height: 2,
                    data: crate::huddle::VideoFrameData::Bgra(Arc::new(vec![seq as u8; 16])),
                },
            })
            .unwrap();
        upd_tx
            .send(crate::huddle::HuddleUpdate::AudioLevels(
                crate::huddle::HuddleAudioLevels::default(),
            ))
            .unwrap();
    }
    upd_tx
        .send(crate::huddle::HuddleUpdate::VideoTileClosed(tile.clone()))
        .unwrap();
    // Follow immediately with a critical session state change on the ordered `Event` lane.
    upd_tx
        .send(crate::huddle::HuddleUpdate::State(Box::new(
            crate::huddle::HuddleSessionSnapshot {
                phase: crate::huddle::HuddlePhase::Idle,
                ..Default::default()
            },
        )))
        .unwrap();
    drop(upd_tx);

    // The ordered `Event` channel receives ONLY the session state change — zero media frames!
    let ev = next(&mut event_rx).await;
    assert!(matches!(
        ev,
        Event::Huddle(HuddleEvent::SessionChanged(s)) if s.phase == crate::huddle::HuddlePhase::Idle
    ));

    // Meanwhile `media_rx` buffered up to the bounded channel capacity (and dropped overflow).
    let mut received = 0usize;
    while media_rx.try_recv().is_ok() {
        received += 1;
    }
    assert!(
        received > 0,
        "expected buffered media frames on huddle_media channel"
    );
    assert!(
        received <= HUDDLE_MEDIA_CHANNEL_CAPACITY + 1,
        "media channel must stay bounded, got {received}"
    );
}

#[test]
fn command_domain_sub_enums_convert_into_command() {
    let _: Command = MessagingCommand::MarkAllRead.into();
    let _: Command = ChannelCommand::ArchiveChannel(c1()).into();
    let _: Command = CollabCommand::LoadFilesExtras.into();
    let _: Command = HuddleCommand::Leave.into();
    let _: Command = crate::HuddleControlCommand::SetMuted(true).into();
    let _: Command = WorkspaceCommand::SignOut.into();
}

#[tokio::test]
async fn usergroup_disable_enable_and_users_commands_update_state_and_emit_events() {
    use crate::types::{Usergroup, UsergroupId};

    let fake = FakeBackend::default();
    let ug_id = UsergroupId::from("S0614TZR7");
    {
        let mut state = fake.state.lock().expect("lock fake state");
        state.usergroups.push(Usergroup {
            id: ug_id.clone(),
            name: "Engineering".to_owned(),
            handle: "eng".to_owned(),
            user_count: Some(1),
            ..Default::default()
        });
        state
            .usergroup_users
            .insert(ug_id.clone(), vec![UserId::from("U01")]);
    }
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    // 1. LoadUsergroupUsers -> Event::UsergroupUsersLoaded
    core.handle(Command::Workspace(WorkspaceCommand::LoadUsergroupUsers(
        ug_id.clone(),
    )))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::UsergroupUsersLoaded {
            usergroup: ug_id.clone(),
            users: vec![UserId::from("U01")],
        }
    );

    // 2. UpdateUsergroupUsers -> Event::UsergroupUsersLoaded + refreshed Event::Usergroups
    let updated_members = vec![UserId::from("U01"), UserId::from("U02")];
    core.handle(Command::Workspace(WorkspaceCommand::UpdateUsergroupUsers {
        usergroup: ug_id.clone(),
        users: updated_members.clone(),
    }))
    .await;
    assert_eq!(
        next(&mut rx).await,
        Event::UsergroupUsersLoaded {
            usergroup: ug_id.clone(),
            users: updated_members.clone(),
        }
    );
    match next(&mut rx).await {
        Event::Usergroups(groups) => {
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].user_count, Some(2));
        }
        other => panic!("expected Event::Usergroups after member update, got {other:?}"),
    }

    // 3. DisableUsergroup -> refreshed Event::Usergroups without disabled group
    core.handle(Command::Workspace(WorkspaceCommand::DisableUsergroup(
        ug_id.clone(),
    )))
    .await;
    assert_eq!(next(&mut rx).await, Event::Usergroups(Vec::new()));

    // 4. EnableUsergroup -> refreshed Event::Usergroups with re-enabled group
    core.handle(Command::Workspace(WorkspaceCommand::EnableUsergroup(
        ug_id.clone(),
    )))
    .await;
    match next(&mut rx).await {
        Event::Usergroups(groups) => {
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].id, ug_id);
        }
        other => panic!("expected Event::Usergroups after enable, got {other:?}"),
    }

    let state = fake.state.lock().expect("lock fake state");
    assert_eq!(state.usergroup_disable_calls, vec![ug_id.clone()]);
    assert_eq!(state.usergroup_enable_calls, vec![ug_id.clone()]);
    assert_eq!(
        state.usergroup_users_update_calls,
        vec![(ug_id, updated_members)]
    );
}

#[tokio::test]
async fn lists_sharing_meta_delete_access_and_record_threads_flow_through_core() {
    let fake = FakeBackend::default();
    {
        let mut state = fake.state.lock().unwrap();
        state.list_data = api::SlackListData {
            file_id: "F0LIST".into(),
            title: "Launch Plan".into(),
            ..Default::default()
        };
        state.list_meta = Some(api::ListMeta {
            file_id: "F0LIST".into(),
            title: "Launch Plan".into(),
            permalink: Some("https://acme.slack.com/lists/T1/F0LIST".into()),
            editable: true,
            access: Some("owner".into()),
            org_or_workspace_access: Some("read".into()),
            editors: vec![UserId::from("U1")],
            shared_channels: vec![ChannelId::from("C1")],
            row_count: Some(5),
            row_count_limit: Some(5000),
            over_row_maximum: false,
            comment_channel_id: ChannelId::from("C0LIST"),
            ..Default::default()
        });
        state.list_record_threads = vec![
            Message {
                ts: Ts::from("1700.1"),
                user: Some(UserId::from("U1")),
                text: "List comment root".into(),
                ..Default::default()
            },
            Message {
                ts: Ts::from("1700.2"),
                thread_ts: Some(Ts::from("1700.1")),
                user: Some(UserId::from("U2")),
                text: "List comment reply".into(),
                ..Default::default()
            },
        ];
    }
    let (store, events, mut rx) = fake_core_parts();
    let mut core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;

    // 1. LoadList emits both ListLoaded and ListMetaLoaded.
    core.handle(Command::Collab(CollabCommand::LoadList {
        file_id: "F0LIST".into(),
    }))
    .await;
    match next(&mut rx).await {
        Event::ListLoaded { file_id, list } => {
            assert_eq!(file_id, "F0LIST");
            assert_eq!(list.unwrap().title, "Launch Plan");
        }
        other => panic!("expected Event::ListLoaded, got {other:?}"),
    }
    match next(&mut rx).await {
        Event::ListMetaLoaded { file_id, meta } => {
            assert_eq!(file_id, "F0LIST");
            assert_eq!(meta.access.as_deref(), Some("owner"));
            assert_eq!(meta.row_count, Some(5));
        }
        other => panic!("expected Event::ListMetaLoaded, got {other:?}"),
    }

    // 2. SetListAccess with mixed channel + user targets splits into two XOR API calls and posts permalink.
    core.handle(Command::Collab(CollabCommand::SetListAccess {
        list_id: "F0LIST".into(),
        access_level: api::ListAccessLevel::Write,
        channel_ids: vec![ChannelId::from("C1")],
        user_ids: vec![UserId::from("U2")],
        post_permalink: Some("https://acme.slack.com/lists/T1/F0LIST".into()),
    }))
    .await;
    match next(&mut rx).await {
        Event::ListAccessUpdated {
            list_id, result, ..
        } => {
            assert_eq!(list_id, "F0LIST");
            assert!(result.is_ok());
        }
        other => panic!("expected Event::ListAccessUpdated, got {other:?}"),
    }

    // 3. Silent SetListAccess does NOT post a permalink message to the channel.
    core.handle(Command::Collab(CollabCommand::SetListAccess {
        list_id: "F0LIST".into(),
        access_level: api::ListAccessLevel::Read,
        channel_ids: vec![ChannelId::from("C2")],
        user_ids: Vec::new(),
        post_permalink: None,
    }))
    .await;
    match next(&mut rx).await {
        Event::ListAccessUpdated { result, .. } => assert!(result.is_ok()),
        other => panic!("expected Event::ListAccessUpdated, got {other:?}"),
    }

    // 4. DeleteListAccess surfaces partial failure details from failed_to_update_user_ids.
    fake.state.lock().unwrap().list_access_delete_result = api::ListAccessDeleteResult {
        failed_to_update_channel_ids: Vec::new(),
        failed_to_update_user_ids: vec![UserId::from("U9")],
    };
    core.handle(Command::Collab(CollabCommand::DeleteListAccess {
        list_id: "F0LIST".into(),
        channel_ids: Vec::new(),
        user_ids: vec![UserId::from("U2"), UserId::from("U9")],
    }))
    .await;
    match next(&mut rx).await {
        Event::ListAccessUpdated {
            list_id,
            result: Err(err),
            ..
        } => {
            assert_eq!(list_id, "F0LIST");
            assert!(
                err.contains("1 user(s)"),
                "partial failure should be reported: {err}"
            );
        }
        other => panic!("expected Event::ListAccessUpdated with partial failure, got {other:?}"),
    }

    // 5. FetchListRecordComments returns list item thread messages.
    core.handle(Command::Collab(CollabCommand::FetchListRecordComments {
        list_id: "F0LIST".into(),
        record_id: "Rec1".into(),
    }))
    .await;
    match next(&mut rx).await {
        Event::ListRecordComments {
            list_id,
            record_id,
            messages,
        } => {
            assert_eq!(list_id, "F0LIST");
            assert_eq!(record_id, "Rec1");
            assert_eq!(messages.len(), 2);
            assert_eq!(messages[0].text, "List comment root");
        }
        other => panic!("expected Event::ListRecordComments, got {other:?}"),
    }

    let state = fake.state.lock().unwrap();
    // 2 calls from mixed share (channel XOR user) + 1 call from silent share = 3 calls.
    assert_eq!(state.list_access_set_calls.len(), 3);
    assert_eq!(
        state.list_access_set_calls[0].2,
        vec![ChannelId::from("C1")]
    );
    assert!(state.list_access_set_calls[0].3.is_empty());
    assert!(state.list_access_set_calls[1].2.is_empty());
    assert_eq!(state.list_access_set_calls[1].3, vec![UserId::from("U2")]);
    assert_eq!(
        state.posted_messages,
        vec![(
            ChannelId::from("C1"),
            "https://acme.slack.com/lists/T1/F0LIST".to_owned()
        )],
        "only the non-silent channel share should post the permalink"
    );
    assert_eq!(state.list_access_delete_calls.len(), 1);
}

#[tokio::test]
async fn hydrate_resolves_slack_connect_external_teams_from_boot_channels() {
    let fake = FakeBackend::default();
    let partner_team = Team {
        id: TeamId::from("T0PARTNER"),
        name: "Partner Co".into(),
        domain: "partner".into(),
        is_verified: true,
        ..Default::default()
    };
    fake.set_team(partner_team.clone());
    fake.state.lock().unwrap().user_boot_override = Some(UserBoot {
        channels: vec![Conversation {
            id: ChannelId::from("C0EXT"),
            name: Some("ext-partner".into()),
            is_channel: true,
            is_ext_shared: true,
            conversation_host_id: Some(TeamId::from("T0PARTNER")),
            connected_team_ids: vec![TeamId::from("T0PARTNER"), TeamId::from("T1")],
            ..Default::default()
        }],
        team: Some(Team {
            id: TeamId::from("T1"),
            name: "Acme".into(),
            domain: "acme".into(),
            ..Default::default()
        }),
        ..Default::default()
    });

    let (store, events, mut rx) = fake_core_parts();
    let core = Core::new(
        fake.clone(),
        Arc::clone(&store),
        TeamId::from("T1"),
        events,
        test_media_tx(),
    )
    .await;
    core.hydrate().await;

    // Wait briefly for the spawned external-team resolution task to complete.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let persisted = store
            .lock()
            .unwrap()
            .teams(&TeamId::from("T1"))
            .unwrap_or_default()
            .into_iter()
            .any(|t| t.id == TeamId::from("T0PARTNER"));
        if fake.team_info_calls().contains(&TeamId::from("T0PARTNER")) && persisted {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for hydrate to resolve Slack Connect external team T0PARTNER"
        );
        let _ = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await;
    }

    let saved = store
        .lock()
        .unwrap()
        .teams(&TeamId::from("T1"))
        .unwrap()
        .into_iter()
        .find(|t| t.id == TeamId::from("T0PARTNER"))
        .expect("external team persisted in store");
    assert_eq!(saved.name, "Partner Co");
    assert!(saved.is_verified);
}

#[tokio::test]
async fn workspace_backfill_commands_report_progress_and_honor_pause_resume() {
    use crate::{BackfillStatus, WorkspaceCommand};

    let fake = FakeBackend::default();
    let ws = TeamId::from("T1");
    let c1 = ChannelId::from("C_BF_1");
    let c2 = ChannelId::from("C_BF_2");

    let (store, events, mut rx) = fake_core_parts();
    {
        let mut s = store.lock().unwrap();
        s.upsert_conversations(
            &ws,
            &[
                Conversation {
                    id: c1.clone(),
                    name: Some("general".into()),
                    is_channel: true,
                    is_member: true,
                    ..Conversation::default()
                },
                Conversation {
                    id: c2.clone(),
                    name: Some("ops".into()),
                    is_channel: true,
                    is_member: true,
                    ..Conversation::default()
                },
            ],
        )
        .unwrap();
    }
    {
        let mut st = fake.state.lock().unwrap();
        st.history = vec![Message {
            ts: Ts::from("1700000100.000100"),
            text: "root in general".into(),
            reply_count: Some(1),
            ..Message::default()
        }];
        st.thread = vec![
            Message {
                ts: Ts::from("1700000100.000100"),
                text: "root in general".into(),
                reply_count: Some(1),
                ..Message::default()
            },
            Message {
                ts: Ts::from("1700000101.000100"),
                thread_ts: Some(Ts::from("1700000100.000100")),
                text: "reply in general".into(),
                ..Message::default()
            },
        ];
    }

    let mut core = Core::new(
        fake,
        Arc::clone(&store),
        ws.clone(),
        events,
        test_media_tx(),
    )
    .await;

    // 1. QueryBackfillProgress before running returns Idle inventory for both channels.
    core.handle(Command::Workspace(WorkspaceCommand::QueryBackfillProgress))
        .await;
    let Event::BackfillProgress(initial) = next(&mut rx).await else {
        panic!("expected BackfillProgress on query");
    };
    assert_eq!(initial.total_channels, 2);
    assert_eq!(initial.completed_channels, 0);
    assert_eq!(initial.status, BackfillStatus::Idle);

    // 2. Pause -> Resume toggles paused flag via Event::BackfillProgress.
    core.handle(Command::Workspace(WorkspaceCommand::PauseBackfill))
        .await;
    let Event::BackfillProgress(paused) = next(&mut rx).await else {
        panic!("expected BackfillProgress on pause");
    };
    assert!(paused.paused);

    core.handle(Command::Workspace(WorkspaceCommand::ResumeBackfill))
        .await;
    let Event::BackfillProgress(resumed) = next(&mut rx).await else {
        panic!("expected BackfillProgress on resume");
    };
    assert!(!resumed.paused);

    // 3. BackfillWorkspace with include_threads=true walks both channels + thread reply.
    core.handle(Command::Workspace(WorkspaceCommand::BackfillWorkspace {
        include_threads: true,
    }))
    .await;

    let mut saw_complete = false;
    for _ in 0..32 {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Ok(Event::BackfillProgress(p))) if p.status == BackfillStatus::Complete => {
                assert_eq!(p.total_channels, 2);
                assert_eq!(p.completed_channels, 2);
                assert!(p.replies_stored >= 1);
                saw_complete = true;
                break;
            }
            Ok(Ok(_)) => {}
            other => panic!("unexpected event waiting for workspace backfill: {other:?}"),
        }
    }
    assert!(saw_complete, "workspace backfill should reach Complete");

    let replies = store
        .lock()
        .unwrap()
        .thread_replies(&ws, &c1, &Ts::from("1700000100.000100"))
        .unwrap();
    assert_eq!(replies.len(), 2);

    // 4. Pausing then issuing an explicit BackfillChannel clears stale paused state and completes.
    core.handle(Command::Workspace(WorkspaceCommand::PauseBackfill))
        .await;
    let Event::BackfillProgress(paused_again) = next(&mut rx).await else {
        panic!("expected BackfillProgress on second pause");
    };
    assert!(paused_again.paused);

    core.handle(Command::Workspace(WorkspaceCommand::BackfillChannel {
        channel: c2.clone(),
        include_threads: false,
    }))
    .await;
    let mut saw_ch_complete = false;
    for _ in 0..32 {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Ok(Event::BackfillProgress(p)))
                if !p.paused
                    && p.status == BackfillStatus::Complete
                    && p.channels.iter().any(|c| c.channel == c2 && c.complete) =>
            {
                saw_ch_complete = true;
                break;
            }
            Ok(Ok(_)) => {}
            other => panic!("unexpected event waiting for single-channel backfill: {other:?}"),
        }
    }
    assert!(
        saw_ch_complete,
        "BackfillChannel after PauseBackfill must clear stale pause and complete"
    );
}
