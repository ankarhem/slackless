//! Slack's channel details dialog: the About and Members tabs over a channel's record, its
//! member list and the connected organizations of a Slack Connect channel. Opened by clicking
//! the channel name in the conversation header; the root mounts it in a gpui-kit dialog and
//! feeds it live data ([`ChannelDetailsEvent`] carries the actions upward).
//!
//! `ChannelDetails::new(Arc<Fixtures>, &mut Window, &mut App) -> Entity<ChannelDetails>`
//! `ChannelDetails::open(&mut self, Arc<Fixtures>, ChannelId, members, loading, tab, cx)`
//! `ChannelDetails::find_people_focus(&self, &App) -> FocusHandle` (Slack's open focus)
//! `ChannelDetails::set_fixtures(&mut self, Arc<Fixtures>, &mut Context<Self>)` (live reloads)
//! `ChannelDetails::set_conversation_info(&mut self, &ChannelId, Vec<UserId>, cx)`
//! emits `ChannelDetailsEvent`

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, AnyElement, App, AppContext as _, BoxShadow, Context, Div, Entity, EventEmitter,
    Focusable as _, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, img, point, px,
};
use slackless_core::NotifyLevel;
use slackless_core::types::{
    ChannelId, Conversation, Presence, Profile, Team, TeamId, User, UserId,
};

use crate::avatars::{user_chip, with_external_badge};
use crate::drafts::now_secs;
use crate::fixtures::Fixtures;

/// Members listed at once (the directory caps its lists the same way).
const MAX_ROWS: usize = 30;

/// Slack's channel details inset: the content's side padding (its rules run edge to edge).
const PAD: f32 = 28.;

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
}

/// Which of the dialog's tabs shows: the About cards or the member list.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DetailsTab {
    About,
    Members,
}

/// Where the Members tab's list stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MemberList {
    /// Live mode, until the core announces the open-time fetch.
    Loading,
    Ready,
    /// The fetch failed and no earlier list is known: the tab says so instead of loading
    /// forever.
    Failed,
}

/// One row of the Members tab, built when the members or the snapshot change.
struct MemberRow {
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
enum EditField {
    Name,
    Topic,
    Description,
}

/// The card being edited: its field and the draft state.
struct Editing {
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
        let profiles = profiles_by_id(&fixtures.users);
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
            orgs(&own, &self.members, &fixtures.users, &fixtures.teams)
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

impl Render for ChannelDetails {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted_foreground, foreground, ring, background) = (
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.ring,
            theme.background,
        );
        let tab = self.tab;
        let member_count = if self.members.is_empty() {
            self.conversation()
                .and_then(|conversation| conversation.num_members)
                .unwrap_or(0)
        } else {
            self.rows.len() as u32
        };

        // Slack's tab row under the title: tabs 20pt apart, the active one in the text color
        // over a 2pt underline that covers the rule; a tab's count keeps the regular weight.
        let underline = foreground.opacity(0.82);
        let tab_row = [
            (DetailsTab::About, "About", None, "details-tab-About"),
            (
                DetailsTab::Members,
                "Members",
                Some(member_count),
                "details-tab-Members",
            ),
        ]
        .map(|(entry, label, count, id)| {
            let active = entry == tab;
            h_flex()
                .id(SharedString::from(id))
                .h_full()
                .mb(px(-1.))
                .items_center()
                .gap_1()
                .cursor_pointer()
                .text_sm()
                .text_color(if active { foreground } else { muted_foreground })
                .when(active, |tab| tab.border_b_2().border_color(underline))
                .hover(|el| el.text_color(foreground))
                .child(
                    div()
                        .when(active, |label| label.font_weight(FontWeight::SEMIBOLD))
                        .child(label),
                )
                .children(count.map(|count| count.to_string()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = entry;
                    cx.notify();
                }))
        });

