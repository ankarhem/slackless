//! The title-bar search pill's dropdown (official Slack's search panel): while the pill has
//! focus it hangs a panel over it, exactly the pill's width, whose first row is the search
//! field itself flanked by the filter and history toggles (accent-tinted while on, each with
//! Slack's tooltip). With the filter toggle on, a wrapping row of chips narrows the message
//! search the panel's Enter runs (the store applies them as SQL WHERE clauses). With an
//! empty query the body lists "Recent searches" (the history toggle on; what the user ran in
//! slackless, most recent first) or "Unreads & drafts" (conversations with unread messages or
//! a saved draft, the Threads row once followed threads have unread replies, mention counts in
//! Slack's badge) and "Recent places" (recently visited conversations). Typing turns the rows
//! into name matches — prefix matches first, ranked by the same helper as the Cmd+K quick
//! switcher — behind a leading "Search for ..." row. Up/Down move the highlight, Enter opens
//! the highlighted row or runs the message search, Escape or a click outside folds the panel.
//! The root mounts and positions the panel (the pill wiring in `App::global_bar` and the
//! overlay in `App::render`); this view owns the rows.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Toggle;
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, Size, ThemeStyled as _, h_flex, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, Div, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, Keystroke, ParentElement as _, Pixels, Render, Stateful,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::store::{Has, SearchFilters};
use slackless_core::types::{ChannelId, ConversationKind, UserId};

use crate::avatars::conversation_avatar;
use crate::fixtures::Fixtures;

/// Unreads & drafts rows shown before a "Show N more" link.
const UNREADS_SHOWN: usize = 10;
/// Recent places rows; the panel stays Slack's size however long the history is.
const RECENTS_SHOWN: usize = 10;
/// Conversation matches listed for a query, like the quick switcher's list.
const MATCHES_SHOWN: usize = 12;
/// Recent searches listed while the history toggle is on.
const HISTORY_SHOWN: usize = 10;
/// Searches kept (newest first, deduped) behind the shown slice.
const HISTORY_KEEP: usize = 20;

pub enum SearchMenuEvent {
    /// Open a conversation (the quick switcher's `SwitcherEvent::Open`).
    Open(ChannelId),
    /// Open the Threads view (the "Threads" row of "Unreads & drafts").
    OpenThreads,
    /// Run the message search for the pill's text (its Enter, exactly as before).
    Search(String),
    /// The history toggle changed; the root persists it.
    SetHistoryShown(bool),
    /// The filter toggle changed; the root persists it.
    SetFiltersShown(bool),
    /// Fold the panel and blur the pill (Escape or a click outside it).
    Dismiss,
}

/// The empty-query rows, computed by the root once when the panel opens: conversations
/// with unread messages or a saved draft, then the recently visited ones (newest first),
/// with the sidebar's unread-mention counts and followed-threads state.
#[derive(Default)]
pub struct Snapshot {
    pub unreads: Vec<ChannelId>,
    pub recents: Vec<ChannelId>,
    /// Per conversation unread-mention counts (conversations with any only).
    pub mentions: Vec<(ChannelId, u32)>,
    /// Followed threads have unread replies, and how many of them mention the user.
    pub threads: (bool, u32),
}

/// The filter chips' state ("In this channel", "From me", "Includes me", "Has a link",
/// "Threads only"): which constraints the next message search carries.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Filters {
    pub in_channel: bool,
    pub from_me: bool,
    pub includes_me: bool,
    pub has_link: bool,
    pub threads_only: bool,
}

impl Filters {
    /// The store's SQL-bound filters for a run in `channel` by `me`.
    pub fn search_filters(self, channel: &ChannelId, me: &UserId) -> SearchFilters {
        let mut filters = SearchFilters::default();
        if self.in_channel {
            filters.in_channels.push(channel.as_str().to_owned());
        }
        if self.from_me {
            filters.from_users.push(me.as_str().to_owned());
        }
        if self.includes_me {
            filters.includes_user = Some(me.as_str().to_owned());
        }
        if self.has_link {
            filters.has.push(Has::Link);
        }
        filters.threads_only = self.threads_only;
        filters
    }
}

