//! In-app search: the title-bar field queries the local archive through
//! `slackless_store::Store::search` (FTS5 + Slack modifiers, the same path `slackless search`
//! uses in the CLI) and this view lists the hits; clicking one opens its conversation.
//!
//! Live mode searches the store the core keeps current ([`crate::bridge::CoreBridge::store`]).
//! Fixture mode seeds an in-memory store from the fixtures ([`fixture_archive`]), so the
//! search path is the real one either way. No archive, or no hits, renders an empty state.
//!
//! `SearchView::new(Arc<Fixtures>, Option<SharedStore>, &mut App) -> Entity<SearchView>`
//! `SearchView::run(&mut self, query: String, &mut Context<Self>)`
//! emits `SearchEvent::Close` and `SearchEvent::Open(ChannelId)`.

use std::sync::{Arc, Mutex};

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::{
    AppContext as _, Context, Div, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
};
use slackless_core::SharedStore;
use slackless_core::store::{SearchQuery, Store, StoredMessage};
use slackless_core::types::{ChannelId, TeamId};

use crate::fixtures::Fixtures;
use crate::messages::{list_stamp, message_row};

/// Most hits listed per query.
const RESULT_LIMIT: usize = 50;

pub enum SearchEvent {
    Close,
    Open(ChannelId),
}

pub struct SearchView {
    fixtures: Arc<Fixtures>,
    archive: Option<SharedStore>,
    workspace: TeamId,
    query: String,
    results: Vec<StoredMessage>,
    /// A failed query (bad FTS syntax, I/O); shown instead of the results.
    error: Option<String>,
}

impl EventEmitter<SearchEvent> for SearchView {}

impl SearchView {
    pub fn new(
        fixtures: Arc<Fixtures>,
        archive: Option<SharedStore>,
        cx: &mut gpui_kit::gpui::App,
    ) -> Entity<SearchView> {
        let workspace = fixtures
            .workspaces
            .first()
            .map(|team| team.id.clone())
            .unwrap_or_default();
        cx.new(|_| SearchView {
            fixtures,
            archive,
            workspace,
            query: String::new(),
            results: Vec::new(),
            error: None,
        })
    }

    /// Live refresh: the query and its results stay; names come from `fixtures`.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        if let Some(team) = fixtures.workspaces.first() {
            self.workspace = team.id.clone();
        }
        self.fixtures = fixtures;
        cx.notify();
    }

    pub fn run(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query.trim().to_owned();
        self.results.clear();
        self.error = None;
        if let (Some(archive), false) = (&self.archive, self.query.is_empty()) {
            let parsed = SearchQuery::parse(&self.query);
            let found = match archive.lock() {
                Ok(store) => store
                    .search(&self.workspace, &parsed, RESULT_LIMIT)
                    .map_err(|e| e.to_string()),
                Err(_) => Err("the local archive is unavailable".to_owned()),
            };
            match found {
                Ok(results) => self.results = results,
                Err(e) => self.error = Some(e),
            }
        }
        cx.notify();
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, hover) = (theme.muted_foreground, theme.border, theme.accent);
        let summary = match self.results.len() {
            0 => String::new(),
            1 => "1 result".to_owned(),
            n => format!("{n} results"),
        };
        h_flex()
            .h(px(46.))
            .flex_none()
            .px_4()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(border)
            .child(Icon::new(IconName::Search).text_color(muted).size(px(16.)))
            .child(
                div()
                    .truncate()
                    .font_weight(FontWeight::BOLD)
                    .child(format!("Results for \u{201C}{}\u{201D}", self.query)),
            )
            .child(div().text_sm().text_color(muted).child(summary))
            .child(div().flex_1())
            .child(
                div()
                    .id("search-close")
                    .size(px(28.))
                    .flex_none()
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|el| el.bg(hover))
                    .child(Icon::new(IconName::X).text_color(muted).size(px(16.)))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SearchEvent::Close))),
            )
    }

    fn empty_state(&self, cx: &Context<Self>) -> Div {
        let (title, hint) = if self.archive.is_none() {
            (
                "Your local archive is empty".to_owned(),
                "Sign in and sync with the slackless CLI; history you open is archived and searchable here.",
            )
        } else if let Some(error) = &self.error {
            (
                format!("Search failed: {error}"),
                "Check the query syntax and try again.",
            )
        } else if self.query.is_empty() {
            (
                "Search your workspace".to_owned(),
                "Type in the search bar and press Enter.",
            )
        } else {
            (
                format!("No results for \u{201C}{}\u{201D}", self.query),
                "Try fewer words, or narrow with in:#channel from:@user has:file before:2026-01-01.",
            )
        };
        let muted = cx.theme().muted_foreground;
        let tile = cx.theme().muted;
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .p_8()
            .child(
                div()
                    .size(px(56.))
                    .rounded_full()
                    .bg(tile)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::Search).text_color(muted).size(px(24.))),
            )
            .child(div().font_weight(FontWeight::BOLD).child(title))
            .child(div().text_sm().text_color(muted).child(hint))
    }

    fn result_row(
        &self,
        ix: usize,
        hit: &StoredMessage,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, hover) = (theme.muted_foreground, theme.border, theme.accent);
        let label = self
            .fixtures
            .conversation(&hit.channel)
            .map(|c| self.fixtures.conversation_label(c))
            .unwrap_or_else(|| hit.channel.to_string());
        let channel = hit.channel.clone();
        let row = message_row(&self.fixtures, &hit.message, None, cx);
        v_flex()
            .id(("search-hit", ix))
            .gap_1()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(border)
            .cursor_pointer()
            .hover(|el| el.bg(hover))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{label} · {}", list_stamp(&hit.message.ts))),
            )
            .child(row)
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(SearchEvent::Open(channel.clone()));
            }))
    }
}

impl Render for SearchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.header(cx);
        let body = if self.results.is_empty() {
            self.empty_state(cx).into_any_element()
        } else {
            let hits: Vec<_> = self
                .results
                .iter()
                .enumerate()
                .map(|(ix, hit)| self.result_row(ix, hit, cx))
                .collect();
            v_flex()
                .id(SharedString::from("search-results"))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .children(hits)
                .into_any_element()
        };
        v_flex().size_full().child(header).child(body)
    }
}

/// Fixture mode's archive: an in-memory store holding the fixture workspace (users,
/// conversations, history and thread replies), searched exactly like a synced archive.
pub fn fixture_archive(fixtures: &Fixtures) -> Option<SharedStore> {
    let workspace = fixtures.workspaces.first()?.id.clone();
    let seed = || -> slackless_core::store::Result<Store> {
        let mut store = Store::open_in_memory()?;
        for user in &fixtures.users {
            store.upsert_user(&workspace, user)?;
        }
        for conversation in fixtures
            .channels
            .iter()
            .chain(&fixtures.dms)
            .chain(&fixtures.directory)
        {
            store.upsert_conversation(&workspace, conversation)?;
        }
        let replies = fixtures
            .threads
            .iter()
            .map(|((channel, _), replies)| (channel, replies));
        for (channel, history) in fixtures.messages.iter().chain(replies) {
            for message in history {
                store.upsert_message(&workspace, channel, message)?;
            }
        }
        Ok(store)
    };
    match seed() {
        Ok(store) => Some(Arc::new(Mutex::new(store))),
        Err(e) => {
            eprintln!("slackless: could not build the fixture search archive ({e})");
            None
        }
    }
}