        // Slack's dialog grows with the window; the member list scrolls inside it.
        let height = (window.viewport_size().height * 0.85 - px(100.)).max(px(360.));
        v_flex()
            .h(height)
            .w_full()
            .child(self.button_row(cx))
            .child(
                h_flex()
                    .items_stretch()
                    .flex_none()
                    .px(px(PAD))
                    .gap(px(20.))
                    .h(px(32.))
                    .border_b_1()
                    .border_color(border)
                    .children(tab_row),
            )
            .child(match tab {
                DetailsTab::About => self.about_tab(cx).into_any_element(),
                DetailsTab::Members => {
                    let needle = self.needle(cx);
                    // Slack's field carries the blue focus ring while the caret is in it
                    // (the dialog opens with it focused).
                    let find_focused = self.filter.read(cx).focus_handle(cx).is_focused(window);
                    let filtering = !needle.is_empty() || self.member_filter != MemberFilter::All;
                    let member_filter = &self.member_filter;
                    let matching: Vec<&MemberRow> = self
                        .rows
                        .iter()
                        .filter(|row| member_filter.keeps(row))
                        .filter(|row| needle.is_empty() || row.key.contains(&needle))
                        .collect();
                    let find_people = h_flex()
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .h(px(36.))
                        .rounded(px(8.))
                        // Opaque, so the focus ring's spread shows only outside the field.
                        .bg(background)
                        .border_1()
                        .border_color(if find_focused { ring } else { border })
                        .when(find_focused, |field| {
                            field.shadow(vec![BoxShadow {
                                color: ring.opacity(0.3),
                                offset: point(px(0.), px(0.)),
                                blur_radius: px(0.),
                                spread_radius: px(3.),
                                inset: false,
                            }])
                        })
                        .child(
                            Icon::new(IconName::Search)
                                .text_color(muted_foreground)
                                .size(px(16.)),
                        )
                        .child(
                            div().flex_1().min_w_0().child(
                                Textarea::new(&self.filter)
                                    .appearance(false)
                                    .bordered(false),
                            ),
                        );
                    let filter_select = self.filter_select(cx);
                    let add_channel = self.channel.clone();
                    let add_people = Button::new("details-add-people")
                        .outline()
                        .flex_none()
                        .h(px(36.))
                        .rounded(px(8.))
                        .icon(IconName::UserPlus)
                        .label("Add people")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(ChannelDetailsEvent::AddPeople(add_channel.clone()));
                        }));
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .child(
                            h_flex()
                                .flex_none()
                                .items_center()
                                .gap_2()
                                .px(px(PAD))
                                .py_4()
                                .border_b_1()
                                .border_color(border)
                                .child(find_people)
                                .child(filter_select)
                                .child(add_people),
                        )
                        .child(v_flex().flex_1().min_h_0().child(
                            if let Some(note) = self.member_note(matching.is_empty(), filtering) {
                                div()
                                    .px(px(PAD))
                                    .py_6()
                                    .text_sm()
                                    .text_color(muted_foreground)
                                    .child(note)
                                    .into_any_element()
                            } else {
                                v_flex()
                                    .id("details-member-list")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .children(
                                        matching
                                            .iter()
                                            .take(MAX_ROWS)
                                            .enumerate()
                                            .map(|(ix, row)| self.member_row(ix, row, cx)),
                                    )
                                    .when(matching.len() > MAX_ROWS, |list| {
                                        list.child(
                                            div()
                                                .px(px(PAD))
                                                .py_3()
                                                .text_xs()
                                                .text_color(muted_foreground)
                                                .child(format!(
                                                    "{} of {} members shown",
                                                    MAX_ROWS,
                                                    matching.len()
                                                )),
                                        )
                                    })
                                    .into_any_element()
                            },
                        ))
                        .into_any_element()
                }
            })
    }
}

