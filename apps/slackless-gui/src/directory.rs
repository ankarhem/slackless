//! The workspace directory (the sidebar's "Directories" row): Slack's People, Channels, and
//! User groups tabs with a search over each. A person opens their profile panel; a channel opens,
//! or offers the channel browser's local Join for one not joined yet; user groups support
//! browsing, searching, viewing/editing members, editing details, and disabling/enabling. The
//! filters and rows live in [`crate::directory_rows`].
//!
//! `DirectoryView::new(Arc<Fixtures>, &mut Window, &mut App) -> Entity<DirectoryView>`
//! `DirectoryView::set_fixtures(&mut self, Arc<Fixtures>, &mut Context<Self>)` (live reloads)
//! emits `DirectoryEvent::OpenProfile(UserId)` / `Open(ChannelId)` / `Join(ChannelId)` /
//! `CreateUsergroup` / `Close`, and dispatches user-group management actions (`Members`, `Edit`,
//! `Disable`, `Enable`) through [`dispatch_usergroup_action`].

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use slackless_core::types::{ChannelId, Conversation, UserId, Usergroup, UsergroupId};

use crate::directory_rows::{
    channel_row, filter_channels, people, person_row, usergroup_row, usergroups,
};
use crate::fixtures::Fixtures;

/// Rows listed per tab at once.
const MAX_ROWS: usize = 30;

#[derive(Clone, Copy, PartialEq, Eq)]
enum DirectoryTab {
    People,
    Channels,
    UserGroups,
}

pub enum DirectoryEvent {
    /// A person row: open their profile panel.
    OpenProfile(UserId),
    /// A joined channel row: open it.
    Open(ChannelId),
    /// An unjoined channel's Join (already marked joined here).
    Join(ChannelId),
    /// Create a new `@handle` user group (`usergroups.create`, `F-COLLAB-1161`, `1162`).
    CreateUsergroup {
        name: String,
        handle: String,
        description: String,
    },
    Close,
}

/// User-group management actions triggered from the Directory's User groups tab.
#[derive(Clone, Debug)]
pub enum UsergroupAction {
    /// Open the member viewer/editor modal (`usergroups.users.list` / `usergroups.users.update`).
    InspectMembers(Usergroup),
    /// Open the edit details modal (`usergroups.update`).
    Edit(Usergroup),
    /// Disable an active user group (`usergroups.disable`).
    Disable(Usergroup),
    /// Re-enable a disabled user group (`usergroups.enable`).
    Enable(Usergroup),
}

type UsergroupActionCallback = Rc<dyn Fn(UsergroupAction, &mut Window, &mut App)>;

struct UsergroupActionRouter(UsergroupActionCallback);

impl Global for UsergroupActionRouter {}

/// Register the root [`App`] handler for user-group management actions (`Members`, `Edit`,
/// `Disable`, `Enable`).
pub(crate) fn set_usergroup_action_handler(
    handler: impl Fn(UsergroupAction, &mut Window, &mut App) + 'static,
    cx: &mut App,
) {
    cx.set_global(UsergroupActionRouter(Rc::new(handler)));
}

/// Dispatch a user-group management action to the root [`App`] handler.
pub(crate) fn dispatch_usergroup_action(
    action: UsergroupAction,
    window: &mut Window,
    cx: &mut App,
) {
    let handler = cx
        .try_global::<UsergroupActionRouter>()
        .map(|router| Rc::clone(&router.0));
    if let Some(handler) = handler {
        handler(action, window, cx);
    }
}

/// Optimistic local overrides for user-group detail edits until the next `Event::Usergroups`
/// refresh arrives from Slack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UsergroupLocalEdit {
    pub name: String,
    pub handle: String,
    pub description: Option<String>,
}

