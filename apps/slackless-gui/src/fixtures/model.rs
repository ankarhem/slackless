//! Production workspace snapshot ViewModel backed by `Arc<WorkspaceData>` so periodic
//! `Event::Counts` / `Event::ReadMarker` updates swap `counts` in O(1) without deep-cloning
//! collections or discarding lazy lookup indices.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::gpui::{Image, ImageSource, SharedString};
use slackless_core::NotifyLevel;
use slackless_core::api::ConversationCounts;
use slackless_core::types::{
    BotId, BotProfile, ChannelId, ChannelSection, Conversation, ConversationKind, FileId, Message,
    Presence, Team, TeamId, Ts, User, UserId, Usergroup, UsergroupId,
};

/// Live presence per user, shared by every snapshot of a workspace: a `presence_change` lands
/// in place and repaints, with no store reload.
pub type PresenceMap = Arc<RwLock<HashMap<UserId, Presence>>>;

/// Shared bot-profile cache (`bots.info`) updated in place as history resolves bot messages.
pub type BotProfileMap = Arc<RwLock<HashMap<BotId, BotProfile>>>;

/// A custom emoji's picture: a generated sample image, or a live `emoji.list` URL that GPUI's
/// `img` loads and caches. (`ImageSource` itself is not `Send`, so the snapshot keeps this.)
#[derive(Clone)]
pub enum EmojiImage {
    Sample(Arc<Image>),
    Url(SharedString),
}

impl EmojiImage {
    pub fn source(&self) -> ImageSource {
        match self {
            EmojiImage::Sample(image) => ImageSource::Image(image.clone()),
            EmojiImage::Url(url) => ImageSource::from(url.clone()),
        }
    }
}

/// Profile data outside `slackless_types::Profile`: `profile.pronouns` and the workspace's
/// custom profile fields resolved to `(label, value)` pairs (`team.profile.get` labels).
#[derive(Clone, Default)]
pub struct ProfileExtras {
    pub pronouns: Option<String>,
    pub fields: Vec<(String, String)>,
}

#[derive(Clone, Copy)]
pub(super) enum ConvBucket {
    Channel,
    Dm,
    Directory,
    Archived,
}