impl ChannelDetails {
    /// The row of actions under the title: the notification level's dropdown and the huddle
    /// button (the header row's siblings, Slack shows the star unstarred there but
    /// slackless has no star backend, so it is left out).
    fn button_row(&self, cx: &mut Context<Self>) -> Div {
        let this = cx.entity().downgrade();
        let channel = self.channel.clone();
        let level = self
            .fixtures
            .notify_levels
            .get(&self.channel)
            .copied()
            .unwrap_or(NotifyLevel::Everything);
        let level_label = match level {
            NotifyLevel::Everything => "All new posts",
            NotifyLevel::Mentions => "Just mentions",
            NotifyLevel::Muted => "Mute and hide",
        };
        let notify = Button::new("details-notify")
            .outline()
            .with_size(Size::Small)
            .icon(IconName::Bell)
            .label(level_label)
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                menu = menu.label("Notify you about…");
                for (choice, label) in [
                    (NotifyLevel::Everything, "All new posts"),
                    (NotifyLevel::Mentions, "Just mentions"),
                    (NotifyLevel::Muted, "Mute and hide"),
                ] {
                    let (this, channel) = (this.clone(), channel.clone());
                    menu = menu.item(PopupMenuItem::new(label).checked(choice == level).on_click(
                        move |_, _, cx| {
                            let _ = this.update(cx, |_, cx| {
                                cx.emit(ChannelDetailsEvent::SetNotifyLevel {
                                    channel: channel.clone(),
                                    level: choice,
                                })
                            });
                        },
                    ));
                }
                menu
            });
        h_flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px(px(PAD))
            .pb_2()
            .child(notify)
            .child(
                Button::new("details-huddle")
                    .outline()
                    .with_size(Size::Small)
                    .icon(IconName::Headphones)
                    .label("Huddle")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ChannelDetailsEvent::StartHuddle))),
            )
    }

    /// The About tab: Slack's cards.
    fn about_tab(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (border, muted_foreground, link_color, accent_color) = (
            theme.border,
            theme.muted_foreground,
            theme.link,
            theme.accent,
        );
        let conversation = self.conversation().cloned().unwrap_or_default();
        let mut cards = v_flex().w_full().px(px(PAD)).py_4().gap_2();

        // Channel name: editable on channels other than #general, with Slack's privacy note on private channels.
        let is_channel = !conversation.is_im && !conversation.is_mpim;
        let can_manage = is_channel && !conversation.is_general;
        let raw_name = self
            .local_name
            .clone()
            .or_else(|| conversation.name.clone())
            .unwrap_or_default();
        let editing_name = self
            .editing
            .as_ref()
            .filter(|e| e.field == EditField::Name)
            .map(|e| e.draft.clone());
        let name_card = if let Some(draft) = editing_name {
            card("Channel name", None, border)
                .child(Textarea::new(&draft))
                .child(
                    h_flex()
                        .flex_none()
                        .gap_2()
                        .child(
                            Button::new("details-save-Name")
                                .primary()
                                .with_size(Size::Small)
                                .label("Save")
                                .on_click(cx.listener(|this, _, _, cx| this.save_edit(cx))),
                        )
                        .child(
                            Button::new("details-cancel-Name")
                                .ghost()
                                .with_size(Size::Small)
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.editing = None;
                                    cx.notify();
                                })),
                        ),
                )
        } else {
            let edit_name = can_manage.then(|| {
                Button::new("details-edit-Name")
                    .ghost()
                    .with_size(Size::XSmall)
                    .label("Edit")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.edit(EditField::Name, window, cx)),
                    )
                    .into_any_element()
            });
            let mut c = card("Channel name", edit_name, border);
            if conversation.is_private {
                c = c.child(
                    div()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child("Only visible to your organization"),
                );
            }
            c.child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(
                        Icon::new(if conversation.is_private {
                            IconName::Lock
                        } else {
                            IconName::Hash
                        })
                        .size(px(14.)),
                    )
                    .child(raw_name),
            )
        };
        cards = cards.child(name_card);

        // Topic and description: the cards Slack lets a member edit.
        cards = cards.child(self.field_card(
            "Topic",
            EditField::Topic,
            self.topic(&conversation),
            "Add a topic",
            cx,
        ));
        cards = cards.child(self.field_card(
            "Description",
            EditField::Description,
            self.purpose(&conversation),
            "Add a description",
            cx,
        ));

        // Created by: the creator's name and the creation date.
        let created = match conversation.created {
            Some(secs) => format!("on {}", created_on(secs)),
            None => String::new(),
        };
        let created_by = conversation
            .creator
            .as_ref()
            .map(|creator| {
                format!("{} {}", self.fixtures.user_name(creator), created)
                    .trim()
                    .to_owned()
            })
            .unwrap_or_else(|| "Unknown member".to_owned());
        cards = cards.child(
            card("Created by", None, border).child(
                div()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child(created_by),
            ),
        );

        // Posting & management permissions card (`F-CORE-064`).
        let manager_names: Vec<String> = self
            .managers
            .iter()
            .filter_map(|id| self.fixtures.user(id))
            .map(|u| self.fixtures.name_of(u).to_owned())
            .collect();
        let posting_restricted = conversation
            .extra
            .get("properties")
            .and_then(|p| p.get("posting_restricted_to"))
            .is_some_and(|v| !v.is_null());
        let posting_policy = if conversation.is_archived {
            "Archived — nobody can post new messages."
        } else if posting_restricted {
            "Restricted — only Channel Managers & Admins can post."
        } else {
            "Everyone in this channel can post."
        };
        let managers_line = if manager_names.is_empty() {
            "Workspace Admins & Owners".to_owned()
        } else {
            manager_names.join(", ")
        };
        cards = cards.child(
            v_flex()
                .flex_none()
                .gap_1p5()
                .p_3()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Posting & management permissions"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child(posting_policy),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child(format!("Channel managers: {managers_line}")),
                ),
        );

        // Leave channel, Slack's red row at the end of the cards.
        let channel = self.channel.clone();
        cards = cards.child(
            h_flex()
                .id("details-leave")
                .flex_none()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .rounded_md()
                .cursor_pointer()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(crate::theme::danger_text(cx))
                .hover(|row| row.bg(crate::theme::danger_text(cx).alpha(0.12)))
                .child(Icon::new(IconName::LogOut).size(px(16.)))
                .child("Leave channel")
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(ChannelDetailsEvent::Leave(channel.clone()))
                })),
        );

        // Archive / Unarchive channel (on non-general channels).
        if can_manage {
            let archived = conversation.is_archived;
            let arch_channel = self.channel.clone();
            let danger = crate::theme::danger_text(cx);
            cards = cards.child(
                h_flex()
                    .id("details-archive")
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if archived { link_color } else { danger })
                    .hover(move |row| {
                        row.bg(if archived {
                            accent_color
                        } else {
                            danger.alpha(0.12)
                        })
                    })
                    .child(Icon::new(IconName::Archive).size(px(16.)))
                    .child(if archived {
                        "Unarchive channel"
                    } else {
                        "Archive channel"
                    })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        if archived {
                            cx.emit(ChannelDetailsEvent::Unarchive(arch_channel.clone()));
                        } else {
                            cx.emit(ChannelDetailsEvent::Archive(arch_channel.clone()));
                        }
                    })),
            );
        }

        // Organizations, on a Slack Connect channel: one row per organization (`rebuild`
        // builds the lines for shared channels only).
        if !self.orgs.is_empty() {
            let host = host_team(&conversation);
            let mut section = v_flex().flex_none().gap_1().mt_2().child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Organizations"),
            );
            // Slack lists the other organizations only, not the viewer's own.
            let own = own_team(&self.fixtures);
            for org in self.orgs.iter().filter(|org| org.team.id != own) {
                let is_host = Some(&org.team.id) == host.as_ref();
                section = section.child(org_row(
                    &org.team,
                    org.members,
                    is_host,
                    &conversation,
                    muted_foreground,
                ));
            }
            cards = cards.child(section);
        }
        div()
            .id("details-about")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(cards)
    }

    /// One of the editable cards (topic, description): the value, or Slack's "Add a …"
    /// placeholder, with Edit at the label's right; a draft box with Save / Cancel while
    /// editing.
    fn field_card(
        &self,
        label: &str,
        field: EditField,
        value: String,
        placeholder: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (border, muted_foreground, link) = (theme.border, theme.muted_foreground, theme.link);
        let draft = self
            .editing
            .as_ref()
            .filter(|editing| editing.field == field)
            .map(|editing| editing.draft.clone());
        if let Some(draft) = draft {
            return card(label, None, border)
                .child(Textarea::new(&draft))
                .child(
                    h_flex()
                        .flex_none()
                        .gap_2()
                        .child(
                            Button::new(SharedString::from(format!("details-save-{field:?}")))
                                .primary()
                                .with_size(Size::Small)
                                .label("Save")
                                .on_click(cx.listener(|this, _, _, cx| this.save_edit(cx))),
                        )
                        .child(
                            Button::new(SharedString::from(format!("details-cancel-{field:?}")))
                                .ghost()
                                .with_size(Size::Small)
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.editing = None;
                                    cx.notify();
                                })),
                        ),
                );
        }
        let edit = Button::new(SharedString::from(format!("details-edit-{field:?}")))
            .ghost()
            .with_size(Size::XSmall)
            .label("Edit")
            .on_click(cx.listener(move |this, _, window, cx| this.edit(field, window, cx)));
        let content = if value.is_empty() {
            let add_id = match field {
                EditField::Name => "details-add-name",
                EditField::Topic => "details-add-topic",
                EditField::Description => "details-add-description",
            };
            div()
                .id(add_id)
                .text_sm()
                .cursor_pointer()
                .text_color(link)
                .child(placeholder.to_owned())
                .on_click(cx.listener(move |this, _, window, cx| this.edit(field, window, cx)))
                .into_any_element()
        } else if field == EditField::Topic {
            // The topic is mrkdwn, as in the conversation header.
            div()
                .text_sm()
                .child(crate::messages::mrkdwn_preview(
                    &value,
                    &self.fixtures,
                    muted_foreground,
                    "details-topic".into(),
                    cx,
                ))
                .into_any_element()
        } else {
            div()
                .text_sm()
                .text_color(muted_foreground)
                .child(value)
                .into_any_element()
        };
        card(label, Some(edit.into_any_element()), border).child(content)
    }

    /// One member row: avatar, name, and Slack's External marker for people from another
    /// organization; a click opens their profile.
    /// Slack's member filter beside "Find people": a 36pt select whose menu lists each
    /// choice with its count at the right, the organizations under "People from".
    fn filter_select(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let choices = Arc::clone(&self.filter_choices);
        let current = self.member_filter.clone();
        let label = choices
            .iter()
            .find(|choice| choice.filter == current)
            .map_or_else(|| SharedString::from("All"), |choice| choice.label.clone());
        let muted = cx.theme().muted_foreground;
        Button::new("details-member-filter")
            .outline()
            .flex_none()
            .w(px(200.))
            .h(px(36.))
            .rounded(px(8.))
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                menu = menu.min_w(px(320.));
                let mut people_from = false;
                for choice in choices.iter() {
                    if matches!(choice.filter, MemberFilter::Team(_)) && !people_from {
                        people_from = true;
                        menu = menu.separator().label("People from");
                    }
                    let (this, filter) = (this.clone(), choice.filter.clone());
                    let (label, count) = (choice.label.clone(), choice.count.to_string());
                    let checked = filter == current;
                    menu = menu.item(
                        PopupMenuItem::element(move |_, _| {
                            h_flex()
                                .w_full()
                                .gap_6()
                                .justify_between()
                                .child(label.clone())
                                .child(div().text_color(muted).child(count.clone()))
                        })
                        .checked(checked)
                        .on_click(move |_, _, cx| {
                            let filter = filter.clone();
                            let _ = this.update(cx, |this, cx| {
                                this.member_filter = filter;
                                cx.notify();
                            });
                        }),
                    );
                }
                menu
            })
    }

    /// One member row, Slack's member list: the 36px avatar (its organization's badge when
    /// the person is external), the bold name and presence glyph, the display name with the
    /// status emoji, and the title as a second muted line. A click opens their profile.
    fn member_row(&self, ix: usize, row: &MemberRow, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (muted_foreground, foreground, hover, background, success) = (
            theme.muted_foreground,
            theme.foreground,
            theme.accent,
            theme.background,
            theme.success,
        );
        let open = row.id.clone();
        let presence = self.fixtures.presence(&row.id);
        let emoji = row
            .emoji
            .as_deref()
            .map(|code| crate::emoji::emoji_element(code, &self.fixtures, px(14.)));
        h_flex()
            .id(("details-member", ix))
            .flex_none()
            .items_center()
            .h(px(60.))
            .gap_3()
            .px(px(PAD))
            .cursor_pointer()
            .hover(|el| el.bg(hover))
            .child(with_external_badge(
                &self.fixtures,
                Some(&row.id),
                user_chip(&self.fixtures, &row.id, 36.),
                36.,
                background,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(foreground)
                                    .child(row.name.clone()),
                            )
                            .child(member_presence(
                                row.team.is_some(),
                                presence,
                                muted_foreground,
                                success,
                            ))
                            .children(row.handle.as_ref().map(|handle| {
                                div()
                                    .truncate()
                                    .text_size(px(15.))
                                    .text_color(foreground)
                                    .child(handle.clone())
                            }))
                            .children(emoji),
                    )
                    .children(row.job.as_ref().map(|job| {
                        div()
                            .truncate()
                            .text_size(px(15.))
                            .text_color(muted_foreground)
                            .child(job.clone())
                    })),
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(ChannelDetailsEvent::OpenProfile(open.clone()))
            }))
    }
}