/// One row of the panel: the query's search row, the Threads view's row, a recorded
/// search, or a conversation.
#[derive(Clone)]
enum Row {
    Search,
    Threads,
    History(String),
    Conversation {
        id: ChannelId,
        label: String,
        kind: ConversationKind,
        /// The peer's handle for DM rows (rendered as the muted ` · @handle` suffix;
        /// the self-DM's label already carries " (you)").
        handle: Option<String>,
        /// The conversation's unread mentions (the badge before the Enter hint).
        mentions: u32,
    },
}

pub struct SearchMenu {
    fixtures: Arc<Fixtures>,
    /// The pill's field, rendered in the panel's first row while the panel is open.
    search: Entity<TextareaState>,
    /// The pill's text, trimmed and lowercased for matching (the switcher's convention).
    query: String,
    /// The pill's trimmed text as typed, for the "Search for ..." row.
    display_query: String,
    /// Empty-query channels, fixed when the panel opens.
    snapshot: Snapshot,
    /// Visible rows in order: search row + matches for a query, else the sections.
    rows: Vec<Row>,
    /// How many rows belong to the "Unreads & drafts" section (header placement).
    unreads_count: usize,
    /// Unreads folded behind the "Show N more" link.
    hidden_unreads: usize,
    /// The "Threads" row sits in the section (followed threads have unread replies).
    has_threads_row: bool,
    selected: usize,
    expanded: bool,
    /// Searches the user ran here, newest first, deduped, at most [`HISTORY_KEEP`].
    history: Vec<String>,
    /// The history (clock) toggle: the empty-query body lists the searches ran here.
    show_history: bool,
    /// The filter toggle: a row of chips narrows the message search.
    show_filters: bool,
    filters: Filters,
    /// The pill's window bounds, captured as it paints; the panel hangs over them.
    pill_bounds: Bounds<Pixels>,
}

impl EventEmitter<SearchMenuEvent> for SearchMenu {}