/// Live client-side user-group membership cache and session disable/edit state shared between the
/// Directory view and user-group modals.
#[derive(Default)]
struct UsergroupLiveState {
    /// Fresh member lists returned by `usergroups.users.list` / `usergroups.users.update`.
    members: HashMap<UsergroupId, Vec<UserId>>,
    /// Groups disabled during this session (kept visible under "Show disabled" so they can be
    /// re-enabled even when `usergroups.list` omits disabled groups).
    disabled_groups: HashMap<UsergroupId, Usergroup>,
    /// Optimistic detail edits applied locally before the next `usergroups.list` round-trip.
    edited_groups: HashMap<UsergroupId, UsergroupLocalEdit>,
}

impl Global for UsergroupLiveState {}

fn ensure_live_state(cx: &mut App) -> &mut UsergroupLiveState {
    if !cx.has_global::<UsergroupLiveState>() {
        cx.set_global(UsergroupLiveState::default());
    }
    cx.global_mut::<UsergroupLiveState>()
}

/// Record a fresh member list for `id` (from `Event::UsergroupUsersLoaded` or local save).
pub(crate) fn record_usergroup_members(id: UsergroupId, users: Vec<UserId>, cx: &mut App) {
    ensure_live_state(cx).members.insert(id, users);
}

/// Return the cached fresh member list for `id`, if already loaded this session.
pub(crate) fn cached_usergroup_members(id: &UsergroupId, cx: &App) -> Option<Vec<UserId>> {
    cx.try_global::<UsergroupLiveState>()
        .and_then(|s| s.members.get(id).cloned())
}

/// Record a user group as disabled locally so it immediately appears under "Show disabled" with an
/// Enable action.
pub(crate) fn record_usergroup_disabled(mut group: Usergroup, cx: &mut App) {
    if group.date_delete == 0 {
        group.date_delete = 1;
    }
    ensure_live_state(cx)
        .disabled_groups
        .insert(group.id.clone(), group);
}

/// Clear any local disabled override when a group is re-enabled.
pub(crate) fn record_usergroup_enabled(id: &UsergroupId, cx: &mut App) {
    if cx.has_global::<UsergroupLiveState>() {
        cx.global_mut::<UsergroupLiveState>()
            .disabled_groups
            .remove(id);
    }
}

/// Record an optimistic detail edit (`name`, `handle`, `description`) for instant feedback.
pub(crate) fn record_usergroup_edit(
    id: UsergroupId,
    name: String,
    handle: String,
    description: Option<String>,
    cx: &mut App,
) {
    let state = ensure_live_state(cx);
    if let Some(disabled) = state.disabled_groups.get_mut(&id) {
        disabled.name = name.clone();
        if !handle.is_empty() {
            disabled.handle = handle.clone();
        }
        disabled.description = description.clone();
    }
    state.edited_groups.insert(
        id,
        UsergroupLocalEdit {
            name,
            handle,
            description,
        },
    );
}

/// Reconcile local optimistic overrides after a fresh `Event::Usergroups` list lands from Slack.
pub(crate) fn reconcile_usergroups_from_server(groups: &[Usergroup], cx: &mut App) {
    if cx.has_global::<UsergroupLiveState>() {
        let state = cx.global_mut::<UsergroupLiveState>();
        for group in groups {
            if !group.is_deleted() {
                state.disabled_groups.remove(&group.id);
            }
            state.edited_groups.remove(&group.id);
        }
    }
}

/// The active tab's matches, computed when the tab, the query, the "Show disabled" toggle,
/// the workspace data or the session's user-group overrides change (a counts-only snapshot
/// shares the data, so it keeps the matches); rows render from it each frame.
struct DirectoryMemo {
    tab: DirectoryTab,
    needle: String,
    show_disabled: bool,
    fixtures: Arc<Fixtures>,
    /// The user-group overrides the list was computed with.
    overrides: Option<(
        HashMap<UsergroupId, Usergroup>,
        HashMap<UsergroupId, UsergroupLocalEdit>,
    )>,
    list: TabList,
    /// Disabled groups the User groups tab's query matched (its summary line).
    disabled_count: usize,
}

enum TabList {
    People(Vec<UserId>),
    Channels(Vec<Conversation>),
    UserGroups(Vec<Usergroup>),
}

