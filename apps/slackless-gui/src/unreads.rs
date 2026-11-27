//! The All Unreads view (Slack's `p-unreads_view`, Cmd+Shift+A and the Home sidebar's
//! "Unreads" row): every conversation with unread messages as one collapsible card - the
//! conversation header with its mention pill and unread count, then the unread messages
//! themselves, oldest first, rendered like any transcript row with working links
//! (@mentions and their hover cards, #channels, URLs). "Mark as Read" moves Slack's read
//! cursor to the group's newest message through the root's existing mark path; "Mark All
//! Messages Read" routes through the root too, so the user's confirmation preference
//! holds.
//!
//! Data: the sidebar's own unread order picks the channels; each group derives from the
//! bundled workspace or the live snapshot ([`local_groups`]), and live mode re-reads the
//! local store off the UI thread ([`store_groups`]) with `client.counts`' read cursors. A
//! channel Slack counts unread whose stored rows lag shows a placeholder line instead -
//! opening the conversation loads them, as in Slack. Muted conversations stay out unless
//! they mention the signed-in user.
//!
//! `UnreadsView::new(Arc<Fixtures>, &mut App) -> Entity<UnreadsView>`
//! `UnreadsView::show_local / show_store / set_fixtures`
//! emits `UnreadsEvent::OpenConversation(ChannelId)` / `OpenLink(LinkTarget)`
//! / `MarkRead { channel, ts }` / `MarkAllRead`

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, AnyElement, App, AppContext as _, Context, Entity, EventEmitter, Focusable as _,
    FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::NotifyLevel;
use slackless_core::store::Store;
use slackless_core::types::{ChannelId, ConversationKind, Message, TeamId, Ts};

use crate::actions::{self, ActionHandler, ActionSet, MenuOrigin, MessageAction};
use crate::avatars::conversation_avatar;
use crate::fixtures::Fixtures;
use crate::messages::{
    LinkHandler, LinkTarget, day_label, message_row_linked, reply_bar, same_group,
};
use crate::sidebar::mentions_user;

/// Channel-level rows scanned per conversation in live mode; beyond it the conversation is
/// read where it stands.
const STORE_SCAN_LIMIT: usize = 100;

/// One conversation's unread messages, as one card.
#[derive(Clone)]
pub struct UnreadGroup {
    pub channel: ChannelId,
    /// The unread messages, oldest first: reading top to bottom is reading them in order.
    pub messages: Vec<Message>,
    /// Where "Mark as Read" moves the read cursor: the newest unread timestamp. Falls back
    /// to `client.counts`' `latest` while neither the snapshot nor the store has that
    /// message, and never sits behind it (a stale row would leave Slack's own cursor short
    /// of the real newest message).
    pub newest_ts: Ts,
    pub mention_count: u32,
}

/// Whether a message renders in the view at all: metadata rows never do, and nothing at or
/// behind the read cursor does.
fn unread_row(message: &Message, last_read: Option<&Ts>) -> bool {
    !message.hidden && !message.is_ephemeral && last_read.is_none_or(|read| message.ts > *read)
}

/// The unread groups of `order` (the sidebar's unread order) from the bundled workspace or
/// the live snapshot. One's own messages are read by definition, so they never group. A
/// conversation Slack counts unread whose rows lag (live snapshots keep channel histories
/// out) still groups, its newest timestamp from `client.counts`; a muted conversation only
/// groups while it mentions the signed-in user, as Slack lists it.
pub fn local_groups(fixtures: &Fixtures, order: &[ChannelId]) -> Vec<UnreadGroup> {
    let mut groups = Vec::new();
    for channel in order {
        let Some(conversation) = fixtures.conversation(channel) else {
            continue;
        };
        let counts = fixtures.counts.get(channel);
        let last_read = counts
            .and_then(|counts| counts.last_read.as_ref())
            .or(conversation.last_read.as_ref());
        let messages: Vec<Message> = fixtures
            .messages
            .get(channel)
            .into_iter()
            .flatten()
            .filter(|message| {
                unread_row(message, last_read)
                    && !message
                        .user
                        .as_ref()
                        .is_some_and(|user| *user == fixtures.self_user)
            })
            .cloned()
            .collect();
        let mention_count = counts.map_or_else(
            || {
                messages
                    .iter()
                    .filter(|message| mentions_user(message, &fixtures.self_user))
                    .count() as u32
            },
            |counts| counts.effective_mentions(),
        );
        if fixtures.notify_levels.get(channel) == Some(&NotifyLevel::Muted) && mention_count == 0 {
            continue;
        }
        let counted = counts.is_some_and(slackless_core::api::ConversationCounts::is_unread);
        let Some(newest_ts) = messages
            .last()
            .map(|message| message.ts.clone())
            .or_else(|| counts.and_then(|counts| counts.latest.clone()))
        else {
            continue;
        };
        if !messages.is_empty() || counted {
            groups.push(UnreadGroup {
                channel: channel.clone(),
                messages,
                newest_ts,
                mention_count,
            });
        }
    }
    groups
}