impl SearchMenu {
    pub fn new(
        fixtures: Arc<Fixtures>,
        search: Entity<TextareaState>,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|_| {
            let mut menu = Self {
                fixtures,
                search,
                query: String::new(),
                display_query: String::new(),
                snapshot: Snapshot::default(),
                rows: Vec::new(),
                unreads_count: 0,
                hidden_unreads: 0,
                has_threads_row: false,
                selected: 0,
                expanded: false,
                history: Vec::new(),
                show_history: false,
                show_filters: false,
                filters: Filters::default(),
                pill_bounds: Bounds::default(),
            };
            menu.refresh();
            menu
        })
    }

    /// Live refresh: match the query against the new snapshot's conversations.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        self.refresh();
        cx.notify();
    }

    /// Open the panel with the root-computed empty-query rows and the pill's current text.
    pub fn open(&mut self, snapshot: Snapshot, query: &str, cx: &mut Context<Self>) {
        self.snapshot = snapshot;
        self.expanded = false;
        self.selected = 0;
        let trimmed = query.trim();
        self.query = trimmed.to_lowercase();
        self.display_query = trimmed.to_owned();
        self.refresh();
        cx.notify();
    }

    /// The pill's text changed while the panel is open: rebuild the rows once.
    pub fn set_query(&mut self, query: &str, cx: &mut Context<Self>) {
        let trimmed = query.trim();
        if trimmed == self.display_query && trimmed.to_lowercase() == self.query {
            return;
        }
        self.query = trimmed.to_lowercase();
        self.display_query = trimmed.to_owned();
        self.selected = 0;
        self.refresh();
        cx.notify();
    }

    /// Record the pill's window bounds. Called from its paint every frame — stores only,
    /// so rows keep being computed on open and query change alone.
    pub fn set_pill_bounds(&mut self, bounds: Bounds<Pixels>) {
        self.pill_bounds = bounds;
    }

    pub fn pill_bounds(&self) -> Bounds<Pixels> {
        self.pill_bounds
    }

    /// The searches ran here, newest first (the root persists them after a run).
    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// The searches restored from the workspace's local state at launch.
    pub fn set_history(&mut self, history: Vec<String>, cx: &mut Context<Self>) {
        self.history = history;
        self.refresh();
        cx.notify();
    }

    /// The history toggle's state (restored from the workspace's local state at launch).
    pub fn history_shown(&self) -> bool {
        self.show_history
    }

    pub fn set_history_shown(&mut self, on: bool, cx: &mut Context<Self>) {
        self.show_history = on;
        self.selected = 0;
        self.refresh();
        cx.notify();
    }

    /// The filter toggle's state (restored from the workspace's local state at launch).
    pub fn filters_shown(&self) -> bool {
        self.show_filters
    }

    pub fn set_filters_shown(&mut self, on: bool, cx: &mut Context<Self>) {
        self.show_filters = on;
        cx.notify();
    }

    /// The chips' constraints for a message search run in `channel` by `me`.
    pub fn search_filters(&self, channel: &ChannelId, me: &UserId) -> SearchFilters {
        self.filters.search_filters(channel, me)
    }

    /// A message search ran: record its query in the history (newest first, deduped,
    /// capped), so the history toggle lists it.
    pub fn record_search(&mut self, query: &str, cx: &mut Context<Self>) {
        self.history = record_history(std::mem::take(&mut self.history), query);
        cx.notify();
    }

    /// A keystroke in the panel's field (the root routes them here ahead of the field's own
    /// key bindings): `true` when the panel consumed it (moving the highlight, confirming,
    /// dismissing), so the caller stops it there. Modified keys stay the field's.
    pub fn handle_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let modifiers = &keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.shift || modifiers.platform {
            return false;
        }
        match keystroke.key.as_str() {
            "down" => self.move_selection(1, cx),
            "up" => self.move_selection(-1, cx),
            "enter" => {
                self.confirm(cx);
                true
            }
            "escape" => {
                cx.emit(SearchMenuEvent::Dismiss);
                true
            }
            _ => false,
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let count = self.rows.len();
        if count == 0 {
            return false;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(count as isize) as usize;
        cx.notify();
        true
    }

    /// Enter: open the highlighted row — a conversation, the Threads view, or the message
    /// search for the pill's text (the search row, a recorded search, or nothing).
    fn confirm(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected.min(self.rows.len().saturating_sub(1));
        self.activate(selected, cx);
    }

    /// Open row `ix` (a click or Enter).
    fn activate(&mut self, ix: usize, cx: &mut Context<Self>) {
        match self.rows.get(ix) {
            Some(Row::Conversation { id, .. }) => cx.emit(SearchMenuEvent::Open(id.clone())),
            Some(Row::Threads) => cx.emit(SearchMenuEvent::OpenThreads),
            Some(Row::History(query)) => cx.emit(SearchMenuEvent::Search(query.clone())),
            _ => cx.emit(SearchMenuEvent::Search(self.display_query.clone())),
        }
    }

    /// The unreads section's "Show N more" link was clicked: list the rest.
    fn expand(&mut self, cx: &mut Context<Self>) {
        self.expanded = true;
        self.refresh();
        cx.notify();
    }

    /// Rebuild the visible rows from the query, toggles and snapshot. Called on open,
    /// query change, toggle flip, fixtures swap and expansion — never per frame.
    fn refresh(&mut self) {
        self.unreads_count = 0;
        self.hidden_unreads = 0;
        self.has_threads_row = false;
        if self.query.is_empty() && self.show_history {
            self.rows = self
                .history
                .iter()
                .take(HISTORY_SHOWN)
                .map(|query| Row::History(query.clone()))
                .collect();
        } else if self.query.is_empty() {
            let sections = section_conversations(
                &self.snapshot.unreads,
                &self.snapshot.recents,
                self.expanded,
            );
            self.unreads_count = sections.unreads;
            self.hidden_unreads = sections.hidden;
            let mut rows: Vec<Row> = sections
                .ids
                .iter()
                .filter_map(|id| self.conversation_row(id))
                .collect();
            // Slack's Threads row follows the section's first row once followed threads
            // have unread replies (and leads it when nothing else is unread).
            self.has_threads_row = self.snapshot.threads.0;
            if self.has_threads_row {
                let at = if rows.is_empty() {
                    0
                } else {
                    1.min(rows.len())
                };
                rows.insert(at, Row::Threads);
            }
            self.rows = rows;
        } else {
            let mut rows = Vec::with_capacity(MATCHES_SHOWN + 1);
            rows.push(Row::Search);
            rows.extend(
                rank_matches(
                    &self.query,
                    labeled_conversations(&self.fixtures),
                    MATCHES_SHOWN,
                )
                .into_iter()
                .filter_map(|(id, _)| self.conversation_row(&id)),
            );
            self.rows = rows;
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    /// A conversation row's parts (label, kind, DM handle, unread mentions), computed once
    /// per refresh from the fixtures and the open-time snapshot.
    fn conversation_row(&self, id: &ChannelId) -> Option<Row> {
        let conversation = self.fixtures.conversation(id)?;
        let kind = conversation.kind();
        let mut label = self
            .fixtures
            .conversation_label(conversation)
            .trim_start_matches("# ")
            .to_owned();
        let handle = match kind {
            ConversationKind::Im => {
                let peer = conversation
                    .user
                    .clone()
                    .unwrap_or_else(|| self.fixtures.self_user.clone());
                if peer == self.fixtures.self_user {
                    label.push_str(" (you)");
                }
                self.fixtures
                    .user(&peer)
                    .map(|user| user.name.clone())
                    .filter(|handle| !handle.is_empty())
            }
            _ => None,
        };
        let mentions = self
            .snapshot
            .mentions
            .iter()
            .find(|(mentioned, _)| mentioned == id)
            .map_or(0, |(_, count)| *count);
        Some(Row::Conversation {
            id: id.clone(),
            label,
            kind,
            handle,
            mentions,
        })
    }

    /// The panel's first row: the search icon, the pill's field, and the filter and
    /// history toggles (accent-tinted while on, with Slack's tooltips).
    fn input_row(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let this = cx.entity();
        let filters = self.filters_shown();
        let history = self.history_shown();
        let filter_toggle = icon_toggle(
            "search-menu-filter-toggle",
            IconName::SlidersHorizontal,
            filters,
            "Show filters",
            "Hide filters",
            {
                let this = this.clone();
                move |on, _, cx: &mut App| {
                    this.update(cx, |menu, cx| {
                        menu.set_filters_shown(*on, cx);
                        cx.emit(SearchMenuEvent::SetFiltersShown(*on));
                    })
                }
            },
            cx,
        );
        let history_toggle = icon_toggle(
            "search-menu-history-toggle",
            IconName::Clock,
            history,
            "Show search history",
            "Hide search history",
            move |on, _, cx: &mut App| {
                this.update(cx, |menu, cx| {
                    menu.set_history_shown(*on, cx);
                    cx.emit(SearchMenuEvent::SetHistoryShown(*on));
                })
            },
            cx,
        );
        h_flex()
            .items_center()
            .gap_2()
            .px_3()
            .h(px(40.))
            .flex_none()
            .child(Icon::new(IconName::Search).text_color(muted).size(px(16.)))
            .child(
                div().flex_1().min_w_0().text_sm().child(
                    Textarea::new(&self.search)
                        .appearance(false)
                        .bordered(false),
                ),
            )
            .child(
                // Pressing a toggle keeps focus in the field, as in Slack: the panel folds
                // when the field blurs, and GPUI focuses a pressed focusable element
                // unless the press's default is prevented, which this does before the
                // press reaches the toggles. Their clicks still fire.
                h_flex()
                    .flex_none()
                    .gap_2()
                    .capture_any_mouse_down(|_, window, _| window.prevent_default())
                    .child(filter_toggle)
                    .child(history_toggle),
            )
    }

    /// The chips row under the input row while the filter toggle is on. Each chip toggles
    /// one constraint of the next message search.
    fn chips_row(&self, cx: &mut Context<Self>) -> Div {
        let this = cx.entity();
        let chip = |id: &'static str,
                    icon: IconName,
                    label: &'static str,
                    on: bool,
                    toggle: fn(&mut Filters),
                    cx: &mut Context<Self>| {
            let this = this.clone();
            filter_chip(
                id,
                icon,
                label,
                on,
                move |cx: &mut App| {
                    this.update(cx, |menu, cx| {
                        toggle(&mut menu.filters);
                        cx.notify();
                    })
                },
                cx,
            )
            .into_any_element()
        };
        let filters = self.filters;
        let chips = vec![
            chip(
                "chip-in-channel",
                IconName::Hash,
                "In this channel",
                filters.in_channel,
                |filters| filters.in_channel = !filters.in_channel,
                cx,
            ),
            chip(
                "chip-from-me",
                IconName::UserRound,
                "From me",
                filters.from_me,
                |filters| filters.from_me = !filters.from_me,
                cx,
            ),
            chip(
                "chip-includes-me",
                IconName::AtSign,
                "Includes me",
                filters.includes_me,
                |filters| filters.includes_me = !filters.includes_me,
                cx,
            ),
            chip(
                "chip-has-link",
                IconName::Link,
                "Has a link",
                filters.has_link,
                |filters| filters.has_link = !filters.has_link,
                cx,
            ),
            chip(
                "chip-threads-only",
                IconName::MessageSquareText,
                "Threads only",
                filters.threads_only,
                |filters| filters.threads_only = !filters.threads_only,
                cx,
            ),
        ];
        h_flex()
            .flex_wrap()
            .gap_1()
            .px_3()
            .py_1p5()
            .flex_none()
            .children(chips)
    }

    /// One list row: a 24px tile (hash / lock / avatar / clock / threads glyph), the
    /// conversation name (DMs with their ` · @handle` suffix), unread-mention badge and,
    /// on the highlighted row, an Enter hint at the right edge.
    fn row_element(&self, ix: usize, row: &Row, on: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (muted, hover, popover) = (theme.muted_foreground, theme.accent, theme.popover);
        let leading = match row {
            Row::Search => div()
                .size(px(24.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(IconName::Search).text_color(muted).size(px(16.))),
            Row::Threads | Row::History(_) => div()
                .size(px(24.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(match row {
                        Row::Threads => IconName::MessageSquareText,
                        _ => IconName::Clock,
                    })
                    .text_color(muted)
                    .size(px(16.)),
                ),
            Row::Conversation { id, kind, .. } => match kind {
                ConversationKind::PublicChannel => channel_tile(IconName::Hash, cx),
                ConversationKind::PrivateChannel => channel_tile(IconName::Lock, cx),
                ConversationKind::Im | ConversationKind::Mpim => self
                    .fixtures
                    .conversation(id)
                    .map(|conversation| {
                        let ring = if on { hover } else { popover };
                        conversation_avatar(&self.fixtures, conversation, 24., ring, cx)
                    })
                    .unwrap_or_else(div),
            },
        };
        let (label, suffix) = match row {
            Row::Search => (format!("Search for \"{}\"", self.display_query), None),
            Row::Threads => ("Threads".to_owned(), None),
            Row::History(query) => (query.clone(), None),
            Row::Conversation { label, handle, .. } => (
                label.clone(),
                handle.as_ref().map(|handle| format!(" · @{handle}")),
            ),
        };
        let mentions = match row {
            Row::Conversation { mentions, .. } => *mentions,
            _ => 0,
        };
        h_flex()
            .id(("search-menu-row", ix))
            .items_center()
            .gap_2()
            .px_3()
            .py_1p5()
            .rounded_md()
            .cursor_pointer()
            .when(on, |row| row.bg(hover))
            .when(!on, |row| row.hover(|el| el.bg(hover)))
            .child(leading)
            .child(
                // Slack's rows are regular weight, unread or not.
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(div().truncate().child(label))
                    .when_some(suffix, |name, suffix| {
                        name.child(div().flex_none().truncate().text_color(muted).child(suffix))
                    }),
            )
            .when(mentions > 0, |row| row.child(mention_pill(mentions, cx)))
            .when(on, |row| row.child(enter_hint(muted)))
            .on_click(cx.listener(move |menu, _, _, cx| menu.activate(ix, cx)))
    }

    /// The unreads section's "Show N more" link, in the app's link color like Slack's.
    fn show_more(&self, hidden: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (link, link_hover, hover) = (theme.link, theme.link_hover, theme.accent);
        h_flex()
            .id("search-menu-more")
            .px_3()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_xs()
            .text_color(link)
            .hover(|el| el.bg(hover).text_color(link_hover))
            .child(format!("Show {hidden} more"))
            .on_click(cx.listener(|menu, _, _, cx| menu.expand(cx)))
    }
}

impl Render for SearchMenu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let selected = self.selected.min(self.rows.len().saturating_sub(1));
        let query_empty = self.query.is_empty();
        let history_mode = query_empty && self.show_history;
        let (unreads_count, hidden_unreads, has_threads_row) = (
            self.unreads_count,
            self.hidden_unreads,
            self.has_threads_row,
        );
        // The last flat index of the "Unreads & drafts" section (its conversation rows
        // plus Slack's Threads row), and whether it opens the list at all.
        let unreads_end = unreads_count + usize::from(has_threads_row);
        let unreads_section = unreads_end > 0;

        let mut children: Vec<AnyElement> = Vec::new();
        for (ix, row) in self.rows.iter().enumerate() {
            if query_empty && ix == 0 {
                children.push(
                    header(
                        if history_mode {
                            "Recent searches"
                        } else if unreads_section {
                            "Unreads & drafts"
                        } else {
                            "Recent places"
                        },
                        muted,
                    )
                    .into_any_element(),
                );
            }
            if query_empty && !history_mode && unreads_count > 0 && ix == unreads_end {
                children.push(divider(border).into_any_element());
                children.push(header("Recent places", muted).into_any_element());
            }
            children.push(
                self.row_element(ix, row, ix == selected, cx)
                    .into_any_element(),
            );
            if query_empty && !history_mode && hidden_unreads > 0 && ix + 1 == unreads_end {
                children.push(self.show_more(hidden_unreads, cx).into_any_element());
            }
        }

        v_flex()
            .w_full()
            // The shared popup surface: popover background, ring shadow and radius.
            .popover_style(cx)
            .child(self.input_row(cx))
            .when(self.show_filters, |panel| panel.child(self.chips_row(cx)))
            .child(divider(border))
            .child(
                v_flex()
                    .id("search-menu-rows")
                    .max_h(px(520.))
                    .overflow_y_scroll()
                    .gap_0p5()
                    .px_1()
                    .children(children),
            )
            .when(!self.rows.is_empty(), |panel| panel.child(divider(border)))
            .child(
                h_flex()
                    .px_3()
                    .py_1p5()
                    .gap_1()
                    .items_center()
                    .text_xs()
                    .text_color(muted)
                    .child(key_cap(IconName::ArrowUp, cx))
                    .child(key_cap(IconName::ArrowDown, cx))
                    .child("Select"),
            )
    }
}

/// The panel's on/off icon toggle (the input row's filter and history buttons): a quiet
/// glyph at rest, the accent fill with white foreground while on, Slack's tooltip either
/// way.
fn icon_toggle(
    id: &'static str,
    icon: IconName,
    on: bool,
    off_label: &'static str,
    on_label: &'static str,
    on_click: impl Fn(&bool, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Toggle {
    let theme = cx.theme();
    let (fg, bg) = if on {
        (theme.sidebar_accent_foreground, theme.sidebar_accent)
    } else {
        (theme.muted_foreground, theme.transparent)
    };
    Toggle::new(id)
        .with_size(Size::Small)
        .icon(Icon::new(icon).text_color(fg).size(px(16.)))
        .tooltip(if on { on_label } else { off_label })
        .checked(on)
        .when(on, |toggle| toggle.bg(bg))
        .on_click(on_click)
}

/// One filter chip: a bordered pill with a glyph and its label; selected chips take the
/// link tint, like Slack's active chips.
fn filter_chip(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    on: bool,
    on_click: impl Fn(&mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let (fg, bg, border, hover_bg) = if on {
        (
            theme.link,
            theme.link.opacity(0.12),
            theme.link,
            theme.link.opacity(0.2),
        )
    } else {
        (
            theme.foreground,
            theme.transparent,
            theme.border,
            theme.accent,
        )
    };
    h_flex()
        .id(id)
        .items_center()
        .gap_1p5()
        .px_2p5()
        .h(px(26.))
        .rounded(theme.radius)
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .text_xs()
        .cursor_pointer()
        .hover(|el| el.bg(hover_bg))
        .child(Icon::new(icon).text_color(fg).size(px(12.)))
        .child(label)
        .on_click(move |_, _, cx: &mut App| on_click(cx))
}

/// The footer's key-cap chip: a tiny bordered tile around an arrow glyph.
fn key_cap(icon: IconName, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .border_1()
        .border_color(theme.border)
        .size(px(18.))
        .child(
            Icon::new(icon)
                .text_color(theme.muted_foreground)
                .size(px(10.)),
        )
}

/// The unread-mention pill of an "Unreads & drafts" row, in the same badge the sidebar
/// paints its mention counts with (`theme::mention_badge`).
fn mention_pill(count: u32, cx: &App) -> Div {
    let (bg, fg) = crate::theme::mention_badge(cx);
    h_flex()
        .flex_none()
        .justify_center()
        .min_w(px(18.))
        .px_1p5()
        .py_0p5()
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .child(count.to_string())
}

/// Every joined conversation with its sidebar/header label (`# name` / peer names).
fn labeled_conversations(
    fixtures: &Fixtures,
) -> impl Iterator<Item = (ChannelId, String)> + use<'_> {
    fixtures
        .dms
        .iter()
        .chain(&fixtures.channels)
        .map(|conversation| {
            (
                conversation.id.clone(),
                fixtures.conversation_label(conversation),
            )
        })
}

/// Conversations whose label matches `query` (lowercase): name-prefix matches first, then
/// substring matches, at most `limit` of them. Shared with the quick switcher's Cmd+K list.
pub(crate) fn rank_matches(
    query: &str,
    conversations: impl Iterator<Item = (ChannelId, String)>,
    limit: usize,
) -> Vec<(ChannelId, String)> {
    let key = |label: &str| label.trim_start_matches("# ").to_lowercase();
    let (mut prefix, mut rest): (Vec<_>, Vec<_>) = conversations
        .filter(|(_, label)| key(label).contains(query))
        .partition(|(_, label)| key(label).starts_with(query));
    prefix.append(&mut rest);
    prefix.truncate(limit);
    prefix
}

/// The empty-query listing: unreads first (all of them once `expanded`, otherwise the
/// first [`UNREADS_SHOWN`]), then the recents not already listed, capped at
/// [`RECENTS_SHOWN`]. `unreads` is the header split, `hidden` the count behind the
/// "Show N more" link.
pub(crate) struct Sections {
    pub ids: Vec<ChannelId>,
    pub unreads: usize,
    pub hidden: usize,
}

pub(crate) fn section_conversations(
    unreads: &[ChannelId],
    recents: &[ChannelId],
    expanded: bool,
) -> Sections {
    let shown = if expanded {
        unreads.len()
    } else {
        unreads.len().min(UNREADS_SHOWN)
    };
    let mut ids: Vec<ChannelId> = unreads[..shown].to_vec();
    let mut taken = 0;
    for id in recents {
        if taken == RECENTS_SHOWN {
            break;
        }
        if !ids.contains(id) {
            ids.push(id.clone());
            taken += 1;
        }
    }
    Sections {
        unreads: shown,
        hidden: unreads.len() - shown,
        ids,
    }
}

/// The search history after recording `query`: deduped case-insensitively, moved to the
/// front, capped at [`HISTORY_KEEP`].
pub(crate) fn record_history(history: Vec<String>, query: &str) -> Vec<String> {
    let mut history: Vec<String> = history
        .into_iter()
        .filter(|saved| !saved.eq_ignore_ascii_case(query))
        .collect();
    history.insert(0, query.to_owned());
    history.truncate(HISTORY_KEEP);
    history
}

/// A conversation's 24px rounded tile: the hash / lock icon on a muted chip, at the
/// panel's row scale (the quick switcher's leading icon).
fn channel_tile(icon: IconName, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .size(px(24.))
        .flex_none()
        .rounded(theme.radius)
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.muted)
        .child(
            Icon::new(icon)
                .text_color(theme.muted_foreground)
                .size(px(14.)),
        )
}

/// The highlighted row's Enter hint, the word "Enter" as Slack writes it.
fn enter_hint(color: Hsla) -> Div {
    div().flex_none().text_xs().text_color(color).child("Enter")
}

/// A section's small gray header ("Unreads & drafts" / "Recent places").
fn header(label: &'static str, color: Hsla) -> Div {
    div()
        .px_3()
        .pt_1p5()
        .pb_1()
        .text_xs()
        .text_color(color)
        .child(label)
}

/// A hairline between the sections and above the footer (the canvas divider convention).
fn divider(color: Hsla) -> Div {
    div().mx_2().h(px(1.)).bg(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_rank_prefixes_before_substrings() {
        let items = vec![
            (ChannelId::new("C1"), "# engineering".to_owned()),
            (ChannelId::new("C2"), "messenger".to_owned()),
            (ChannelId::new("C3"), "# engine".to_owned()),
            (ChannelId::new("C4"), "Engine room".to_owned()),
        ];
        let matched = rank_matches("eng", items.into_iter(), 10);
        assert_eq!(
            matched
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            ["C1", "C3", "C4", "C2"]
        );
    }

    #[test]
    fn matches_are_capped() {
        let items = (0..5)
            .map(|i| (ChannelId::new(format!("C{i}")), format!("room {i}")))
            .collect::<Vec<_>>();
        assert_eq!(rank_matches("room", items.into_iter(), 2).len(), 2);
    }

    #[test]
    fn unreads_show_ten_then_more() {
        let unreads: Vec<ChannelId> = (0..13).map(|i| ChannelId::new(format!("C{i}"))).collect();
        let recents = vec![ChannelId::new("C0"), ChannelId::new("R1")];

        let sections = section_conversations(&unreads, &recents, false);
        assert_eq!(sections.ids.len(), 11); // 10 unreads + the one recent not listed
        assert_eq!(sections.unreads, 10);
        assert_eq!(sections.hidden, 3);
        assert_eq!(sections.ids.last().unwrap().as_str(), "R1");

        let expanded = section_conversations(&unreads, &recents, true);
        assert_eq!(expanded.ids.len(), 14);
        assert_eq!(expanded.unreads, 13);
        assert_eq!(expanded.hidden, 0);
    }

    #[test]
    fn recents_are_capped_and_deduped() {
        let recents: Vec<ChannelId> = (0..20).map(|i| ChannelId::new(format!("R{i}"))).collect();
        let sections = section_conversations(&[], &recents, false);
        assert_eq!(sections.ids.len(), RECENTS_SHOWN);
        assert_eq!(sections.unreads, 0);
        assert_eq!(sections.hidden, 0);

        let duped = vec![ChannelId::new("R0"), ChannelId::new("R0")];
        assert_eq!(section_conversations(&[], &duped, false).ids.len(), 1);
    }

    #[test]
    fn history_dedupes_moves_and_caps() {
        let history = vec![
            "ipa".to_owned(),
            "jensie".to_owned(),
            "paypal cli".to_owned(),
        ];
        let recorded = record_history(history, "JENSIE");
        assert_eq!(recorded, ["JENSIE", "ipa", "paypal cli"]);

        let kept = (0..30).map(|i| format!("q{i}")).collect::<Vec<_>>();
        let recorded = record_history(kept, "new");
        assert_eq!(recorded.len(), HISTORY_KEEP);
        assert_eq!(recorded[0], "new");
        assert_eq!(record_history(Vec::new(), "first"), ["first"]);
    }

    #[test]
    fn chips_map_to_store_filters() {
        let (channel, me) = (ChannelId::new("C1"), UserId::new("U1"));
        assert_eq!(
            Filters::default().search_filters(&channel, &me),
            SearchFilters::default()
        );

        let all = Filters {
            in_channel: true,
            from_me: true,
            includes_me: true,
            has_link: true,
            threads_only: true,
        }
        .search_filters(&channel, &me);
        assert_eq!(all.in_channels, vec!["C1"]);
        assert_eq!(all.from_users, vec!["U1"]);
        assert_eq!(all.includes_user.as_deref(), Some("U1"));
        assert_eq!(all.has, vec![Has::Link]);
        assert!(all.threads_only);
    }
}