/// Normalize user input into a valid Slack channel slug (lowercase letters, digits, hyphens/underscores, max 80 chars).
pub fn normalize_channel_slug(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in raw.trim().trim_start_matches('#').chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if (ch == '-' || ch.is_whitespace()) && !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
        if out.len() >= 80 {
            break;
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// A Slack details card: the bold label (with the card's action, Slack's "Edit", at its
/// right) over the content, on a rounded bordered surface.
fn card(label: &str, action: Option<AnyElement>, border: Hsla) -> Div {
    v_flex()
        .flex_none()
        .gap_1()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(border)
        .child(
            h_flex()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(label.to_owned()),
                )
                .children(action),
        )
}

/// One connected-organization row of the About tab's Organizations section: the org icon,
/// name, its member count here, and Slack's Channel owner / Private notes.
fn org_row(
    team: &Team,
    members: u32,
    is_host: bool,
    conversation: &Conversation,
    muted: Hsla,
) -> Div {
    let mut row = h_flex()
        .flex_none()
        .items_center()
        .gap_2()
        .px_1()
        .py_1p5()
        .child(org_chip(team, 20.))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(team.name.clone()),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child(format!("{members} members")),
        );
    if is_host {
        row = row.child(div().text_xs().text_color(muted).child("Channel owner"));
    }
    if conversation.is_private {
        row = row.child(div().text_xs().text_color(muted).child("Private"));
    }
    row
}