fn live_overrides(
    cx: &App,
) -> Option<(
    HashMap<UsergroupId, Usergroup>,
    HashMap<UsergroupId, UsergroupLocalEdit>,
)> {
    cx.try_global::<UsergroupLiveState>()
        .map(|s| (s.disabled_groups.clone(), s.edited_groups.clone()))
}

pub struct DirectoryView {
    fixtures: Arc<Fixtures>,
    tab: DirectoryTab,
    filter: Entity<TextareaState>,
    /// Channels joined from this directory during the session.
    joined: HashSet<ChannelId>,
    /// Expanded user group id showing clickable member and default channel chips.
    expanded_group: Option<String>,
    /// Whether disabled user groups are shown in the User groups tab.
    show_disabled_groups: bool,
    /// The active tab's matches (see [`DirectoryMemo`]).
    memo: Option<DirectoryMemo>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DirectoryEvent> for DirectoryView {}

impl DirectoryView {
    pub fn new(
        fixtures: Arc<Fixtures>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<DirectoryView> {
        cx.new(|cx| {
            let filter = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Search the directory")
                    .auto_grow(1, 1)
            });
            let subscriptions = vec![
                cx.subscribe(&filter, |_: &mut DirectoryView, _, _: &InputEvent, cx| {
                    cx.notify()
                }),
            ];
            DirectoryView {
                fixtures,
                tab: DirectoryTab::People,
                filter,
                joined: HashSet::new(),
                expanded_group: None,
                show_disabled_groups: false,
                memo: None,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Live refresh: search the directory in `fixtures`, keeping the typed filter.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        cx.notify();
    }

    /// The active tab's matches for the current query, recomputed only when an input changed.
    fn refresh_memo(&mut self, cx: &Context<Self>) {
        let needle = self.needle(cx);
        let overrides = live_overrides(cx);
        let up_to_date = self.memo.as_ref().is_some_and(|memo| {
            memo.tab == self.tab
                && memo.needle == needle
                && memo.show_disabled == self.show_disabled_groups
                && memo.fixtures.shares_data(&self.fixtures)
                && overrides
                    .as_ref()
                    .map(|(disabled, edited)| (disabled, edited))
                    == memo
                        .overrides
                        .as_ref()
                        .map(|(disabled, edited)| (disabled, edited))
        });
        if !up_to_date {
            let (disabled, edited) = overrides
                .as_ref()
                .map_or((HashMap::new(), HashMap::new()), |(disabled, edited)| {
                    (disabled.clone(), edited.clone())
                });
            let fixtures = self.fixtures.clone();
            let (list, disabled_count) = match self.tab {
                DirectoryTab::People => (TabList::People(people(&fixtures, &needle)), 0),
                DirectoryTab::Channels => {
                    (TabList::Channels(filter_channels(&fixtures, &needle)), 0)
                }
                DirectoryTab::UserGroups => {
                    let (groups, disabled_count) = usergroups(
                        &fixtures,
                        &disabled,
                        &edited,
                        &needle,
                        self.show_disabled_groups,
                    );
                    (TabList::UserGroups(groups), disabled_count)
                }
            };
            self.memo = Some(DirectoryMemo {
                tab: self.tab,
                needle,
                show_disabled: self.show_disabled_groups,
                fixtures,
                overrides,
                list,
                disabled_count,
            });
        }
    }

    /// The snapshot the rows read.
    pub(super) fn fixtures(&self) -> &Arc<Fixtures> {
        &self.fixtures
    }

    fn needle(&self, cx: &Context<Self>) -> String {
        self.filter.read(cx).value().trim().to_lowercase()
    }

    /// Mark `channel` joined here and hand the join to the root.
    pub(super) fn join(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        self.joined.insert(channel.clone());
        cx.emit(DirectoryEvent::Join(channel));
        cx.notify();
    }

    pub(super) fn is_group_expanded(&self, id: &str) -> bool {
        self.expanded_group.as_deref() == Some(id)
    }

    pub(super) fn toggle_group(&mut self, id: String, cx: &mut Context<Self>) {
        if self.expanded_group.as_deref() == Some(id.as_str()) {
            self.expanded_group = None;
        } else {
            self.expanded_group = Some(id);
        }
        cx.notify();
    }

    fn toggle_show_disabled_groups(&mut self, cx: &mut Context<Self>) {
        self.show_disabled_groups = !self.show_disabled_groups;
        cx.notify();
    }
}

impl Render for DirectoryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, foreground, link, secondary, secondary_hover) = (
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.link,
            theme.secondary,
            theme.secondary_hover,
        );
        self.refresh_memo(cx);
        let (tab, joined) = (self.tab, &self.joined);
        let DirectoryMemo {
            list,
            disabled_count,
            show_disabled,
            ..
        } = self
            .memo
            .as_ref()
            .expect("refresh_memo computed the matches");
        let show_disabled = *show_disabled;
        let disabled_count = *disabled_count;
        let count = match list {
            TabList::People(people) => people.len(),
            TabList::Channels(channels) => channels.len(),
            TabList::UserGroups(groups) => groups.len(),
        };
        let rows: Vec<_> = match list {
            TabList::People(people) => people
                .iter()
                .take(MAX_ROWS)
                .enumerate()
                .map(|(ix, user)| person_row(self, ix, user, cx))
                .collect(),
            TabList::Channels(channels) => channels
                .iter()
                .take(MAX_ROWS)
                .enumerate()
                .map(|(ix, channel)| channel_row(ix, channel, joined, cx))
                .collect(),
            TabList::UserGroups(groups) => groups
                .iter()
                .take(MAX_ROWS)
                .enumerate()
                .map(|(ix, group)| usergroup_row(self, ix, group, cx))
                .collect(),
        };
        let tab_row = [
            (DirectoryTab::People, "People", "directory-tab-People"),
            (DirectoryTab::Channels, "Channels", "directory-tab-Channels"),
            (
                DirectoryTab::UserGroups,
                "User groups",
                "directory-tab-UserGroups",
            ),
        ]
        .map(|(tab, label, id)| {
            let active = tab == self.tab;
            div()
                .id(SharedString::from(id))
                .h_full()
                .flex()
                .items_center()
                .px_1()
                .cursor_pointer()
                .text_sm()
                .text_color(if active { foreground } else { muted })
                .when(active, |tab| {
                    tab.font_weight(FontWeight::SEMIBOLD)
                        .border_b_2()
                        .border_color(link)
                })
                .hover(|el| el.text_color(foreground))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = tab;
                    cx.notify();
                }))
        });

        let summary_label = if tab == DirectoryTab::UserGroups && disabled_count > 0 {
            if show_disabled {
                format!("{count} groups ({disabled_count} disabled)")
            } else {
                format!("{count} active · {disabled_count} disabled hidden")
            }
        } else {
            format!("{count} results")
        };

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .h(px(46.))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .font_weight(FontWeight::BOLD)
                            .child(
                                Icon::new(IconName::BookUser)
                                    .text_color(foreground)
                                    .size(px(16.)),
                            )
                            .child("Directory"),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .when(tab == DirectoryTab::UserGroups, |row| {
                                let this_ug = cx.weak_entity();
                                row.child(
                                    Button::new("directory-new-usergroup")
                                        .primary()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Plus)
                                        .label("New user group")
                                        .on_click(move |_, window, cx| {
                                            open_new_usergroup_dialog(this_ug.clone(), window, cx);
                                        }),
                                )
                            })
                            .child(
                                Button::new("directory-close")
                                    .ghost()
                                    .with_size(Size::Small)
                                    .icon(IconName::X)
                                    .tooltip("Close")
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(DirectoryEvent::Close)),
                                    ),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .items_stretch()
                    .flex_none()
                    .px_4()
                    .gap_5()
                    .h(px(38.))
                    .border_b_1()
                    .border_color(border)
                    .children(tab_row),
            )
            .child(
                crate::preferences::search_input_box(&self.filter, cx)
                    .flex_none()
                    .mx_4()
                    .my_2(),
            )
            .child(
                h_flex()
                    .px_4()
                    .pb_1()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .child(div().text_xs().text_color(muted).child(summary_label))
                    .when(
                        tab == DirectoryTab::UserGroups && (disabled_count > 0 || show_disabled),
                        |bar| {
                            bar.child(
                                h_flex()
                                    .id("directory-toggle-disabled-groups")
                                    .items_center()
                                    .gap_1()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .text_xs()
                                    .bg(if show_disabled {
                                        link.opacity(0.16)
                                    } else {
                                        secondary
                                    })
                                    .text_color(if show_disabled { link } else { muted })
                                    .hover(move |el| {
                                        el.bg(if show_disabled {
                                            link.opacity(0.24)
                                        } else {
                                            secondary_hover
                                        })
                                    })
                                    .child(if show_disabled {
                                        "Hide disabled".to_owned()
                                    } else {
                                        format!("Show disabled ({disabled_count})")
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_show_disabled_groups(cx);
                                    })),
                            )
                        },
                    ),
            )
            .child(
                v_flex()
                    .id("directory-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .when(count == 0, |list| {
                        list.child(
                            div()
                                .p_6()
                                .text_sm()
                                .text_color(muted)
                                .child("Nothing matches your search."),
                        )
                    }),
            )
    }
}