/// The live store's unread groups for `targets`: each channel Slack counts unread with its
/// read cursor, its newest message (`client.counts`) and its mention count. A channel whose
/// stored rows lag behind `latest` still groups - its card asks for the conversation to be
/// opened - and the read cursor still moves to `latest`.
pub fn store_groups(
    store: &Store,
    team: &TeamId,
    targets: &[(ChannelId, Option<Ts>, Option<Ts>, u32)],
) -> Vec<UnreadGroup> {
    let mut groups = Vec::new();
    for (channel, last_read, latest, mention_count) in targets {
        let Ok(rows) = store.channel_history(team, channel, None, STORE_SCAN_LIMIT) else {
            continue;
        };
        let messages: Vec<Message> = rows
            .iter()
            .filter(|row| !row.deleted && unread_row(&row.message, last_read.as_ref()))
            .map(|row| row.message.clone())
            .collect();
        // `latest` at or behind the cursor means Slack itself counts nothing unread there.
        let unread_latest = match (latest, last_read) {
            (Some(latest), Some(read)) => latest > read,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if messages.is_empty() && !unread_latest {
            continue;
        }
        let Some(newest_ts) = messages
            .last()
            .map(|message| message.ts.clone())
            .into_iter()
            .chain(latest.clone())
            .max()
        else {
            continue;
        };
        groups.push(UnreadGroup {
            channel: channel.clone(),
            messages,
            newest_ts,
            mention_count: *mention_count,
        });
    }
    groups
}

/// The store targets for the channels `groups` names: each one's read cursor, newest
/// message and mention count, as the snapshot's `client.counts` copy reported them.
pub fn store_targets(
    fixtures: &Fixtures,
    groups: &[UnreadGroup],
) -> Vec<(ChannelId, Option<Ts>, Option<Ts>, u32)> {
    groups
        .iter()
        .map(|group| {
            let counts = fixtures.counts.get(&group.channel);
            let last_read = counts
                .and_then(|counts| counts.last_read.clone())
                .or_else(|| {
                    fixtures
                        .conversation(&group.channel)
                        .and_then(|conversation| conversation.last_read.clone())
                });
            (
                group.channel.clone(),
                last_read,
                counts.and_then(|counts| counts.latest.clone()),
                group.mention_count,
            )
        })
        .collect()
}

/// Single-flight bookkeeping for the view's live store reads ([`store_groups`]): one read in
/// flight at a time, the refreshes asked for meanwhile folded into one more read after it, and
/// a read a newer refresh overtook (another workspace's, or a derivation shown without the
/// store since) dropped when it lands.
#[derive(Default)]
pub struct LiveRefresh {
    /// The newest refresh started; a read of an older generation was overtaken.
    generation: u64,
    /// The workspace whose read is in flight.
    in_flight: Option<TeamId>,
    /// Refreshes were asked for while that read was in flight.
    pending: bool,
}

impl LiveRefresh {
    /// Ask for a store read of `team`'s unreads: the generation to start it under, or `None`
    /// while that workspace's read is in flight (one more read follows it). A read in flight
    /// for another workspace is overtaken.
    pub fn request(&mut self, team: &TeamId) -> Option<u64> {
        if self.in_flight.as_ref() == Some(team) {
            self.pending = true;
            return None;
        }
        self.generation += 1;
        self.in_flight = Some(team.clone());
        self.pending = false;
        Some(self.generation)
    }

    /// The view was just derived without the store (the bundled workspace's groups): a read
    /// still in flight must not land over it.
    pub fn overtake(&mut self) {
        self.generation += 1;
        self.in_flight = None;
        self.pending = false;
    }

    /// The read started under `generation` finished: `None` when a newer refresh overtook it
    /// (its result is dropped), else whether refreshes asked for meanwhile need one more read.
    pub fn land(&mut self, generation: u64) -> Option<bool> {
        if generation != self.generation {
            return None;
        }
        self.in_flight = None;
        Some(std::mem::take(&mut self.pending))
    }
}

/// What the All Unreads view asks the root for.
pub enum UnreadsEvent {
    /// A card's conversation (its name, or the placeholder of a channel without rows) was
    /// clicked: open it.
    OpenConversation(ChannelId),
    /// A message's reply-count chip was clicked: open the thread in its conversation.
    OpenThread { channel: ChannelId, thread_ts: Ts },
    /// A link inside a message (@mention, #channel, URL) was activated.
    OpenLink(LinkTarget),
    /// The group's "Mark as Read" (or Esc): move the read cursor to `ts`.
    MarkRead { channel: ChannelId, ts: Ts },
    /// Quick inline reply sent directly from an unread card (`F-CORE-031`).
    Reply { channel: ChannelId, text: String },
    /// A message-level action triggered from an unread row's hover toolbar, reaction chips,
    /// image click, or context menu (react, reply in thread, save, pin, copy, forward, etc.).
    MessageAction {
        channel: ChannelId,
        message: Box<Message>,
        action: MessageAction,
    },
    /// "Mark All Messages Read": the root runs it through the user's confirmation setting.
    MarkAllRead,
}

/// Sort order for the All Unreads sections, matching Slack's header dropdown.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum UnreadsSort {
    #[default]
    Recommended,
    Newest,
    Oldest,
    Alphabetical,
}

impl UnreadsSort {
    fn label(self) -> &'static str {
        match self {
            Self::Recommended => "Sorted by recommended order",
            Self::Newest => "Sorted by newest",
            Self::Oldest => "Sorted by oldest",
            Self::Alphabetical => "Sorted alphabetically",
        }
    }

    fn menu_label(self) -> &'static str {
        match self {
            Self::Recommended => "Recommended order",
            Self::Newest => "Newest first",
            Self::Oldest => "Oldest first",
            Self::Alphabetical => "Alphabetical",
        }
    }
}

pub struct UnreadsView {
    fixtures: Arc<Fixtures>,
    groups: Vec<UnreadGroup>,
    /// True during the very first live store read before any groups have landed.
    loading: bool,
    /// Conversations whose cards the user collapsed.
    collapsed: HashSet<ChannelId>,
    /// The keyboard-focused group (Up/Down move it, Left/Right fold it, Esc marks it read).
    focused: Option<usize>,
    /// Active sort order for unread sections.
    sort: UnreadsSort,
    /// Active inline reply composer on one unread card (`F-CORE-031`).
    replying: Option<(ChannelId, Entity<TextareaState>)>,
    /// Thread currently open in the right-hand panel so its root row carries Slack's tint.
    open_thread: Option<(ChannelId, Ts)>,
    /// Message timestamps currently saved for later (mirrored from `LaterView`).
    saved: HashSet<Ts>,
}

impl EventEmitter<UnreadsEvent> for UnreadsView {}

impl UnreadsView {
    pub fn new(fixtures: Arc<Fixtures>, cx: &mut App) -> Entity<UnreadsView> {
        cx.new(|_| UnreadsView {
            fixtures,
            groups: Vec::new(),
            loading: false,
            collapsed: HashSet::new(),
            focused: None,
            sort: UnreadsSort::Recommended,
            replying: None,
            open_thread: None,
            saved: HashSet::new(),
        })
    }

    /// Replace the set of saved message timestamps shown with Slack's saved banner/tint.
    pub fn replace_saved(&mut self, saved: HashSet<Ts>, cx: &mut Context<Self>) {
        if self.saved != saved {
            self.saved = saved;
            cx.notify();
        }
    }

    /// Optimistically toggle `name` reaction on `(channel, ts)` inside the unread groups.
    /// Returns `Some(true)` if added, `Some(false)` if removed, `None` if message wasn't present.
    pub fn toggle_reaction(
        &mut self,
        channel: &ChannelId,
        ts: &Ts,
        name: &str,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        let me = self.fixtures.self_user.clone();
        let group = self.groups.iter_mut().find(|g| g.channel == *channel)?;
        let msg = group.messages.iter_mut().find(|m| m.ts == *ts)?;
        let added = actions::toggle_reaction(msg, &me, name);
        cx.notify();
        Some(added)
    }

    /// Optimistically toggle pinned status on `(channel, ts)` inside the unread groups.
    pub fn toggle_pin(
        &mut self,
        channel: &ChannelId,
        ts: &Ts,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        let group = self.groups.iter_mut().find(|g| g.channel == *channel)?;
        let msg = group.messages.iter_mut().find(|m| m.ts == *ts)?;
        let pinned = actions::toggle_pin(msg, channel);
        cx.notify();
        Some(pinned)
    }