/// Immutable (copy-on-write via `Arc`) workspace data shared across count refreshes.
#[derive(Clone, Default)]
pub struct WorkspaceData {
    /// Every workspace in the switcher rail's display order (read via [`Fixtures::rail_workspaces`],
    /// never as the shown workspace's own team — use [`Fixtures::own_team`] for that).
    pub(super) workspaces: Vec<Team>,
    /// Every organization the snapshot knows: the signed-in workspace's own team first, then
    /// the connected teams of Slack Connect channels (live: stored `team.info` records).
    pub teams: Vec<Team>,
    /// The signed-in user; author of locally composed messages.
    pub self_user: UserId,
    /// Snapshot users before live `fresh_people` overlays (read via [`Fixtures::people`] /
    /// [`Fixtures::user`] so status, profile, and deactivation updates are never missed).
    pub(super) users: Vec<User>,
    /// Active `@handle` user groups (`usergroups.list`).
    pub usergroups: Vec<Usergroup>,
    /// Workspace slash commands from `client.appCommands` (merged into `/` autocomplete).
    pub app_commands: Vec<slackless_core::api::AppCommand>,
    pub channels: Vec<Conversation>,
    /// IMs and MPIMs.
    pub dms: Vec<Conversation>,
    /// Channel history, oldest first. Thread replies live in `threads`.
    pub messages: HashMap<ChannelId, Vec<Message>>,
    /// Each DM's latest top-level message: one store query in live mode ([`super::live`]), the last
    /// history message in fixture mode. Feeds the DM list's previews, stamps and order
    /// without loading histories.
    pub latest: HashMap<ChannelId, Message>,
    /// Replies keyed by `(channel, thread root ts)`, oldest first.
    pub threads: HashMap<(ChannelId, Ts), Vec<Message>>,
    /// Workspace channels the signed-in user has not joined (channel browser only).
    pub directory: Vec<Conversation>,
    /// Archived channels: never listed, only named (a shared message's "Thread in #old
    /// (archived)").
    pub archived: Vec<Conversation>,
    /// Last known presence per user (`presence_change` / `users.getPresence`).
    pub presence: PresenceMap,
    /// Resolved bot profiles (`bots.info`) keyed by `BotId`, shared across snapshots.
    pub bots: BotProfileMap,
    /// Conversations whose notification level is not "All new posts" (the header bell).
    pub notify_levels: HashMap<ChannelId, NotifyLevel>,
    /// The message hover bar's one-click reactions (none when the user turned them off).
    pub quick_reactions: Vec<String>,
    /// The emoji picker's Frequently Used, most used first, from the user's `emoji_use` pref
    /// (live mode); empty in the sample workspace, whose picker shows its defaults.
    pub frequent_emoji: Vec<String>,
    /// The user's emoji skin tone (`preferred_skin_tone`): 1 is the default yellow, 2..=6 the
    /// Fitzpatrick tones.
    pub skin_tone: u8,
    /// Decoded image thumbnails per file (live mode: downloaded `thumb_*` bytes).
    pub thumbnails: HashMap<FileId, Arc<Image>>,
    /// Locally persisted audio/video transcripts keyed by `FileId` (read via
    /// [`Fixtures::file_transcript`] / [`Fixtures::all_file_transcripts`] so transcripts landed
    /// since the snapshot are never missed).
    pub(super) file_transcripts: HashMap<FileId, String>,
    /// Timed transcript cues keyed by `FileId` (read via [`Fixtures::file_transcript_cues`]).
    pub(super) file_transcript_cues: HashMap<FileId, Vec<slackless_core::types::TranscriptCue>>,
    /// Profile data the typed `Profile` does not model (pronouns, custom fields).
    pub profile_extras: HashMap<UserId, ProfileExtras>,
    /// The workspace's custom emoji by name.
    pub custom_emoji: BTreeMap<String, EmojiImage>,
    /// Decoded Block Kit / attachment images keyed by their `image_url`.
    pub block_images: HashMap<String, Arc<Image>>,
    /// The bundled sample workspace. Demo-only data (the fixture huddle, channel bookmarks) is
    /// shown only here, never over a live workspace.
    pub sample: bool,
    /// Live mode: the signed-in user's sidebar sections in display order. Empty in the sample
    /// workspace, which shows plain Channels / Direct messages sections.
    pub sections: Vec<ChannelSection>,
    /// Show people by full name (the user's `display_real_names_override`, else the team's
    /// `display_real_names`).
    pub real_names: bool,
    /// @mentions show full names too (`mentions_real_names_override`, else the team's
    /// `display_real_names_in_mentions`); otherwise they show display names.
    pub real_names_in_mentions: bool,
    /// Live mode: ids of the sidebar sections the user collapsed in Slack.
    pub collapsed_sections: HashSet<String>,
    /// The color theme Slack stores for the user, applied at launch; `None` in the sample
    /// workspace, which keeps the reference screenshot's dark look.
    pub palette: Option<crate::theme::Palette>,
    /// Workspace-configured status presets (`team.prefs.custom_status_default_suggestions`).
    pub status_presets: Vec<(String, String, usize)>,
    /// Lazy O(1) lookup index into `users`.
    pub(super) user_index: OnceLock<HashMap<UserId, usize>>,
    /// Lazy O(1) lookup index into `channels` / `dms` / `directory` / `archived`.
    pub(super) conv_index: OnceLock<HashMap<ChannelId, (ConvBucket, usize)>>,
    /// Lazy O(1) lookup index into `teams`.
    pub(super) team_index: OnceLock<HashMap<TeamId, usize>>,
}

/// Each person's position in `users` (the first, should an id repeat).
fn index_users(users: &[User]) -> HashMap<UserId, usize> {
    let mut map = HashMap::with_capacity(users.len());
    for (ix, u) in users.iter().enumerate() {
        map.entry(u.id.clone()).or_insert(ix);
    }
    map
}