/// One organization's Slack Connect line: its member count and record.
pub(crate) struct OrgLine {
    pub team: Team,
    pub members: u32,
}

/// The signed-in workspace's own team id.
pub(crate) fn own_team(fixtures: &Fixtures) -> TeamId {
    fixtures
        .workspaces
        .first()
        .map(|team| team.id.clone())
        .unwrap_or_default()
}

/// Stored profiles by id, so member surfaces look each person up once instead of scanning
/// the directory per member.
fn profiles_by_id(users: &[User]) -> HashMap<&UserId, &User> {
    users.iter().map(|user| (&user.id, user)).collect()
}

/// Whether Slack's member list shows `member`: everyone except the app/bot and deactivated
/// members their stored profiles identify (an unknown member is a person whose profile has
/// not landed yet), and never `USLACKBOT`.
fn is_listed(member: &UserId, profiles: &HashMap<&UserId, &User>) -> bool {
    member.as_str() != "USLACKBOT"
        && profiles
            .get(member)
            .is_none_or(|user| !user.is_bot && !user.deleted)
}

/// `field` when it holds more than whitespace.
fn non_empty(field: &Option<String>) -> Option<String> {
    field
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// The profile's status emoji while it is current, trimmed of its colons (Slack hides
/// statuses past their expiration; `now` is unix seconds).
fn current_status_emoji(profile: &Profile, now: u64) -> Option<String> {
    if profile
        .status_expiration
        .is_some_and(|end| end != 0 && end <= now as i64)
    {
        return None;
    }
    non_empty(&profile.status_emoji).map(|code| code.trim_matches(':').to_owned())
}

/// Slack's presence glyph beside a member's name: a green dot when active, a hollow ring
/// when away, and a hollow rounded square for people from another organization.
fn member_presence(external: bool, presence: Option<Presence>, muted: Hsla, success: Hsla) -> Div {
    let glyph = div().flex_none().size(px(9.));
    if external {
        return glyph.rounded(px(2.5)).border_1().border_color(muted);
    }
    match presence {
        Some(Presence::Active) => glyph.rounded_full().bg(success),
        _ => glyph.rounded_full().border_1().border_color(muted),
    }
}

/// The Members tab's filter, Slack's select beside "Find people".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum MemberFilter {
    #[default]
    All,
    /// Everyone from another organization (offered once two or more are present).
    External,
    Managers,
    /// Full members of the workspace: no channel manager, guest or external person.
    Members,
    Guests,
    /// "People from": the signed-in workspace (`None`) or one external organization.
    Team(Option<TeamId>),
}

