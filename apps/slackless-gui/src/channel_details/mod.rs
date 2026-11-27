//! Slack's channel details dialog: the About and Members tabs over a channel's record, its
//! member list and the connected organizations of a Slack Connect channel.

mod orgs;
mod render;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, Subscription, Window,
};
use slackless_core::NotifyLevel;
use slackless_core::types::{ChannelId, Conversation, TeamId, UserId};

use crate::drafts::now_secs;
use crate::fixtures::Fixtures;

pub use orgs::normalize_channel_slug;
use orgs::{
    FilterChoice, MemberFilter, current_status_emoji, filter_choices, is_listed, non_empty,
    profiles_by_id,
};
pub(crate) use orgs::{OrgLine, external_orgs, host_team, org_chip, orgs, own_team};

/// Members listed at once (the directory caps its lists the same way).
pub(super) const MAX_ROWS: usize = 30;

/// Slack's channel details inset: the content's side padding (its rules run edge to edge).
pub(super) const PAD: f32 = 28.;

/// What the dialog can do, executed by the root (it owns the bridge and the views).
pub enum ChannelDetailsEvent {
    /// "Leave channel": the root reuses its leave path, then closes the dialog.
    Leave(ChannelId),
    /// A channel rename save: the root posts it to the core (`conversations.rename`).
    Rename { channel: ChannelId, name: String },
    /// Archive an active channel (`conversations.archive`).
    Archive(ChannelId),
    /// Unarchive an archived channel (`conversations.unarchive`).
    Unarchive(ChannelId),
    /// Permanently delete a channel (`admin.conversations.delete`, `F-CORE-053`).
    Delete(ChannelId),
    /// Toggle channel-wide thread auto-following (`F-CORE-111`).
    ToggleFollowThreads(ChannelId),
    /// Convert a public channel or group DM into a private channel (`conversations.convertToPrivate`).
    ConvertToPrivate(ChannelId),
    /// Open the "Add people" picker for this channel or DM.
    AddPeople(ChannelId),
    /// A topic save: the root posts it to the core (`conversations.setTopic`).
    SetTopic { channel: ChannelId, topic: String },
    /// A description save: the root posts it to the core (`conversations.setPurpose`).
    SetPurpose { channel: ChannelId, purpose: String },
    /// The button row's huddle button.
    StartHuddle,
    /// The notifications dropdown's choice, the header bell's sibling.
    SetNotifyLevel {
        channel: ChannelId,
        level: NotifyLevel,
    },
    /// A member row: open the person's profile panel (after the dialog closes).
    OpenProfile(UserId),
    /// Export this channel's locally archived messages as Markdown (`markdown = true`) or JSON.
    Export { channel: ChannelId, markdown: bool },
    /// Start background history + thread backfill for this channel.
    Backfill(ChannelId),
}

/// Which of the dialog's tabs shows: the About cards or the member list.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DetailsTab {
    About,
    Members,
}

/// Where the Members tab's list stands.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MemberList {
    /// Live mode, until the core announces the open-time fetch.
    Loading,
    Ready,
    /// The fetch failed and no earlier list is known: the tab says so instead of loading
    /// forever.
    Failed,
}

/// One row of the Members tab, built when the members or the snapshot change.
pub(super) struct MemberRow {
    id: UserId,
    name: String,
    /// `name` lowercased: what the filter matches.
    key: String,
    /// The member's organization when they are from another one
    /// ([`Fixtures::external_team`]): the row's avatar badge, its square presence glyph and
    /// the organization filter read it.
    team: Option<TeamId>,
    /// The display name shown after the full name, Slack's member list's second text.
    handle: Option<String>,
    /// The person's title, the row's second line.
    job: Option<String>,
    /// A current status emoji, trimmed of its colons; rendered as a glyph.
    emoji: Option<String>,
    /// Holds Slack's channel-manager role here.
    manager: bool,
    /// A guest account (`is_restricted` / `is_ultra_restricted`).
    guest: bool,
}

/// Which About card is being edited.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum EditField {
    Name,
    Topic,
    Description,
}

/// The card being edited: its field and the draft state.
pub(super) struct Editing {
    field: EditField,
    draft: Entity<TextareaState>,
}