/// Snapshot of a workspace rendered by views: `Arc`-shared [`WorkspaceData`] paired with the
/// latest per-conversation read/unread `counts` and what changed since the data was read.
pub struct Fixtures {
    pub(super) data: Arc<WorkspaceData>,
    /// Live mode: Slack's own read and unread state per conversation (`client.counts`). Empty
    /// in the sample workspace, whose unreads come from its history.
    pub counts: HashMap<ChannelId, ConversationCounts>,
    /// Fresh records of people already in `users` (a status or profile change), looked up
    /// before them: such a change copies no workspace data.
    fresh_people: Arc<HashMap<UserId, User>>,
    /// Transcripts that landed after the data was read, looked up before its own.
    transcripts: Arc<HashMap<FileId, Transcript>>,
}

/// A transcript that landed after the snapshot was read: its trimmed text (empty clears the
/// stored one) and its timed cues.
#[derive(Clone)]
struct Transcript {
    text: String,
    cues: Vec<slackless_core::types::TranscriptCue>,
}

impl Deref for Fixtures {
    type Target = WorkspaceData;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl DerefMut for Fixtures {
    fn deref_mut(&mut self) -> &mut Self::Target {
        let data = Arc::make_mut(&mut self.data);
        data.user_index = OnceLock::new();
        data.conv_index = OnceLock::new();
        data.team_index = OnceLock::new();
        data
    }
}

impl Fixtures {
    pub(super) fn from_parts(
        data: WorkspaceData,
        counts: HashMap<ChannelId, ConversationCounts>,
    ) -> Self {
        Self {
            data: Arc::new(data),
            counts,
            fresh_people: Arc::default(),
            transcripts: Arc::default(),
        }
    }

    /// O(1) snapshot update swapping only `counts` while sharing the immutable [`WorkspaceData`]
    /// and its already-populated `user_index` / `conv_index` / `team_index` caches across
    /// periodic `Event::Counts` and `Event::ReadMarker` ticks.
    pub fn with_counts(&self, counts: HashMap<ChannelId, ConversationCounts>) -> Self {
        Self {
            data: Arc::clone(&self.data),
            counts,
            fresh_people: Arc::clone(&self.fresh_people),
            transcripts: Arc::clone(&self.transcripts),
        }
    }

    /// Locally persisted transcript for an audio or video file, if available.
    pub fn file_transcript(&self, id: &FileId) -> Option<&str> {
        match self.transcripts.get(id) {
            Some(landed) => Some(landed.text.as_str()),
            None => self.file_transcripts.get(id).map(String::as_str),
        }
        .filter(|s| !s.is_empty())
    }

    /// Timed transcript cues for an audio or video file, if available.
    pub fn file_transcript_cues(
        &self,
        id: &FileId,
    ) -> Option<&[slackless_core::types::TranscriptCue]> {
        let landed = self.transcripts.get(id).map(|t| t.cues.as_slice());
        landed
            .filter(|cues| !cues.is_empty())
            .or_else(|| self.file_transcript_cues.get(id).map(Vec::as_slice))
            .filter(|cues| !cues.is_empty())
    }

    /// Return a new [`Fixtures`] snapshot with `file_id`'s transcript inserted or updated.
    #[cfg(test)]
    pub fn with_file_transcript(&self, file_id: FileId, text: String) -> Self {
        self.with_file_transcript_cues(file_id, text, Vec::new())
    }

    /// Return a new [`Fixtures`] snapshot with `file_id`'s transcript text and timed cues updated.
    pub fn with_file_transcript_cues(
        &self,
        file_id: FileId,
        text: String,
        cues: Vec<slackless_core::types::TranscriptCue>,
    ) -> Self {
        let mut next = self.with_counts(self.counts.clone());
        let transcripts = Arc::make_mut(&mut next.transcripts);
        // Text alone keeps the cues already known.
        let cues = match transcripts.remove(&file_id) {
            Some(earlier) if cues.is_empty() => earlier.cues,
            _ => cues,
        };
        let text = text.trim().to_owned();
        transcripts.insert(file_id, Transcript { text, cues });
        next
    }