impl MemberFilter {
    fn keeps(&self, row: &MemberRow) -> bool {
        match self {
            Self::All => true,
            Self::External => row.team.is_some() && !row.guest,
            Self::Managers => row.manager,
            Self::Members => !row.manager && !row.guest && row.team.is_none(),
            Self::Guests => row.guest,
            Self::Team(team) => row.team == *team,
        }
    }
}

/// One choice of the member filter: its label and how many rows it keeps.
struct FilterChoice {
    label: SharedString,
    filter: MemberFilter,
    count: usize,
}

/// Slack's filter choices over `rows` (its account-type filter and "People from" group):
/// All; External organizations once two or more are present; Channel Managers, Members and
/// Guests; then, when external people are listed, the signed-in workspace and each
/// external organization in row order. Free-standing for the test.
fn filter_choices(fixtures: &Fixtures, rows: &[MemberRow]) -> Vec<FilterChoice> {
    // The organization options list only non-guest externals: a guest appears under
    // Guests alone, never under their organization or "External organizations".
    let mut externals: Vec<&TeamId> = Vec::new();
    for team in rows
        .iter()
        .filter(|row| !row.guest)
        .filter_map(|row| row.team.as_ref())
    {
        if !externals.contains(&team) {
            externals.push(team);
        }
    }
    let mut filters = vec![("All".to_owned(), MemberFilter::All)];
    if externals.len() > 1 {
        filters.push(("External organizations".to_owned(), MemberFilter::External));
    }
    filters.extend([
        ("Channel Managers".to_owned(), MemberFilter::Managers),
        ("Members".to_owned(), MemberFilter::Members),
        ("Guests".to_owned(), MemberFilter::Guests),
    ]);
    if !externals.is_empty() {
        let workspace = fixtures
            .workspaces
            .first()
            .map_or_else(|| "Your workspace".to_owned(), |team| team.name.to_string());
        filters.push((workspace, MemberFilter::Team(None)));
        // Every row's team came from `external_team`, whose record is in `teams`.
        for team in externals {
            if let Some(record) = fixtures.teams.iter().find(|record| &record.id == team) {
                filters.push((
                    record.name.to_string(),
                    MemberFilter::Team(Some(team.clone())),
                ));
            }
        }
    }
    filters
        .into_iter()
        .map(|(label, filter)| FilterChoice {
            count: rows.iter().filter(|row| filter.keeps(row)).count(),
            label: label.into(),
            filter,
        })
        .collect()
}