    /// Remove a deleted message from its unread group.
    pub fn remove_message(&mut self, channel: &ChannelId, ts: &Ts, cx: &mut Context<Self>) {
        if let Some(group) = self.groups.iter_mut().find(|g| g.channel == *channel) {
            group.messages.retain(|m| m.ts != *ts);
        }
        self.groups.retain(|g| !g.messages.is_empty());
        cx.notify();
    }

    /// Look up one unread message by `(channel, ts)` (seeds the right-hand thread panel when a
    /// thread is opened directly from All Unreads).
    pub fn message_by_ts(&self, channel: &ChannelId, ts: &Ts) -> Option<Message> {
        self.groups
            .iter()
            .find(|group| group.channel == *channel)
            .and_then(|group| group.messages.iter().find(|msg| msg.ts == *ts))
            .cloned()
    }

    /// Highlight the unread row whose thread is currently open in the right-hand panel.
    pub fn set_open_thread(&mut self, open: Option<(ChannelId, Ts)>, cx: &mut Context<Self>) {
        if self.open_thread != open {
            self.open_thread = open;
            cx.notify();
        }
    }

    /// Live mode open/refresh: keep existing store-loaded groups visible while the background
    /// SQLite read runs (no flash of empty `"Open conversation to load messages."` placeholders).
    pub fn begin_live_refresh(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        if self.groups.is_empty() {
            self.loading = true;
        }
        cx.notify();
    }

    /// Show `groups` (the fixture or snapshot derivation); call when the view opens or the
    /// workspace reloads.
    pub fn show_local(
        &mut self,
        fixtures: Arc<Fixtures>,
        groups: Vec<UnreadGroup>,
        cx: &mut Context<Self>,
    ) {
        self.fixtures = fixtures;
        self.loading = false;
        self.set_groups(groups, cx);
    }

    /// Fold in `groups` (the store derivation) for the conversations `keep` still counts
    /// unread, in `keep`'s order: shown conversations take their stored rows, one the store
    /// missed keeps the rows it has (a mark raced the query), and conversations no longer
    /// unread drop out.
    pub fn show_store(
        &mut self,
        groups: Vec<UnreadGroup>,
        keep: &[ChannelId],
        cx: &mut Context<Self>,
    ) {
        let mut fresh: Vec<Option<UnreadGroup>> = groups.into_iter().map(Some).collect();
        let take = |fresh: &mut [Option<UnreadGroup>], channel: &ChannelId| {
            let at = fresh
                .iter()
                .position(|group| group.as_ref().is_some_and(|g| &g.channel == channel))?;
            fresh[at].take()
        };
        let mut merged: Vec<UnreadGroup> = self
            .groups
            .drain(..)
            .filter(|group| keep.contains(&group.channel))
            .map(|group| take(&mut fresh, &group.channel).unwrap_or(group))
            .collect();
        for channel in keep {
            if merged.iter().any(|group| &group.channel == channel) {
                continue;
            }
            if let Some(group) = take(&mut fresh, channel) {
                merged.push(group);
            }
        }
        merged.sort_by_key(|group| keep.iter().position(|channel| *channel == group.channel));
        self.loading = false;
        self.set_groups(merged, cx);
    }

    fn set_groups(&mut self, groups: Vec<UnreadGroup>, cx: &mut Context<Self>) {
        self.groups = groups;
        self.apply_sort();
        self.collapsed
            .retain(|channel| self.groups.iter().any(|group| &group.channel == channel));
        self.focused = self.focused.filter(|ix| *ix < self.groups.len());
        cx.notify();
    }

    fn set_sort(&mut self, sort: UnreadsSort, cx: &mut Context<Self>) {
        self.sort = sort;
        self.apply_sort();
        cx.notify();
    }

    fn apply_sort(&mut self) {
        match self.sort {
            UnreadsSort::Recommended => {}
            UnreadsSort::Newest => {
                self.groups.sort_by(|a, b| b.newest_ts.cmp(&a.newest_ts));
            }
            UnreadsSort::Oldest => {
                self.groups.sort_by(|a, b| a.newest_ts.cmp(&b.newest_ts));
            }
            UnreadsSort::Alphabetical => {
                let fixtures = &self.fixtures;
                self.groups.sort_by_cached_key(|g| {
                    fixtures
                        .conversation(&g.channel)
                        .map(|c| fixtures.conversation_label(c))
                        .unwrap_or_else(|| g.channel.to_string())
                        .trim_start_matches("# ")
                        .to_lowercase()
                });
            }
        }
    }

    /// Live refresh: keep the groups, draw them from `fixtures`.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        cx.notify();
    }

    /// The view's keys (Slack's "Navigate unread messages"): Up/Down move the focus,
    /// Left/Right collapse or expand the focused group, Esc marks it read. Returns whether
    /// the key was taken; with nothing unread no key is.
    pub fn handle_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        match key {
            "up" | "down" if !self.groups.is_empty() => {
                self.focused = match self.focused {
                    None => Some(0),
                    Some(ix) if key == "down" => Some((ix + 1).min(self.groups.len() - 1)),
                    Some(ix) => Some(ix.saturating_sub(1)),
                };
                cx.notify();
                true
            }
            "left" | "right" if !self.groups.is_empty() => {
                let ix = self
                    .focused
                    .filter(|ix| *ix < self.groups.len())
                    .unwrap_or(0);
                self.focused = Some(ix);
                let channel = self.groups[ix].channel.clone();
                if key == "left" {
                    self.collapsed.insert(channel);
                } else {
                    self.collapsed.remove(&channel);
                }
                cx.notify();
                true
            }
            "escape" if !self.groups.is_empty() => {
                if let Some(channel) = self
                    .groups
                    .get(self.focused.unwrap_or(0))
                    .map(|group| group.channel.clone())
                {
                    self.mark_read(&channel, cx);
                }
                true
            }
            _ => false,
        }
    }

    /// Collapse or expand group `ix` (its chevron) without triggering keyboard-focus styling.
    fn toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(group) = self.groups.get(ix) {
            let channel = group.channel.clone();
            if !self.collapsed.remove(&channel) {
                self.collapsed.insert(channel);
            }
        }
        cx.notify();
    }

    /// Mark `channel`'s group read, looked up when the action runs (a refresh that moved the
    /// groups cannot redirect it): drop its card, pass the focus to the group that takes its
    /// place, and tell the root where Slack's read cursor moves.
    fn mark_read(&mut self, channel: &ChannelId, cx: &mut Context<Self>) {
        let Some(ix) = self
            .groups
            .iter()
            .position(|group| &group.channel == channel)
        else {
            return;
        };
        let group = self.groups.remove(ix);
        self.collapsed.remove(channel);
        if self.replying.as_ref().is_some_and(|(ch, _)| ch == channel) {
            self.replying = None;
        }
        self.focused = (!self.groups.is_empty() && self.focused.is_some())
            .then(|| ix.min(self.groups.len() - 1));
        cx.emit(UnreadsEvent::MarkRead {
            channel: group.channel,
            ts: group.newest_ts,
        });
        cx.notify();
    }

    fn start_reply(&mut self, channel: ChannelId, window: &mut Window, cx: &mut Context<Self>) {
        self.collapsed.remove(&channel);
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Write a quick reply…")
                .auto_grow(1, 4)
        });
        input.read(cx).focus_handle(cx).focus(window, cx);
        self.replying = Some((channel, input));
        cx.notify();
    }

    /// The message `ts` of `channel` the list shows.
    fn message_in(&self, channel: &ChannelId, ts: &Ts) -> Option<&Message> {
        self.groups
            .iter()
            .find(|group| &group.channel == channel)?
            .messages
            .iter()
            .find(|message| &message.ts == ts)
    }

    fn submit_reply(&mut self, cx: &mut Context<Self>) {
        let Some((channel, input)) = self.replying.take() else {
            return;
        };
        let text = input.read(cx).value().trim().to_owned();
        if text.is_empty() {
            cx.notify();
            return;
        }
        cx.emit(UnreadsEvent::Reply {
            channel: channel.clone(),
            text,
        });
        self.mark_read(&channel, cx);
    }
}