    /// A copy of this snapshot with `users` (fresh store records) in place of the people of
    /// the same ids and the others added: a status or profile change reaches the views without
    /// re-reading the rest of the workspace. People already listed are only looked up anew;
    /// someone new joins `users` itself, which member lists and org lines walk, so that copy
    /// takes the earlier updates along (the people index is carried over and extended).
    pub fn with_users(&self, users: Vec<User>) -> Self {
        let mut next = self.with_counts(self.counts.clone());
        let known = self.user_map();
        if users.iter().all(|user| known.contains_key(&user.id)) {
            Arc::make_mut(&mut next.fresh_people)
                .extend(users.into_iter().map(|user| (user.id.clone(), user)));
            return next;
        }
        let mut data = (*self.data).clone();
        let mut index = data
            .user_index
            .take()
            .unwrap_or_else(|| index_users(&data.users));
        for user in self.fresh_people.values().cloned().chain(users) {
            if let Some(slot) = index.get(&user.id).and_then(|&ix| data.users.get_mut(ix)) {
                *slot = user;
            } else {
                index.insert(user.id.clone(), data.users.len());
                data.users.push(user);
            }
        }
        data.user_index = OnceLock::from(index);
        next.data = Arc::new(data);
        next.fresh_people = Arc::default();
        next
    }

    /// Whether `other` holds this snapshot's workspace data and updates: it is this snapshot
    /// or one of its [`Fixtures::with_counts`] copies.
    pub fn shares_data(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.data, &other.data)
            && Arc::ptr_eq(&self.fresh_people, &other.fresh_people)
            && Arc::ptr_eq(&self.transcripts, &other.transcripts)
    }

    /// Every non-empty file transcript, merging freshly landed updates over the snapshot's map.
    pub fn all_file_transcripts(&self) -> HashMap<FileId, String> {
        let mut out: HashMap<FileId, String> = self
            .data
            .file_transcripts
            .iter()
            .filter(|(_, text)| !text.is_empty())
            .map(|(id, text)| (id.clone(), text.clone()))
            .collect();
        for (id, landed) in self.transcripts.iter() {
            if landed.text.is_empty() {
                out.remove(id);
            } else {
                out.insert(id.clone(), landed.text.clone());
            }
        }
        out
    }

    /// Workspaces in the switcher rail's display order. Do **not** use `.first()` to identify
    /// the shown workspace — use [`Self::own_team`] / [`Self::own_team_id`] instead.
    pub fn rail_workspaces(&self) -> &[Team] {
        &self.data.workspaces
    }

    /// Replace the switcher rail's workspace list in display order.
    pub fn set_rail_workspaces(&mut self, teams: Vec<Team>) {
        self.workspaces = teams;
    }

    /// Look up any workspace/team record by ID across `teams` and the switcher rail.
    pub fn workspace_by_id(&self, id: &TeamId) -> Option<&Team> {
        self.team_map()
            .get(id)
            .and_then(|&ix| self.teams.get(ix))
            .or_else(|| self.data.workspaces.iter().find(|t| t.id == *id))
    }

    /// Replace the snapshot's user list cleanly, clearing any stale `fresh_people` overlay.
    pub fn replace_users(&mut self, users: Vec<User>) {
        self.fresh_people = Arc::default();
        self.users = users;
    }

    #[cfg(test)]
    pub fn rail_workspaces_mut(&mut self) -> &mut Vec<Team> {
        &mut self.workspaces
    }

    #[cfg(test)]
    pub fn users_mut(&mut self) -> &mut Vec<User> {
        self.fresh_people = Arc::default();
        &mut self.users
    }

