//! The title-bar search pill's dropdown (official Slack's search panel): while the pill has
//! focus it hangs a panel under it, as wide as the pill. With an empty query it lists
//! "Unreads & drafts" (conversations with unread messages or a saved draft, ten rows before
//! a "Show N more" link) and "Recent places" (recently visited conversations). Typing turns
//! the rows into name matches — prefix matches first, ranked by the same helper as the
//! Cmd+K quick switcher — behind a leading "Search for ..." row. Up/Down move the
//! highlight, Enter opens the highlighted conversation or runs the message search, Escape
//! or a click outside folds the panel. The root mounts and positions the panel (the pill
//! wiring in `App::global_bar` and the overlay in `App::render`); this view owns the rows.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, ThemeStyled as _, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, Div, Entity, EventEmitter, Hsla,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Pixels, Render,
    Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::types::{ChannelId, ConversationKind};

use crate::avatars::conversation_avatar;
use crate::fixtures::Fixtures;

/// Unreads & drafts rows shown before a "Show N more" link.
const UNREADS_SHOWN: usize = 10;
/// Recent places rows; the panel stays Slack's size however long the history is.
const RECENTS_SHOWN: usize = 10;
/// Conversation matches listed for a query, like the quick switcher's list.
const MATCHES_SHOWN: usize = 12;

pub enum SearchMenuEvent {
    /// Open a conversation (the quick switcher's `SwitcherEvent::Open`).
    Open(ChannelId),
    /// Run the message search for the pill's text (its Enter, exactly as before).
    Search(String),
    /// Fold the panel and blur the pill (Escape or a click outside it).
    Dismiss,
}

/// The empty-query rows, computed by the root once when the panel opens: conversations
/// with unread messages or a saved draft, then the recently visited ones (newest first).
#[derive(Default)]
pub struct Snapshot {
    pub unreads: Vec<ChannelId>,
    pub recents: Vec<ChannelId>,
}

/// One row of the panel: the query's search row or a conversation.
#[derive(Clone)]
enum Row {
    Search,
    Conversation {
        id: ChannelId,
        label: String,
        kind: ConversationKind,
    },
}

pub struct SearchMenu {
    fixtures: Arc<Fixtures>,
    /// The pill's text, trimmed and lowercased for matching (the switcher's convention).
    query: String,
    /// The pill's trimmed text as typed, for the "Search for ..." row.
    display_query: String,
    /// Empty-query channels, fixed when the panel opens.
    snapshot: Snapshot,
    /// Visible rows in order: search row + matches for a query, else the two sections.
    rows: Vec<Row>,
    /// How many of `rows` belong to the "Unreads & drafts" section (header placement).
    unreads_count: usize,
    /// Unreads folded behind the "Show N more" link.
    hidden_unreads: usize,
    selected: usize,
    expanded: bool,
    /// The pill's window bounds, captured as it paints; the panel hangs under them.
    pill_bounds: Bounds<Pixels>,
}

impl EventEmitter<SearchMenuEvent> for SearchMenu {}