impl Render for UnreadsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let background = theme.background;
        let groups = &self.groups;
        let collapsed = &self.collapsed;
        let (replying, open_thread, saved) = (&self.replying, &self.open_thread, &self.saved);
        let fixtures = &self.fixtures;
        let focused = self.focused;
        let sort = self.sort;
        let this = cx.entity().downgrade();

        // Official Slack header: bold "Unreads" followed by subtle filter & sort dropdowns,
        // with "Mark All Messages Read" at the bottom of the stream.
        let sort_menu = Button::new("unreads-sort")
            .ghost()
            .with_size(Size::XSmall)
            .text_color(muted)
            .label(sort.label())
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                for choice in [
                    UnreadsSort::Recommended,
                    UnreadsSort::Newest,
                    UnreadsSort::Oldest,
                    UnreadsSort::Alphabetical,
                ] {
                    let this = this.clone();
                    menu = menu.item(
                        PopupMenuItem::new(choice.menu_label())
                            .checked(choice == sort)
                            .on_click(move |_, _, cx| {
                                let _ = this.update(cx, |view, cx| view.set_sort(choice, cx));
                            }),
                    );
                }
                menu
            });

        let header = h_flex()
            .flex_none()
            .h(px(49.))
            .px_5()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::BOLD)
                    .child("Unreads"),
            )
            .child(sort_menu);

        let loading = self.loading;
        let body = if groups.is_empty() {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(if loading {
                    v_flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .text_color(muted)
                        .child("Loading unread messages…")
                } else {
                    v_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::CheckCheck)
                                .text_color(muted)
                                .size(px(28.)),
                        )
                        .child(
                            div()
                                .text_size(px(20.))
                                .font_weight(FontWeight::BOLD)
                                .child("No more unread messages. All clear."),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child("You're all caught up."),
                        )
                })
                .into_any_element()
        } else {
            div()
                .id("unreads-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(
                    v_flex()
                        .w_full()
                        .children((0..groups.len()).map(|ix| {
                            let reply_input = replying
                                .as_ref()
                                .filter(|(ch, _)| *ch == groups[ix].channel)
                                .map(|(_, input)| input.clone());
                            let show_esc_hint =
                                focused == Some(ix) || (focused.is_none() && ix == 0);
                            card_view(
                                UnreadCardArgs {
                                    fixtures,
                                    ix,
                                    group: &groups[ix],
                                    collapsed: collapsed.contains(&groups[ix].channel),
                                    focused: focused == Some(ix),
                                    show_esc_hint,
                                    reply_input,
                                    open_thread: open_thread.as_ref(),
                                    saved_set: saved,
                                    background,
                                },
                                cx,
                            )
                        }))
                        .child(
                            h_flex().flex_none().justify_center().py_6().child(
                                Button::new("unreads-mark-all-bottom")
                                    .outline()
                                    .with_size(Size::Small)
                                    .label("Mark All Messages Read")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(UnreadsEvent::MarkAllRead)
                                    })),
                            ),
                        ),
                )
                .into_any_element()
        };

        v_flex()
            .size_full()
            .bg(background)
            .child(header)
            .child(body)
    }
}

struct UnreadCardArgs<'a> {
    fixtures: &'a Arc<Fixtures>,
    ix: usize,
    group: &'a UnreadGroup,
    collapsed: bool,
    focused: bool,
    show_esc_hint: bool,
    reply_input: Option<Entity<TextareaState>>,
    open_thread: Option<&'a (ChannelId, Ts)>,
    saved_set: &'a HashSet<Ts>,
    background: Hsla,
}