    #[cfg(test)]
    pub fn file_transcripts_mut(&mut self) -> &mut HashMap<FileId, String> {
        &mut self.file_transcripts
    }

    /// Everyone in `users`, each by their freshest record.
    pub fn people(&self) -> impl Iterator<Item = &User> {
        self.users
            .iter()
            .map(|user| self.fresh_people.get(&user.id).unwrap_or(user))
    }

    /// Effective read cursor for `id`: prefers live `counts[id].last_read`, falling back to
    /// the stored `Conversation::last_read`.
    pub fn last_read(&self, id: &ChannelId) -> Option<&Ts> {
        self.counts
            .get(id)
            .and_then(|c| c.last_read.as_ref())
            .or_else(|| self.conversation(id).and_then(|c| c.last_read.as_ref()))
    }

    pub fn user_name<'a>(&'a self, id: &'a UserId) -> &'a str {
        self.user(id).map_or(id.as_str(), |user| self.name_of(user))
    }

    /// The name `user` shows by: their full name where the workspace shows full names, else
    /// their display name ([`Fixtures::user_name`] for a profile already at hand).
    pub fn name_of<'a>(&self, user: &'a User) -> &'a str {
        let real = [user.profile.real_name.as_deref(), user.real_name.as_deref()]
            .into_iter()
            .flatten()
            .find(|name| !name.is_empty());
        match real {
            Some(real) if self.real_names => real,
            _ => user.display_name(),
        }
    }

    fn conv_map(&self) -> &HashMap<ChannelId, (ConvBucket, usize)> {
        self.data.conv_index.get_or_init(|| {
            let mut map = HashMap::with_capacity(
                self.channels.len() + self.dms.len() + self.directory.len() + self.archived.len(),
            );
            for (ix, c) in self.channels.iter().enumerate() {
                map.entry(c.id.clone()).or_insert((ConvBucket::Channel, ix));
            }
            for (ix, c) in self.dms.iter().enumerate() {
                map.entry(c.id.clone()).or_insert((ConvBucket::Dm, ix));
            }
            for (ix, c) in self.directory.iter().enumerate() {
                map.entry(c.id.clone())
                    .or_insert((ConvBucket::Directory, ix));
            }
            for (ix, c) in self.archived.iter().enumerate() {
                map.entry(c.id.clone())
                    .or_insert((ConvBucket::Archived, ix));
            }
            map
        })
    }

    fn user_map(&self) -> &HashMap<UserId, usize> {
        self.data
            .user_index
            .get_or_init(|| index_users(&self.users))
    }

    fn team_map(&self) -> &HashMap<TeamId, usize> {
        self.data.team_index.get_or_init(|| {
            let mut map = HashMap::with_capacity(self.teams.len());
            for (ix, t) in self.teams.iter().enumerate() {
                map.entry(t.id.clone()).or_insert(ix);
            }
            map
        })
    }

    /// Resolved bot profile for `bot_id` (from `bots.info`), if cached.
    pub fn bot_profile(&self, id: &BotId) -> Option<BotProfile> {
        self.bots
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    pub fn conversation(&self, id: &ChannelId) -> Option<&Conversation> {
        let map = self.conv_map();
        if let Some(&(bucket, ix)) = map.get(id) {
            let candidate = match bucket {
                ConvBucket::Channel => self.channels.get(ix),
                ConvBucket::Dm => self.dms.get(ix),
                ConvBucket::Directory => self.directory.get(ix),
                ConvBucket::Archived => self.archived.get(ix),
            };
            if let Some(conv) = candidate.filter(|c| c.id == *id) {
                return Some(conv);
            }
        }
        let total =
            self.channels.len() + self.dms.len() + self.directory.len() + self.archived.len();
        if map.len() != total {
            return self
                .channels
                .iter()
                .chain(&self.dms)
                .chain(&self.directory)
                .chain(&self.archived)
                .find(|c| c.id == *id);
        }
        None
    }

    pub fn user(&self, id: &UserId) -> Option<&User> {
        if let Some(fresh) = self.fresh_people.get(id) {
            return Some(fresh);
        }
        let map = self.user_map();
        if let Some(&ix) = map.get(id)
            && let Some(user) = self.users.get(ix).filter(|u| u.id == *id)
        {
            return Some(user);
        }
        if map.len() != self.users.len() {
            return self.users.iter().find(|u| u.id == *id);
        }
        None
    }

    pub fn usergroup(&self, id: &UsergroupId) -> Option<&Usergroup> {
        self.usergroups.iter().find(|g| g.id == *id)
    }

    pub fn usergroup_handle(&self, id: &UsergroupId) -> Option<&str> {
        self.usergroup(id)
            .map(|g| g.handle.as_str())
            .filter(|h| !h.is_empty())
    }

    pub fn presence(&self, user: &UserId) -> Option<Presence> {
        self.presence
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(user)
            .cloned()
    }

    /// The name an @mention of `id` shows (see [`WorkspaceData::real_names_in_mentions`]).
    pub fn mention_name<'a>(&'a self, id: &'a UserId) -> &'a str {
        if self.real_names_in_mentions {
            return self.user_name(id);
        }
        self.user(id).map(User::display_name).unwrap_or(id.as_str())
    }

    /// The image of the workspace custom emoji `name` (no colons).
    pub fn custom_emoji(&self, name: &str) -> Option<&EmojiImage> {
        self.custom_emoji.get(name)
    }

    /// The other parties of a DM (the peer of an IM, the members of an MPIM minus the
    /// signed-in user); empty for channels.
    pub fn peers<'a>(&'a self, conversation: &'a Conversation) -> Vec<&'a UserId> {
        match conversation.kind() {
            ConversationKind::Im => conversation.user.iter().collect(),
            ConversationKind::Mpim => conversation
                .members
                .iter()
                .filter(|u| **u != self.self_user)
                .collect(),
            ConversationKind::PublicChannel | ConversationKind::PrivateChannel => Vec::new(),
        }
    }

    /// One-line plain preview of a message: its text as plain text (see
    /// [`crate::messages::plain_text`]) on one line, file-only messages described by their first
    /// file.
    pub fn preview(&self, message: &Message) -> String {
        if message.text.trim().is_empty()
            && let Some(file) = message.files.first()
        {
            return format!("Shared a file: {}", file.name.as_deref().unwrap_or("file"));
        }
        crate::messages::plain_text(&message.text, self).replace('\n', " ")
    }

    /// The signed-in workspace's own team record: it leads `teams`, while `workspaces` lists
    /// every workspace in the rail's order, whichever tile comes first.
    pub fn own_team(&self) -> Option<&Team> {
        self.teams.first()
    }

    /// The signed-in workspace's own team ID, if present (see [`Self::own_team`]).
    pub fn own_team_id(&self) -> Option<&TeamId> {
        self.own_team().map(|team| &team.id)
    }

    /// Look up any known external organization record by team ID (returns `None` for own workspace).
    pub fn external_team_by_id(&self, team_id: &TeamId) -> Option<&Team> {
        if self.own_team_id().is_some_and(|own| own == team_id) {
            return None;
        }
        let &ix = self.team_map().get(team_id)?;
        self.teams.get(ix)
    }

    /// The organization of `user` when it is not the signed-in workspace's own and its record
    /// is known: Slack badges such people's avatars with its icon.
    pub fn external_team(&self, user: &UserId) -> Option<&Team> {
        let team = self.user(user)?.effective_team_id()?;
        self.external_team_by_id(team)
    }

    /// True when `user` belongs to an external organization (`is_stranger` or a different team
    /// ID, even before `team.info` finishes resolving).
    pub fn is_external_user(&self, user: &UserId) -> bool {
        let Some(own) = self.own_team_id() else {
            return false;
        };
        self.user(user).is_some_and(|u| u.is_external_to(own))
    }

    /// The external organization that owns the bot/app authoring `message`, when it belongs to
    /// another organization (`F-COLLAB-804`).
    pub fn external_bot_team(&self, message: &Message) -> Option<&Team> {
        if let Some(team_id) = message
            .bot_profile
            .as_ref()
            .and_then(|b| b.team_id.as_ref())
            .filter(|id| !id.as_str().is_empty())
            && let Some(team) = self.external_team_by_id(team_id)
        {
            return Some(team);
        }
        if let Some(bot_id) = message.bot_id.as_ref()
            && let Some(cached) = self.bot_profile(bot_id)
            && let Some(team_id) = cached.team_id.as_ref().filter(|id| !id.as_str().is_empty())
            && let Some(team) = self.external_team_by_id(team_id)
        {
            return Some(team);
        }
        if let Some(user_id) = message.user.as_ref()
            && self.user(user_id).is_some_and(|u| u.is_bot)
        {
            return self.external_team(user_id);
        }
        None
    }

    /// The hosting/owning organization of `conversation` (`conversation_host_id`, else
    /// `context_team_id`), resolved against known teams and workspaces (`F-COLLAB-798`, `828`).
    pub fn channel_owner_team(&self, conversation: &Conversation) -> Option<&Team> {
        let host_id = conversation.host_team_id()?;
        self.team_map()
            .get(&host_id)
            .and_then(|&ix| self.teams.get(ix))
            .or_else(|| self.workspaces.iter().find(|t| t.id == host_id))
    }

    /// The other organizations a Slack Connect conversation is shared with, in Slack's own
    /// order (`shared_team_ids` / `connected_team_ids` / `connected_limited_team_ids`, with
    /// member/host fallback), as far as their team records are known.
    pub fn shared_teams(&self, conversation: &Conversation) -> Vec<&Team> {
        let own = self.own_team_id();
        let team_map = self.team_map();
        let mut out: Vec<&Team> = Vec::new();
        let mut push_id = |id: &TeamId| {
            if own.is_some_and(|o| o == id) {
                return;
            }
            if let Some(&ix) = team_map.get(id)
                && let Some(team) = self.teams.get(ix)
                && !out.iter().any(|existing| existing.id == team.id)
            {
                out.push(team);
            }
        };
        for id in conversation.external_team_ids(own) {
            push_id(&id);
        }
        if let Some(host) = conversation.host_team_id() {
            push_id(&host);
        }
        if out.is_empty() && conversation.is_external_connect() {
            for member in &conversation.members {
                if let Some(team) = self.external_team(member)
                    && !out.iter().any(|existing| existing.id == team.id)
                {
                    out.push(team);
                }
            }
        }
        out
    }

    /// Sidebar/header label: `# name` for channels, the other parties' names for DMs.
    pub fn conversation_label(&self, conversation: &Conversation) -> String {
        match conversation.kind() {
            ConversationKind::Im => conversation.user.as_ref().map_or_else(
                || conversation.id.to_string(),
                |u| self.user_name(u).to_owned(),
            ),
            // Slack names a group DM by its other people, alphabetically.
            ConversationKind::Mpim => {
                let mut names: Vec<&str> = conversation
                    .members
                    .iter()
                    .filter(|u| **u != self.self_user)
                    .map(|u| self.user_name(u))
                    .collect();
                names.sort_by_cached_key(|name| name.to_lowercase());
                names.join(", ")
            }
            ConversationKind::PublicChannel | ConversationKind::PrivateChannel => format!(
                "# {}",
                conversation
                    .name
                    .as_deref()
                    .unwrap_or(conversation.id.as_str())
            ),
        }
    }

    /// A message authored now by the signed-in user (optimistic local echo).
    pub fn local_message(
        &self,
        channel: &ChannelId,
        text: String,
        thread_ts: Option<Ts>,
    ) -> Message {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Message {
            ts: Ts::new(format!("{}.{:06}", now.as_secs(), now.subsec_micros())),
            thread_ts,
            channel: Some(channel.clone()),
            user: Some(self.self_user.clone()),
            text,
            ..Default::default()
        }
    }
}