fn open_new_usergroup_dialog(
    view: gpui_kit::gpui::WeakEntity<DirectoryView>,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::gpui::Focusable as _;
    let name_in = cx.new(|cx| {
        TextareaState::new(window, cx)
            .placeholder("Group name (e.g. Checkout Engineers)")
            .auto_grow(1, 1)
    });
    let handle_in = cx.new(|cx| {
        TextareaState::new(window, cx)
            .placeholder("Handle without @ (e.g. checkout-eng)")
            .auto_grow(1, 1)
    });
    let desc_in = cx.new(|cx| {
        TextareaState::new(window, cx)
            .placeholder("Purpose / description (optional)")
            .auto_grow(1, 2)
    });
    name_in.read(cx).focus_handle(cx).focus(window, cx);
    window.open_dialog(cx, move |dialog, _, cx| {
        let theme = cx.theme();
        let (muted, bg) = (theme.muted_foreground, theme.background);
        let (n_c, h_c, d_c, v_c) = (
            name_in.clone(),
            handle_in.clone(),
            desc_in.clone(),
            view.clone(),
        );
        let field = |label: &'static str, st: &Entity<TextareaState>, multi_line: bool| {
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(muted)
                        .child(label),
                )
                .child(crate::preferences::text_input_box(st, multi_line, cx).bg(bg))
        };
        dialog.title("Create user group").w(px(420.)).child(
            v_flex()
                .gap_3()
                .p_4()
                .child(field("Name", &name_in, false))
                .child(field("Handle (@handle)", &handle_in, false))
                .child(field("Description", &desc_in, true))
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            crate::preferences::form_button(
                                Button::new("ug-create-cancel").outline().label("Cancel"),
                                cx,
                            )
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            crate::preferences::form_button(
                                Button::new("ug-create-submit")
                                    .primary()
                                    .label("Create group"),
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                let name = n_c.read(cx).value().trim().to_owned();
                                let handle = h_c
                                    .read(cx)
                                    .value()
                                    .trim()
                                    .trim_start_matches('@')
                                    .to_owned();
                                let description = d_c.read(cx).value().trim().to_owned();
                                if name.is_empty() {
                                    return;
                                }
                                window.close_dialog(cx);
                                let _ = v_c.update(cx, |_, cx| {
                                    cx.emit(DirectoryEvent::CreateUsergroup {
                                        name,
                                        handle,
                                        description,
                                    });
                                });
                            }),
                        ),
                ),
        )
    });
}