impl SearchMenu {
    pub fn new(fixtures: Arc<Fixtures>, cx: &mut App) -> Entity<Self> {
        cx.new(|_| {
            let mut menu = Self {
                fixtures,
                query: String::new(),
                display_query: String::new(),
                snapshot: Snapshot::default(),
                rows: Vec::new(),
                unreads_count: 0,
                hidden_unreads: 0,
                selected: 0,
                expanded: false,
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

    /// A pill key-down while the panel is open: `true` when the panel consumed the key
    /// (moving the highlight, confirming, dismissing), so the caller stops propagation.
    pub fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        match event.keystroke.key.as_str() {
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

    /// Enter: open the highlighted conversation, or run the message search when the
    /// search row (or nothing) is highlighted — the pill's Enter, exactly as before.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected.min(self.rows.len().saturating_sub(1));
        self.activate(selected, cx);
    }

    /// Open row `ix` (a click or Enter): a conversation row opens it; the search row
    /// searches for the pill's text.
    fn activate(&mut self, ix: usize, cx: &mut Context<Self>) {
        match self.rows.get(ix) {
            Some(Row::Conversation { id, .. }) => cx.emit(SearchMenuEvent::Open(id.clone())),
            _ => cx.emit(SearchMenuEvent::Search(self.display_query.clone())),
        }
    }

    /// The unreads section's "Show N more" link was clicked: list the rest.
    fn expand(&mut self, cx: &mut Context<Self>) {
        self.expanded = true;
        self.refresh();
        cx.notify();
    }

    /// Rebuild the visible rows from the query and snapshot. Called on open, query change,
    /// fixtures swap and expansion — never per frame.
    fn refresh(&mut self) {
        self.unreads_count = 0;
        self.hidden_unreads = 0;
        if self.query.is_empty() {
            let sections = section_conversations(
                &self.snapshot.unreads,
                &self.snapshot.recents,
                self.expanded,
            );
            self.unreads_count = sections.unreads;
            self.hidden_unreads = sections.hidden;
            self.rows = sections
                .ids
                .into_iter()
                .filter_map(|id| {
                    let conversation = self.fixtures.conversation(&id)?;
                    Some(Row::Conversation {
                        id,
                        label: self
                            .fixtures
                            .conversation_label(conversation)
                            .trim_start_matches("# ")
                            .to_owned(),
                        kind: conversation.kind(),
                    })
                })
                .collect();
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
                .filter_map(|(id, label)| {
                    let kind = self.fixtures.conversation(&id)?.kind();
                    Some(Row::Conversation {
                        id,
                        label: label.trim_start_matches("# ").to_owned(),
                        kind,
                    })
                }),
            );
            self.rows = rows;
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    /// One list row: a 24px tile (hash / lock / avatar), the conversation name, and on the
    /// highlighted row an Enter hint at the right edge.
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
        let label = match row {
            Row::Search => format!("Search for \"{}\"", self.display_query),
            Row::Conversation { label, .. } => label.clone(),
        };
        let muted = cx.theme().muted_foreground;
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
                div().flex_1().min_w_0().truncate().child(label),
            )
            .when(on, |row| row.child(enter_hint(muted)))
            .on_click(cx.listener(move |menu, _, _, cx| menu.activate(ix, cx)))
    }

    /// The unreads section's "Show N more" link, row-styled like the switcher's list.
    fn show_more(&self, hidden: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let (muted, hover) = (cx.theme().muted_foreground, cx.theme().accent);
        h_flex()
            .id("search-menu-more")
            .px_3()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_xs()
            .text_color(muted)
            .hover(|el| el.bg(hover))
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
        let (unreads_count, hidden_unreads) = (self.unreads_count, self.hidden_unreads);

        let mut children: Vec<AnyElement> = Vec::new();
        for (ix, row) in self.rows.iter().enumerate() {
            if query_empty && ix == 0 {
                // Both sections can open the list; the first header names whichever does.
                children.push(
                    header(
                        if unreads_count > 0 {
                            "Unreads & drafts"
                        } else {
                            "Recent places"
                        },
                        muted,
                    )
                    .into_any_element(),
                );
            }
            if query_empty && unreads_count > 0 && ix == unreads_count {
                children.push(divider(border).into_any_element());
                children.push(header("Recent places", muted).into_any_element());
            }
            children.push(
                self.row_element(ix, row, ix == selected, cx)
                    .into_any_element(),
            );
            if query_empty && hidden_unreads > 0 && ix + 1 == unreads_count {
                children.push(self.show_more(hidden_unreads, cx).into_any_element());
            }
        }

        v_flex()
            .w_full()
            // The shared popup surface: popover background, ring shadow and radius.
            .popover_style(cx)
            .py_1()
            .child(
                v_flex()
                    .id("search-menu-rows")
                    .max_h(px(560.))
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
                    .text_xs()
                    .text_color(muted)
                    .child("\u{2191} \u{2193} Select"),
            )
    }
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

/// The highlighted row's Enter hint, in the design system's key-cap chip.
/// The highlighted row's hint, the word "Enter" as Slack writes it.
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
}