/// One conversation's edge-to-edge section matching official Slack's Unreads view:
/// a tinted full-width section bar (collapse chevron, channel icon + bold name on left;
/// message count, optional `Press [Esc] to` hint, and outline `Mark as Read` button on right)
/// over full-width messages with date dividers, consecutive-message grouping, and reply chips.
fn card_view(args: UnreadCardArgs<'_>, cx: &mut Context<UnreadsView>) -> AnyElement {
    let UnreadCardArgs {
        fixtures,
        ix,
        group,
        collapsed,
        focused,
        show_esc_hint,
        reply_input,
        open_thread,
        saved_set,
        background,
    } = args;
    let theme = cx.theme();
    let (border, muted, danger, danger_foreground, link, hover, strip_bg) = (
        theme.border,
        theme.muted_foreground,
        theme.danger,
        theme.danger_foreground,
        theme.link,
        theme.accent,
        theme.muted.opacity(0.42),
    );
    let conversation = fixtures.conversation(&group.channel);
    let label = conversation
        .map(|conversation| fixtures.conversation_label(conversation))
        .unwrap_or_else(|| group.channel.to_string());
    let kind = conversation.map(|conversation| conversation.kind());
    // The conversation badge: # for public channels, a lock for private ones, the peer
    // avatar for DMs - as every row in the app renders it.
    let badge = match kind {
        Some(ConversationKind::Im) | Some(ConversationKind::Mpim) => conversation
            .map(|conversation| {
                conversation_avatar(fixtures, conversation, 18., background, cx).into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element()),
        Some(ConversationKind::PrivateChannel) => Icon::new(IconName::Lock)
            .text_color(theme.foreground)
            .size(px(14.))
            .into_any_element(),
        _ => Icon::new(IconName::Hash)
            .text_color(theme.foreground)
            .size(px(14.))
            .into_any_element(),
    };
    let name = match kind {
        Some(ConversationKind::PublicChannel) | Some(ConversationKind::PrivateChannel) => {
            label.trim_start_matches("# ").to_owned()
        }
        _ => label,
    };
    let open = group.channel.clone();
    let mark_read = group.channel.clone();

    let count_label = (!group.messages.is_empty()).then(|| {
        if group.messages.len() == 1 {
            "1 message".to_owned()
        } else {
            format!("{} messages", group.messages.len())
        }
    });

    let header = h_flex()
        .relative()
        .flex_none()
        .group(SharedString::from(format!("unreads-hdr-{ix}")))
        .h(px(44.))
        .px_5()
        .items_center()
        .justify_between()
        .gap_2()
        .bg(strip_bg)
        .border_b_1()
        .when(ix > 0, |header| header.border_t_1())
        .border_color(border)
        .when(focused, |header| {
            header.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(2.))
                    .bg(link),
            )
        })
        .child(
            h_flex()
                .min_w_0()
                .items_center()
                .gap_1p5()
                .child(
                    div()
                        .id(("unreads-collapse", ix))
                        .size(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|toggle| toggle.bg(hover))
                        .child(
                            Icon::new(if collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            })
                            .text_color(muted)
                            .size(px(14.)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle(ix, cx))),
                )
                .child(
                    h_flex()
                        .id(("unreads-open", ix))
                        .min_w_0()
                        .items_center()
                        .gap_1p5()
                        .rounded_md()
                        .px_1()
                        .cursor_pointer()
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(UnreadsEvent::OpenConversation(open.clone()))
                        }))
                        .child(badge)
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::BOLD)
                                .truncate()
                                .hover(|el| el.underline())
                                .child(name),
                        )
                        .when(group.mention_count > 0, |row| {
                            row.child(
                                div()
                                    .rounded_full()
                                    .bg(danger)
                                    .text_xs()
                                    .px_1p5()
                                    .text_color(danger_foreground)
                                    .child(group.mention_count.to_string()),
                            )
                        }),
                ),
        )
        .child(
            h_flex()
                .flex_none()
                .items_center()
                .gap_2p5()
                .child({
                    let reply_channel = group.channel.clone();
                    div()
                        .invisible()
                        .group_hover(SharedString::from(format!("unreads-hdr-{ix}")), |el| {
                            el.visible()
                        })
                        .child(
                            Button::new(("unreads-reply-btn", ix))
                                .ghost()
                                .with_size(Size::XSmall)
                                .icon(IconName::Reply)
                                .tooltip("Quick reply")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.start_reply(reply_channel.clone(), window, cx);
                                })),
                        )
                })
                .children(count_label.map(|label| div().text_xs().text_color(muted).child(label)))
                .when(show_esc_hint, |row| {
                    row.child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .text_color(muted)
                            .child("Press")
                            .child(
                                div()
                                    .px_1p5()
                                    .py(px(1.))
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(border)
                                    .text_color(theme.foreground)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Esc"),
                            )
                            .child("to"),
                    )
                })
                .child(
                    Button::new(("unreads-mark-read", ix))
                        .outline()
                        .with_size(Size::Small)
                        .label("Mark as Read")
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.mark_read(&mark_read, cx)),
                        ),
                ),
        );

    let channel = group.channel.clone();
    let body = if collapsed {
        None
    } else if group.messages.is_empty() {
        // Live mode, before the conversation's history lands in the store.
        Some(
            div()
                .id(("unreads-load", ix))
                .px_5()
                .py_4()
                .text_sm()
                .text_color(muted)
                .cursor_pointer()
                .hover(|row| row.underline())
                .child("Open conversation to load messages.")
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(UnreadsEvent::OpenConversation(channel.clone()))
                }))
                .into_any_element(),
        )
    } else {
        // The section's rows share one link handler: @mentions (and their hover cards),
        // #channels and URLs route out as the view's own event.
        let view = cx.entity();
        let view_for_links = view.clone();
        let links: LinkHandler = Rc::new(move |target, _window, cx| {
            view_for_links.update(cx, |_, cx| cx.emit(UnreadsEvent::OpenLink(target.clone())));
        });
        let hover_bg = cx.theme().secondary.opacity(0.5);
        let saved_bg = cx.theme().warning.opacity(0.08);
        let thread_bg = crate::theme::thread_root(cx);
        let mut rows: Vec<AnyElement> = Vec::with_capacity(group.messages.len());
        let mut prev_day: Option<String> = None;
        let mut prev_msg: Option<&Message> = None;
        for (m_ix, message) in group.messages.iter().enumerate() {
            let day = day_label(&message.ts);
            let date_break = match prev_day.as_deref() {
                None => day != "Today",
                Some(prev) => prev != day,
            };
            if date_break {
                rows.push(
                    crate::messages::date_divider(
                        day.clone(),
                        format!("unreads-{ix}-day-{m_ix}"),
                        None,
                        cx,
                    )
                    .into_any_element(),
                );
            }
            let msg_group = SharedString::from(format!("unreads-{ix}-msg-{m_ix}"));
            let key = format!("unreads-{ix}-row-{}", message.ts.as_str());
            let grouped = (!date_break)
                .then(|| prev_msg.filter(|prev| same_group(prev, message)))
                .flatten()
                .map(|_| msg_group.clone());
            let is_saved = saved_set.contains(&message.ts);
            let is_open_thread = !is_saved
                && open_thread.is_some_and(|(ch, ts)| *ch == group.channel && *ts == message.ts);

            let (view_for_action, ch_for_action, ts_for_action) =
                (view.clone(), group.channel.clone(), message.ts.clone());
            let handler: ActionHandler = Rc::new(move |action, _window, cx| {
                let (ch, ts) = (ch_for_action.clone(), ts_for_action.clone());
                view_for_action.update(cx, |view, cx| {
                    // The groups may have reordered or dropped the message since the row rendered.
                    let Some(message) = view.message_in(&ch, &ts) else {
                        return;
                    };
                    cx.emit(UnreadsEvent::MessageAction {
                        channel: ch,
                        message: Box::new(message.clone()),
                        action,
                    });
                });
            });
            let set = ActionSet {
                own: message.user.as_ref() == Some(&fixtures.self_user),
                reply: true,
                saved: is_saved,
                pinned: message.pinned_to.contains(&group.channel),
                in_thread: false,
            };
            let row_body =
                message_row_linked(fixtures, message, Some(&handler), Some(&links), grouped, cx);
            let ch = group.channel.clone();
            let ts = message.ts.clone();
            let view_for_thread = view.clone();
            let reply_group = SharedString::from(format!("unreads-{ix}-reply-{m_ix}"));
            let reply_bar_el = reply_bar(
                fixtures,
                message,
                ("unreads-thread-chip", ix * 1000 + m_ix),
                msg_group.clone(),
                reply_group,
                move |_, cx| {
                    view_for_thread.update(cx, |_, cx| {
                        cx.emit(UnreadsEvent::OpenThread {
                            channel: ch.clone(),
                            thread_ts: ts.clone(),
                        });
                    });
                },
                cx,
            )
            // Align flush with `message_row_linked`'s body (36px avatar + 12px gap).
            .map(|bar| div().pl(px(48.)).child(bar));
            let quick = crate::preferences::active_quick_reactions(&fixtures.quick_reactions, cx);
            let menu_handler = handler.clone();
            let row_key = key.clone();
            let keep_inside = m_ix == 0 && !date_break;
            let item = actions::HoverOverlay::new(SharedString::from(key), move |hover, _, cx| {
                let revealed = hover.revealed(cx);
                v_flex()
                    .id(SharedString::from(row_key.clone()))
                    .group(msg_group)
                    .relative()
                    .px_5()
                    .py_0p5()
                    .when(is_saved, |row| {
                        row.bg(saved_bg).child(actions::saved_banner(cx))
                    })
                    .when(is_open_thread, |row| row.bg(thread_bg))
                    .when(!is_saved && !is_open_thread && revealed, |row| {
                        row.bg(hover_bg)
                    })
                    .child(row_body)
                    .children(reply_bar_el)
                    .when(revealed, |row| {
                        row.child(
                            actions::hover_toolbar(&row_key, &quick, set, &handler, &hover, cx)
                                // Keep the first row's bar inside the row so the section header never clips it.
                                .when(keep_inside, |bar| bar.top(px(3.))),
                        )
                    })
                    .on_hover(hover.host_on_hover())
                    .context_menu(move |menu, window, cx| {
                        actions::actions_menu(
                            menu,
                            set,
                            &menu_handler,
                            MenuOrigin::Context,
                            window,
                            cx,
                        )
                    })
                    .into_any_element()
            });
            rows.push(item.into_any_element());
            prev_day = Some(day);
            prev_msg = Some(message);
        }
        Some(
            v_flex()
                .w_full()
                .py_3()
                .gap_1()
                .children(rows)
                .into_any_element(),
        )
    };

    let reply_box = if !collapsed {
        reply_input.map(|input| {
            h_flex()
                .items_end()
                .gap_2()
                .px_5()
                .py_2()
                .border_t_1()
                .border_color(border)
                .child(
                    crate::preferences::text_input_box(&input, true, cx)
                        .flex_1()
                        .min_w_0(),
                )
                .child(
                    crate::preferences::form_button(
                        Button::new(("unreads-reply-cancel", ix))
                            .ghost()
                            .label("Cancel"),
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.replying = None;
                        cx.notify();
                    })),
                )
                .child(
                    crate::preferences::form_button(
                        Button::new(("unreads-reply-send", ix))
                            .primary()
                            .label("Send"),
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.submit_reply(cx))),
                )
        })
    } else {
        None
    };

    v_flex()
        .id(("unreads-card", ix))
        .w_full()
        .flex_none()
        .bg(background)
        .child(header)
        .children(body)
        .children(reply_box)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui_kit::gpui::{Subscription, TestAppContext};

    use super::*;
    use crate::fixtures;

    /// A `client.counts` entry, as the snapshot keeps it.
    fn counts(
        channel: &str,
        last_read: Option<&str>,
        latest: Option<&str>,
        mentions: u32,
        unreads: bool,
    ) -> slackless_core::api::ConversationCounts {
        slackless_core::api::ConversationCounts {
            id: ChannelId::from(channel),
            last_read: last_read.map(Ts::from),
            latest: latest.map(Ts::from),
            mention_count: mentions,
            dm_count: 0,
            has_unreads: unreads,
            extra: Default::default(),
        }
    }

    /// A group of bare messages at `timestamps`, for view-state tests.
    fn group(channel: &str, timestamps: &[&str]) -> UnreadGroup {
        UnreadGroup {
            channel: ChannelId::from(channel),
            messages: timestamps
                .iter()
                .map(|ts| Message {
                    ts: Ts::from(*ts),
                    ..Default::default()
                })
                .collect(),
            newest_ts: Ts::from(timestamps.last().copied().unwrap_or("0")),
            mention_count: 0,
        }
    }

    #[test]
    fn local_groups_keep_only_newer_rows_oldest_first() {
        let fixtures = fixtures::load();
        // D0ALICE is read up to its first message; the reply after it (and only it - the
        // middle one is the user's own) is unread.
        let last_read = fixtures
            .conversation(&ChannelId::from("D0ALICE"))
            .and_then(|dm| dm.last_read.clone())
            .unwrap();
        let groups = local_groups(&fixtures, &[ChannelId::from("D0ALICE")]);
        let unread = groups
            .iter()
            .find(|group| group.channel == ChannelId::from("D0ALICE"))
            .expect("the unread DM groups");
        assert!(unread.messages.iter().all(|message| message.ts > last_read));
        assert!(
            unread
                .messages
                .iter()
                .all(|message| message.user.as_ref() != Some(&fixtures.self_user))
        );
        assert_eq!(
            unread.messages.len(),
            1,
            "one reply, one own message behind"
        );
        // #general has no cursor: every message but the user's own shows, oldest first.
        let general = local_groups(&fixtures, &[ChannelId::from("C0GENERAL")])
            .pop()
            .unwrap();
        assert!(
            general
                .messages
                .windows(2)
                .all(|pair| pair[0].ts < pair[1].ts)
        );
        assert!(
            general
                .messages
                .iter()
                .all(|message| message.user.as_ref() != Some(&fixtures.self_user))
        );
        assert_eq!(general.newest_ts, general.messages.last().unwrap().ts);
    }

    #[test]
    fn muted_channels_list_only_with_mentions() {
        let mut fixtures = fixtures::load();
        fixtures.counts.clear();
        let muted = ChannelId::from("C0DESIGN");
        assert_eq!(
            fixtures.notify_levels.get(&muted),
            Some(&NotifyLevel::Muted),
            "the sample workspace mutes #design"
        );
        fixtures.counts.insert(
            ChannelId::from("C0DESIGN"),
            counts("C0DESIGN", None, Some("3000.000000"), 0, true),
        );
        assert!(
            local_groups(&fixtures, std::slice::from_ref(&muted)).is_empty(),
            "a muted channel without mentions stays out, as Slack lists it"
        );
        fixtures.counts.insert(
            ChannelId::from("C0DESIGN"),
            counts("C0DESIGN", None, Some("3000.000000"), 2, true),
        );
        let groups = local_groups(&fixtures, std::slice::from_ref(&muted));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].mention_count, 2);
    }

    #[test]
    fn counted_unreads_group_without_stored_rows() {
        let mut fixtures = fixtures::load();
        let channel = ChannelId::from("C0ENG");
        fixtures.messages.remove(&channel);
        fixtures.counts.insert(
            ChannelId::from("C0ENG"),
            counts("C0ENG", Some("1500.000000"), Some("3000.000000"), 1, true),
        );
        let groups = local_groups(&fixtures, std::slice::from_ref(&channel));
        let unread = groups.first().expect("the counted channel groups");
        assert!(unread.messages.is_empty(), "no rows are stored for it");
        assert_eq!(unread.newest_ts, Ts::from("3000.000000"));
        assert_eq!(unread.mention_count, 1);
        // The store targets keep the cursor the counts reported.
        let targets = store_targets(&fixtures, groups.as_slice());
        assert_eq!(
            targets,
            vec![(
                channel,
                Some(Ts::from("1500.000000")),
                Some(Ts::from("3000.000000")),
                1
            )]
        );
    }

    #[test]
    fn read_conversations_do_not_group() {
        let mut fixtures = fixtures::load();
        let channel = ChannelId::from("C0GENERAL");
        let last_read = fixtures
            .messages
            .get(&channel)
            .unwrap()
            .last()
            .unwrap()
            .ts
            .clone();
        if let Some(conversation) = fixtures.channels.iter_mut().find(|c| c.id == channel) {
            conversation.last_read = Some(last_read.clone());
        }
        let groups = local_groups(&fixtures, std::slice::from_ref(&channel));
        assert!(
            groups.is_empty(),
            "a cursor at the newest message leaves nothing to read"
        );
        let targets = vec![(channel, Some(last_read), None, 0)];
        let store =
            crate::search::fixture_archive(&fixtures).expect("the fixture archive seeds a store");
        let team = fixtures.workspaces[0].id.clone();
        assert!(
            store_groups(&store.lock().unwrap(), &team, &targets).is_empty(),
            "the store agrees: no row is newer"
        );
    }

    #[test]
    fn store_groups_read_the_watermark_and_reach_the_newest_known() {
        let fixtures = fixtures::load();
        let store =
            crate::search::fixture_archive(&fixtures).expect("the fixture archive seeds a store");
        let team = fixtures.workspaces[0].id.clone();
        let channel = ChannelId::from("C0GENERAL");
        let history = fixtures.messages.get(&channel).unwrap();
        let last_read = history.first().unwrap().ts.clone();
        // `client.counts` knows a newest message the store has not synced.
        let latest = Ts::from("9999999999.000100");
        let targets = vec![(
            channel.clone(),
            Some(last_read.clone()),
            Some(latest.clone()),
            0,
        )];
        let groups = store_groups(&store.lock().unwrap(), &team, &targets);
        let unread = groups.first().expect("the channel groups");
        assert!(unread.messages.iter().all(|message| message.ts > last_read));
        assert!(
            unread
                .messages
                .windows(2)
                .all(|pair| pair[0].ts < pair[1].ts)
        );
        assert_eq!(unread.newest_ts, latest);
    }

    #[test]
    fn live_refresh_coalesces_bursts_and_drops_overtaken_reads() {
        let (ours, theirs) = (TeamId::from("T0OURS"), TeamId::from("T0THEIRS"));
        let mut refresh = LiveRefresh::default();

        // A burst while a read is in flight runs as one more read after it.
        let first = refresh
            .request(&ours)
            .expect("an idle refresh reads at once");
        assert_eq!(refresh.request(&ours), None);
        assert_eq!(refresh.request(&ours), None);
        assert_eq!(
            refresh.land(first),
            Some(true),
            "it lands, one more read follows"
        );
        let follow_up = refresh.request(&ours).expect("the follow-up read starts");
        assert_eq!(
            refresh.land(follow_up),
            Some(false),
            "nothing left to follow"
        );

        // Another workspace's refresh overtakes the read in flight: its result never lands.
        let overtaken = refresh.request(&ours).unwrap();
        let current = refresh
            .request(&theirs)
            .expect("another workspace reads at once");
        assert_eq!(
            refresh.land(overtaken),
            None,
            "the overtaken result is dropped"
        );
        assert_eq!(refresh.land(current), Some(false));

        // So does a derivation shown without the store meanwhile, leaving nothing in flight.
        let overtaken = refresh.request(&ours).unwrap();
        refresh.overtake();
        assert_eq!(refresh.land(overtaken), None);
        assert!(refresh.request(&ours).is_some());
    }

    #[test]
    fn store_groups_keep_channels_the_store_has_not_synced() {
        let fixtures = fixtures::load();
        let store =
            crate::search::fixture_archive(&fixtures).expect("the fixture archive seeds a store");
        let team = fixtures.workspaces[0].id.clone();
        // A directory channel: the conversation is stored, its history is not.
        let unsynced = ChannelId::from("C0OPS");
        let ahead = vec![(
            unsynced.clone(),
            Some(Ts::from("1000.000000")),
            Some(Ts::from("2000.000000")),
            3,
        )];
        let groups = store_groups(&store.lock().unwrap(), &team, &ahead);
        let unread = groups.first().expect("the counted channel groups");
        assert!(unread.messages.is_empty());
        assert_eq!(unread.newest_ts, Ts::from("2000.000000"));
        assert_eq!(unread.mention_count, 3);
        // A `latest` at or behind the cursor means nothing unread: no group.
        let behind = vec![(
            unsynced,
            Some(Ts::from("2000.000000")),
            Some(Ts::from("1000.000000")),
            0,
        )];
        assert!(store_groups(&store.lock().unwrap(), &team, &behind).is_empty());
    }

    #[gpui::test]
    fn keyboard_walks_collapses_and_marks_read(cx: &mut TestAppContext) {
        let fixtures = Arc::new(fixtures::load());
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let view = cx.update(|cx| UnreadsView::new(fixtures.clone(), cx));
        let seen = events.clone();
        let recorder = cx.new(|cx| {
            struct Recorder {
                _sub: Subscription,
            }
            let subscription = cx.subscribe(&view, move |_, _, event: &UnreadsEvent, _| {
                if let UnreadsEvent::MarkRead { channel, ts } = event {
                    seen.borrow_mut().push((channel.clone(), ts.clone()));
                }
            });
            Recorder { _sub: subscription }
        });

        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.show_local(
                    fixtures.clone(),
                    vec![
                        group("C1", &["100.000001"]),
                        group("C2", &["200.000001"]),
                        group("C3", &["300.000001"]),
                    ],
                    cx,
                );
                // Left right after opening (before Up/Down) focuses and collapses the first card.
                assert!(view.handle_key("left", cx));
                assert_eq!(view.focused, Some(0));
                assert!(view.collapsed.contains(&ChannelId::from("C1")));
                assert!(view.handle_key("right", cx));
                assert!(!view.collapsed.contains(&ChannelId::from("C1")));
                view.focused = None;

                // Down focuses the first group; the focus walks both ways and stops at the
                // ends.
                assert!(view.handle_key("down", cx));
                assert_eq!(view.focused, Some(0));
                assert!(view.handle_key("up", cx));
                assert_eq!(view.focused, Some(0));
                assert!(view.handle_key("down", cx));
                assert_eq!(view.focused, Some(1));
                // Left collapses the focused group, Right expands it.
                assert!(view.handle_key("left", cx));
                assert!(view.collapsed.contains(&ChannelId::from("C2")));
                assert!(view.handle_key("right", cx));
                assert!(!view.collapsed.contains(&ChannelId::from("C2")));
            });
        });

        cx.update(|cx| {
            view.update(cx, |view, cx| {
                // Esc marks the focused group read; the group below takes its place and the
                // focus stays on it.
                assert!(view.handle_key("escape", cx));
            });
        });
        assert_eq!(
            events.borrow().as_slice(),
            [(ChannelId::from("C2"), Ts::from("200.000001"))],
            "Esc marks the focused group read up to its newest message"
        );
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(view.groups.len(), 2);
            assert_eq!(view.groups[0].channel, ChannelId::from("C1"));
            assert_eq!(
                view.focused,
                Some(1),
                "the group below the marked one takes its place and the focus"
            );
        });

        // Marking the last group read passes the focus back.
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.handle_key("down", cx);
                view.handle_key("escape", cx);
                view.handle_key("escape", cx);
            });
        });
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(view.groups.is_empty());
            assert_eq!(view.focused, None);
        });
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                assert!(
                    !view.handle_key("escape", cx),
                    "with nothing left, Esc is not taken"
                );
            });
        });
        drop(recorder);
    }

    #[gpui::test]
    fn store_folds_refresh_without_resurrecting_marks(cx: &mut TestAppContext) {
        let fixtures = Arc::new(fixtures::load());
        let view = cx.update(|cx| UnreadsView::new(fixtures.clone(), cx));
        let c2 = ChannelId::from("C2");
        let c3 = ChannelId::from("C3");
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.show_local(
                    fixtures.clone(),
                    vec![group("C1", &["100.000001"]), group("C2", &["200.000001"])],
                    cx,
                );
                view.mark_read(&ChannelId::from("C1"), cx);
                assert_eq!(view.groups.len(), 1, "the marked group left");
                // The store derivation still lists C1 (the read raced the query); only
                // channels Slack still counts unread take part.
                view.show_store(
                    vec![
                        group("C1", &["100.000001", "100.000002"]),
                        group("C2", &["200.000001", "200.000002"]),
                        group("C3", &["300.000001"]),
                    ],
                    &[c2.clone(), c3.clone()],
                    cx,
                );
                assert_eq!(
                    view.groups
                        .iter()
                        .map(|group| group.channel.clone())
                        .collect::<Vec<_>>(),
                    vec![c2.clone(), c3.clone()],
                    "the dismissed group stays gone; the fresh one joins in unread order"
                );
                assert_eq!(
                    view.groups[0].messages.len(),
                    2,
                    "the shown group took its stored rows"
                );
            });
        });
    }

    /// The quick-reply Send button marks the conversation replied to read, looked up by
    /// identity: a store refresh that lands while the user types reorders the groups, so the
    /// position the button captured may now be another conversation's.
    #[gpui::test]
    fn quick_reply_marks_its_own_conversation_read(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let fixtures = Arc::new(fixtures::load());
        let mounted = std::rc::Rc::new(std::cell::RefCell::new(None));
        let slot = mounted.clone();
        let (_root, cx) = cx.add_window_view(|window, cx| {
            let view = UnreadsView::new(fixtures.clone(), cx);
            *slot.borrow_mut() = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let view = mounted.borrow_mut().take().unwrap();
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = events.clone();
        let recorder = cx.new(|cx| {
            struct Recorder {
                _sub: Subscription,
            }
            let subscription =
                cx.subscribe(&view, move |_, _, event: &UnreadsEvent, _| match event {
                    UnreadsEvent::MarkRead { channel, .. } => {
                        seen.borrow_mut().push(format!("read {channel}"));
                    }
                    UnreadsEvent::Reply { channel, .. } => {
                        seen.borrow_mut().push(format!("reply {channel}"));
                    }
                    _ => {}
                });
            Recorder { _sub: subscription }
        });
        let (c2, c3) = (ChannelId::from("C2"), ChannelId::from("C3"));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.show_local(
                    fixtures.clone(),
                    vec![group("C1", &["100.000001"]), group("C2", &["200.000001"])],
                    cx,
                );
                view.start_reply(c2.clone(), window, cx);
                let input = view.replying.as_ref().unwrap().1.clone();
                input.update(cx, |input, cx| input.set_value("ok", window, cx));
                // C2's Send button captured position 1; a refresh lands meanwhile: C1 was read
                // elsewhere and C3 is new, so C2 now sits first and C3 at position 1.
                view.show_store(
                    vec![group("C2", &["200.000001"]), group("C3", &["300.000001"])],
                    &[c2.clone(), c3.clone()],
                    cx,
                );
                view.submit_reply(cx);
                assert_eq!(
                    view.groups
                        .iter()
                        .map(|group| group.channel.clone())
                        .collect::<Vec<_>>(),
                    vec![c3],
                    "C3 stays unread"
                );
            });
        });
        assert_eq!(
            *events.borrow(),
            ["reply C2", "read C2"],
            "the reply and the mark-read land on the conversation replied to"
        );
        drop(recorder);
    }

    #[gpui::test]
    fn message_actions_mutate_reactions_pins_and_saved_state(cx: &mut TestAppContext) {
        let fixtures = Arc::new(fixtures::load());
        let view = cx.update(|cx| UnreadsView::new(fixtures.clone(), cx));
        let channel = ChannelId::from("C1");
        let ts = Ts::from("100.000001");
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.show_local(fixtures.clone(), vec![group("C1", &["100.000001"])], cx);
                assert_eq!(view.toggle_reaction(&channel, &ts, "eyes", cx), Some(true));
                assert_eq!(view.groups[0].messages[0].reactions.len(), 1);
                assert_eq!(view.toggle_reaction(&channel, &ts, "eyes", cx), Some(false));
                assert!(view.groups[0].messages[0].reactions.is_empty());

                assert_eq!(view.toggle_pin(&channel, &ts, cx), Some(true));
                assert!(view.groups[0].messages[0].pinned_to.contains(&channel));
                assert_eq!(view.toggle_pin(&channel, &ts, cx), Some(false));
                assert!(view.groups[0].messages[0].pinned_to.is_empty());

                let saved: HashSet<Ts> = [ts.clone()].into_iter().collect();
                view.replace_saved(saved, cx);
                assert!(view.saved.contains(&ts));

                view.remove_message(&channel, &ts, cx);
                assert!(view.groups.is_empty());
            });
        });
    }
}