pub struct ChannelDetails {
    fixtures: Arc<Fixtures>,
    channel: ChannelId,
    tab: DetailsTab,
    /// The channel's member ids as last announced by the core (live) or the conversation
    /// row (sample); empty while the list loads.
    members: Vec<UserId>,
    member_list: MemberList,
    /// The channel's managers as the core last announced them (empty until then).
    managers: Vec<UserId>,
    /// The Members tab's rows by name and the About tab's Organizations lines, rebuilt when
    /// the members, the managers or the snapshot change - never per frame.
    rows: Vec<MemberRow>,
    orgs: Vec<OrgLine>,
    /// The member filter's choices and counts over [`Self::rows`], rebuilt with the rows.
    filter_choices: Arc<Vec<FilterChoice>>,
    /// The Members tab's filter, Slack's select beside "Find people".
    member_filter: MemberFilter,
    /// The Members tab's filter.
    filter: Entity<TextareaState>,
    /// The About card being edited, with its draft.
    editing: Option<Editing>,
    /// Saves applied here while the core confirms them (a live reload overwrites them).
    local_name: Option<String>,
    local_topic: Option<String>,
    local_purpose: Option<String>,
    /// Whether the user automatically follows all new threads in this channel (`F-CORE-111`).
    follow_all_threads: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ChannelDetailsEvent> for ChannelDetails {}

impl ChannelDetails {
    pub fn new(fixtures: Arc<Fixtures>, window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let filter = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Find people")
                    .auto_grow(1, 1)
            });
            let subscriptions =
                vec![
                    cx.subscribe(&filter, |_: &mut Self, _, _: &InputEvent, cx| {
                        cx.notify();
                    }),
                ];
            Self {
                fixtures,
                channel: ChannelId::default(),
                tab: DetailsTab::About,
                members: Vec::new(),
                member_list: MemberList::Ready,
                managers: Vec::new(),
                rows: Vec::new(),
                orgs: Vec::new(),
                filter_choices: Arc::new(Vec::new()),
                member_filter: MemberFilter::All,
                filter,
                editing: None,
                local_name: None,
                local_topic: None,
                local_purpose: None,
                follow_all_threads: false,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Point the dialog at `channel`: called every time the dialog opens. `members` is the
    /// freshest list the root holds; `loading_members` keeps the Members tab on its
    /// placeholder until the core announces the open-time fetch; `tab` is the tab the
    /// dialog opens on (the header's members chip opens straight onto Members).
    pub fn open(
        &mut self,
        fixtures: Arc<Fixtures>,
        channel: ChannelId,
        members: Vec<UserId>,
        loading_members: bool,
        tab: DetailsTab,
        cx: &mut Context<Self>,
    ) {
        self.fixtures = fixtures;
        self.channel = channel;
        self.members = members;
        self.member_list = if loading_members {
            MemberList::Loading
        } else {
            MemberList::Ready
        };
        self.managers.clear();
        self.rebuild();
        self.tab = tab;
        self.editing = None;
        self.member_filter = MemberFilter::All;
        self.local_name = None;
        self.local_topic = None;
        self.local_purpose = None;
        cx.notify();
    }

    /// Update the channel-wide thread following toggle state (`F-CORE-111`).
    pub fn set_follow_all_threads(&mut self, follow: bool, cx: &mut Context<Self>) {
        self.follow_all_threads = follow;
        cx.notify();
    }

    /// Slack's member list puts the caret in "Find people" as the dialog opens; the root's
    /// dialog builder calls this on its first build.
    pub fn find_people_focus(&self, cx: &App) -> gpui_kit::gpui::FocusHandle {
        self.filter.read(cx).focus_handle(cx)
    }

    /// Live refresh: re-read everything from `fixtures`, keeping the open tab, filter and
    /// draft.    /// The core announced `channel`'s managers (`Event::ChannelManagers`); a late answer
    /// for a channel closed since is dropped.
    pub fn set_managers(
        &mut self,
        channel: &ChannelId,
        managers: Vec<UserId>,
        cx: &mut Context<Self>,
    ) {
        if *channel == self.channel {
            self.managers = managers;
            self.rebuild();
            cx.notify();
        }
    }

    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        self.rebuild();
        cx.notify();
    }

    /// The open-time member fetch for `channel` was announced (`Event::ConversationInfo`):
    /// its list, or none when the fetch failed (an earlier list then stays shown).
    pub fn set_conversation_info(
        &mut self,
        channel: &ChannelId,
        members: Vec<UserId>,
        cx: &mut Context<Self>,
    ) {
        if channel != &self.channel {
            return;
        }
        if !members.is_empty() {
            self.members = members;
            self.rebuild();
        }
        self.member_list = if self.members.is_empty() {
            MemberList::Failed
        } else {
            MemberList::Ready
        };
        cx.notify();
    }