/// The organizations present in `members`: every team a resolved profile belongs to, plus the
/// own organization carrying the members without one (own-workspace people need no lookup).
/// The own organization comes first, the connected ones after it by name; an organization
/// whose record has not been fetched yet is left out.
pub(crate) fn orgs(
    own: &TeamId,
    members: &[UserId],
    users: &[User],
    teams: &[Team],
) -> Vec<OrgLine> {
    let profiles = profiles_by_id(users);
    // A channel spans a handful of organizations: a short list beats a map here.
    let mut counts: Vec<(&TeamId, u32)> = Vec::new();
    for member in members.iter().filter(|member| is_listed(member, &profiles)) {
        let team = profiles
            .get(member)
            .and_then(|user| user.team_id.as_ref())
            .unwrap_or(own);
        match counts.iter_mut().find(|(id, _)| *id == team) {
            Some(entry) => entry.1 += 1,
            None => counts.push((team, 1)),
        }
    }
    let mut lines: Vec<OrgLine> = counts
        .into_iter()
        .filter_map(|(id, members)| {
            let team = teams.iter().find(|team| team.id == *id).cloned()?;
            Some(OrgLine { team, members })
        })
        .collect();
    // The own organization first, then the connected ones by name; unknown orgs sort by id.
    lines.sort_by(|a, b| {
        let own = |line: &OrgLine| line.team.id == *own;
        own(b)
            .cmp(&own(a))
            .then_with(|| a.team.name.cmp(&b.team.name))
    });
    lines
}

/// The Slack Connect banner's lines: [`orgs`] minus the own organization.
pub(crate) fn external_orgs(
    own: &TeamId,
    members: &[UserId],
    users: &[User],
    teams: &[Team],
) -> Vec<OrgLine> {
    orgs(own, members, users, teams)
        .into_iter()
        .filter(|line| line.team.id != *own)
        .collect()
}

/// The team hosting the conversation: the host id the boot payloads carry (a Slack Connect
/// channel another organization hosts), else `context_team_id`, which names the viewing
/// workspace rather than the host.
pub(crate) fn host_team(conversation: &Conversation) -> Option<TeamId> {
    conversation
        .extra
        .get("conversation_host_id")
        .and_then(|host| host.as_str())
        .map(TeamId::from)
        .or_else(|| conversation.context_team_id.clone())
}

/// The smallest icon URL of a team record (`icon.image_34`, then the larger sizes).
pub(crate) fn team_icon_url(team: &Team) -> Option<SharedString> {
    let icon = team.icon.as_ref()?;
    [
        "image_34",
        "image_44",
        "image_68",
        "image_88",
        "image_132",
        "image_230",
    ]
    .into_iter()
    .find_map(|key| icon.get(key).and_then(|url| url.as_str()))
    .filter(|url| !url.is_empty())
    .map(SharedString::from)
}

/// An organization's avatar: its icon, else an initials chip in its identity color.
pub(crate) fn org_chip(team: &Team, size: f32) -> Div {
    match team_icon_url(team) {
        Some(url) => div()
            .size(px(size))
            .flex_none()
            .rounded(px(crate::avatars::avatar_radius(size)))
            .bg(crate::theme::avatar_color(team.id.as_str()))
            // GPUI clips square: the image rounds its own corners.
            .child(
                img(url)
                    .size_full()
                    .rounded(px(crate::avatars::avatar_radius(size))),
            ),
        None => {
            let initial = team
                .name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            div()
                .size(px(size))
                .flex_none()
                .rounded(px((size / 5.).max(3.)))
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .bg(crate::theme::avatar_color(team.id.as_str()))
                .text_color(crate::theme::avatar_foreground())
                .child(initial)
        }
    }
}

/// `June 25, 2020` for a channel's `created` unix seconds: Slack's "Created by" date, in UTC
/// (messages.rs keeps the local-clock machinery for the transcript; this one only ever names
/// a day of a stored creation date).
pub(crate) fn created_on(secs: i64) -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    format!("{} {}, {}", MONTHS[(month - 1) as usize], day, year)
}

/// (year, month, day) from days since the Unix epoch, proleptic Gregorian (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_shift = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_shift + 2) / 5 + 1) as u32;
    let month = if month_shift < 10 {
        month_shift + 3
    } else {
        month_shift - 9
    } as u32;
    ((year + i64::from(month <= 2)) as i32, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn team(id: &str, name: &str) -> Team {
        Team {
            id: TeamId::from(id),
            name: name.to_owned(),
            domain: name.to_lowercase(),
            ..Default::default()
        }
    }

    fn user(id: &str, team: Option<&str>) -> User {
        User {
            id: UserId::from(id),
            team_id: team.map(TeamId::from),
            ..Default::default()
        }
    }

    #[test]
    fn orgs_count_members_per_team_with_own_first() {
        let own = TeamId::from("T0ACME");
        let members = ["U1", "U2", "UE1", "UE2", "UE3", "UU"]
            .map(UserId::from)
            .to_vec();
        let users = vec![
            user("U1", Some("T0ACME")),
            user("U2", Some("T0ACME")),
            user("UE1", Some("T0ZETA")),
            user("UE2", Some("T0KLARNA")),
            user("UE3", Some("T0ZETA")),
            // UU has no profile: an own-workspace person waiting on `users.info`.
        ];
        let teams = vec![
            team("T0KLARNA", "Klarna"),
            team("T0ZETA", "Zeta"),
            team("T0ACME", "Acme"),
        ];

        let lines = orgs(&own, &members, &users, &teams);
        let summary: Vec<(&str, u32)> = lines
            .iter()
            .map(|line| (line.team.name.as_str(), line.members))
            .collect();
        assert_eq!(
            summary,
            [("Acme", 3), ("Klarna", 1), ("Zeta", 2)],
            "own org first (with the unprofiled member), connected by name"
        );
    }

    #[test]
    fn orgs_skip_bots_and_members_of_unknown_teams() {
        let own = TeamId::from("T0ACME");
        let members = ["U1", "UBOT", "UE1"].map(UserId::from).to_vec();
        let users = vec![
            user("U1", Some("T0ACME")),
            User {
                id: UserId::from("UBOT"),
                is_bot: true,
                ..Default::default()
            },
            user("UE1", Some("T0GHOST")),
        ];
        let teams = vec![team("T0ACME", "Acme")];

        let lines = orgs(&own, &members, &users, &teams);
        assert_eq!(lines.len(), 1, "the bot never lists, T0GHOST has no record");
        assert_eq!((lines[0].team.name.as_str(), lines[0].members), ("Acme", 1));
    }

    #[test]
    fn listed_members_keep_unknowns_and_drop_bots() {
        let members = ["U1", "UBOT", "UGONE", "UU", "USLACKBOT"]
            .map(UserId::from)
            .to_vec();
        let users = vec![
            user("U1", Some("T1")),
            User {
                id: UserId::from("UBOT"),
                is_bot: true,
                ..Default::default()
            },
            User {
                id: UserId::from("UGONE"),
                deleted: true,
                ..Default::default()
            },
        ];
        let profiles = profiles_by_id(&users);
        let listed: Vec<&UserId> = members
            .iter()
            .filter(|member| is_listed(member, &profiles))
            .collect();
        assert_eq!(listed, [&UserId::from("U1"), &UserId::from("UU")]);
    }

    #[test]
    fn created_on_formats_slacks_date() {
        assert_eq!(created_on(1_593_043_200), "June 25, 2020");
        assert_eq!(created_on(0), "January 1, 1970");
        assert_eq!(created_on(1_633_305_600), "October 4, 2021");
    }

    #[test]
    fn team_icon_url_picks_the_smallest_size() {
        let mut record = team("T1", "Acme");
        record.icon = Some(crate::blockkit::serde_json::json!({
            "image_default": true,
            "image_34": "https://img.example/34.png",
            "image_132": "https://img.example/132.png"
        }));
        assert_eq!(
            team_icon_url(&record).as_deref(),
            Some("https://img.example/34.png")
        );
        let bare = team("T2", "Bare");
        assert_eq!(team_icon_url(&bare), None, "no icon object, no url");
    }

    #[test]
    fn filter_choices_follow_slacks_order_and_counts() {
        let mut fixtures = crate::fixtures::load();
        fixtures.workspaces = vec![team("T0ACME", "Acme Corp")];
        fixtures.teams = vec![team("T0KLARNA", "Klarna"), team("T0ZETA", "Zeta")];
        let row = |team: Option<&str>, manager: bool, guest: bool| MemberRow {
            id: UserId::from("U1"),
            name: "A Person".to_owned(),
            key: "a person".to_owned(),
            team: team.map(TeamId::from),
            handle: None,
            job: None,
            emoji: None,
            manager,
            guest,
        };
        let rows = vec![
            row(None, true, false),
            row(None, false, false),
            row(None, false, true),
            row(Some("T0ZETA"), false, false),
            row(Some("T0KLARNA"), false, false),
            row(Some("T0ZETA"), false, false),
            row(Some("T0KLARNA"), false, true),
        ];
        let choices = filter_choices(&fixtures, &rows);
        let got: Vec<(&str, usize)> = choices
            .iter()
            .map(|choice| (choice.label.as_ref(), choice.count))
            .collect();
        assert_eq!(
            got,
            [
                ("All", 7),
                ("External organizations", 3),
                ("Channel Managers", 1),
                ("Members", 1),
                ("Guests", 2),
                ("Acme Corp", 3),
                ("Zeta", 2),
                ("Klarna", 2),
            ]
        );
        // An internal channel: neither the external choice nor the "People from" group.
        let internal = filter_choices(&fixtures, &rows[..3]);
        let labels: Vec<&str> = internal
            .iter()
            .map(|choice| choice.label.as_ref())
            .collect();
        assert_eq!(labels, ["All", "Channel Managers", "Members", "Guests"]);
    }
}