    /// Rebuild the member rows and the organization lines from the members and the snapshot:
    /// one pass over the directory whatever the channel's size, so the filter only matches
    /// strings.
    fn rebuild(&mut self) {
        if self.members.is_empty() {
            self.rows.clear();
            self.orgs.clear();
            return;
        }
        let fixtures = Arc::clone(&self.fixtures);
        let own = own_team(&fixtures);
        let profiles = profiles_by_id(fixtures.people());
        let now = now_secs();
        let mut rows: Vec<MemberRow> = self
            .members
            .iter()
            .filter(|member| is_listed(member, &profiles))
            .map(|member| {
                let profile = profiles.get(member).copied();
                let name = profile
                    .map_or(member.as_str(), |user| fixtures.name_of(user))
                    .to_owned();
                MemberRow {
                    id: member.clone(),
                    key: name.to_lowercase(),
                    team: fixtures.external_team(member).map(|team| team.id.clone()),
                    handle: profile.and_then(|user| non_empty(&user.profile.display_name)),
                    job: profile.and_then(|user| non_empty(&user.profile.title)),
                    emoji: profile.and_then(|user| current_status_emoji(&user.profile, now)),
                    manager: self.managers.contains(member),
                    guest: profile
                        .is_some_and(|user| user.is_restricted || user.is_ultra_restricted),
                    name,
                }
            })
            .collect();
        rows.sort_by(|a, b| a.key.cmp(&b.key));
        self.rows = rows;
        let shared = fixtures
            .conversation(&self.channel)
            .is_some_and(|c| c.is_ext_shared || c.is_shared);
        self.orgs = if shared {
            orgs(&own, &self.members, fixtures.people(), &fixtures.teams)
        } else {
            Vec::new()
        };
        self.filter_choices = Arc::new(filter_choices(&fixtures, &self.rows));
    }

    /// What the Members tab says instead of rows: the load in flight, its failure, a channel
    /// without a list, or a filter matching nobody; `None` when there are rows to show.
    fn member_note(&self, nothing_matches: bool, filtering: bool) -> Option<&'static str> {
        match self.member_list {
            MemberList::Loading => Some("Loading members…"),
            MemberList::Failed => Some("Couldn't load this channel's members."),
            MemberList::Ready if self.members.is_empty() => {
                Some("No member list for this channel.")
            }
            MemberList::Ready if !nothing_matches => None,
            MemberList::Ready if filtering => Some("Nothing matches your search."),
            MemberList::Ready => Some("No members to show."),
        }
    }

    fn conversation(&self) -> Option<&Conversation> {
        self.fixtures.conversation(&self.channel)
    }

    fn needle(&self, cx: &Context<Self>) -> String {
        self.filter.read(cx).value().trim().to_lowercase()
    }

    /// The topic card shows the last save made here until the store's copy arrives.
    fn topic(&self, conversation: &Conversation) -> String {
        self.local_topic
            .clone()
            .or_else(|| conversation.topic.as_ref().map(|topic| topic.value.clone()))
            .unwrap_or_default()
    }

    /// The description card, like [`Self::topic`].
    fn purpose(&self, conversation: &Conversation) -> String {
        self.local_purpose
            .clone()
            .or_else(|| {
                conversation
                    .purpose
                    .as_ref()
                    .map(|purpose| purpose.value.clone())
            })
            .unwrap_or_default()
    }

    /// Begin editing `field`: the card swaps to a draft box with Save / Cancel.
    fn edit(&mut self, field: EditField, window: &mut Window, cx: &mut Context<Self>) {
        let conversation = self.conversation().cloned().unwrap_or_default();
        let initial = match field {
            EditField::Name => self
                .local_name
                .clone()
                .or_else(|| conversation.name.clone())
                .unwrap_or_default(),
            EditField::Topic => self.topic(&conversation),
            EditField::Description => self.purpose(&conversation),
        };
        let draft = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(match field {
                    EditField::Name => "channel-name",
                    EditField::Topic => "Add a topic",
                    EditField::Description => "Add a description",
                })
                .auto_grow(1, if field == EditField::Name { 1 } else { 3 })
        });
        if !initial.is_empty() {
            draft.update(cx, |state, cx| state.set_value(initial, window, cx));
        }
        self.editing = Some(Editing { field, draft });
        cx.notify();
    }

    /// Save the open draft: the root posts it; the card already shows the new value.
    fn save_edit(&mut self, cx: &mut Context<Self>) {
        let Some(editing) = self.editing.take() else {
            return;
        };
        let text = editing.draft.read(cx).value().trim().to_owned();
        match editing.field {
            EditField::Name => {
                let slug = normalize_channel_slug(&text);
                if !slug.is_empty() {
                    self.local_name = Some(slug.clone());
                    cx.emit(ChannelDetailsEvent::Rename {
                        channel: self.channel.clone(),
                        name: slug,
                    });
                }
            }
            EditField::Topic => {
                self.local_topic = Some(text.clone());
                cx.emit(ChannelDetailsEvent::SetTopic {
                    channel: self.channel.clone(),
                    topic: text,
                });
            }
            EditField::Description => {
                self.local_purpose = Some(text.clone());
                cx.emit(ChannelDetailsEvent::SetPurpose {
                    channel: self.channel.clone(),
                    purpose: text,
                });
            }
        }
        cx.notify();
    }
}
