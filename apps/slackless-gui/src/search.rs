//! In-app search: instant local-first SQLite FTS5 search (`slackless_store::Store::search_sorted`)
//! by default, with opt-in remote Slack search (`search.modules.messages` & `search.modules.files`)
//! when you need older history, unjoined channels, or server-indexed files.
//!
//! `SearchView::new(Arc<Fixtures>, Option<SharedStore>, &mut App) -> Entity<SearchView>`
//! `SearchView::run(&mut self, query: String, &mut Context<Self>)`
//! emits `SearchEvent`.

use std::sync::{Arc, Mutex};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AppContext as _, Context, Div, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
};
use slackless_core::SharedStore;
use slackless_core::api::{
    RemoteFilePage, RemoteMessageHit, RemoteMessagePage, SearchSort as ApiSearchSort,
};
use slackless_core::store::{SearchFilters, SearchQuery, SearchSort, Store, StoredMessage};
use slackless_core::types::{
    ChannelId, Conversation, ConversationKind, File, TeamId, Ts, User, UserId,
};

use crate::fixtures::Fixtures;
use crate::messages::{list_stamp, message_row};

/// Most local hits listed per query.
const RESULT_LIMIT: usize = 50;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SearchTab {
    Messages,
    Files,
    Channels,
    People,
    Dms,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SearchSource {
    Local,
    Remote,
}

pub enum SearchEvent {
    Close,
    Open {
        channel: ChannelId,
        ts: Ts,
    },
    /// Open a channel or DM matched in the Channels / DMs search tabs.
    OpenChannel(ChannelId),
    /// Open a person's profile matched in the People search tab.
    OpenProfile(UserId),
    /// User opted into remote Slack message search (`search.modules.messages`).
    RemoteMessages {
        query: String,
        sort: ApiSearchSort,
        page: usize,
    },
    /// User opened the Files tab or requested another remote files page (`search.modules.files`).
    RemoteFiles {
        query: String,
        sort: ApiSearchSort,
        page: usize,
    },
}

pub struct SearchView {
    fixtures: Arc<Fixtures>,
    archive: Option<SharedStore>,
    workspace: TeamId,
    query: String,
    filters: SearchFilters,
    tab: SearchTab,
    source: SearchSource,
    sort: SearchSort,
    remote_auto: bool,
    /// Instant local FTS5 hits.
    local_results: Vec<StoredMessage>,
    local_error: Option<String>,
    /// Opt-in remote Slack message hits (`search.modules.messages`).
    remote_messages: Vec<RemoteMessageHit>,
    remote_messages_total: usize,
    remote_messages_page: usize,
    remote_messages_pages: usize,
    remote_messages_loading: bool,
    remote_messages_error: Option<String>,
    /// Opt-in remote Slack file hits (`search.modules.files`).
    remote_files: Vec<File>,
    remote_files_total: usize,
    remote_files_page: usize,
    remote_files_pages: usize,
    remote_files_loading: bool,
    remote_files_error: Option<String>,
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
            filters: SearchFilters::default(),
            tab: SearchTab::Messages,
            source: SearchSource::Local,
            sort: SearchSort::Recent,
            remote_auto: false,
            local_results: Vec::new(),
            local_error: None,
            remote_messages: Vec::new(),
            remote_messages_total: 0,
            remote_messages_page: 0,
            remote_messages_pages: 0,
            remote_messages_loading: false,
            remote_messages_error: None,
            remote_files: Vec::new(),
            remote_files_total: 0,
            remote_files_page: 0,
            remote_files_pages: 0,
            remote_files_loading: false,
            remote_files_error: None,
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

    /// Honor Preferences → Advanced → Search ("Also search Slack remotely by default").
    pub fn set_remote_auto(&mut self, auto: bool) {
        self.remote_auto = auto;
    }

    /// Search messages for `query` (local FTS5 instant by default).
    pub fn run(&mut self, query: String, cx: &mut Context<Self>) {
        self.run_with_filters(query, SearchFilters::default(), cx);
    }

    /// Search for `query` with the search menu's filter chips layered over Slack modifiers.
    pub fn run_with_filters(
        &mut self,
        query: String,
        filters: SearchFilters,
        cx: &mut Context<Self>,
    ) {
        self.query = query.trim().to_owned();
        self.filters = filters;
        self.tab = SearchTab::Messages;
        self.source = if self.remote_auto && !self.fixtures.sample {
            SearchSource::Remote
        } else {
            SearchSource::Local
        };
        self.clear_remote();
        self.run_local();
        if self.source == SearchSource::Remote && !self.query.is_empty() {
            self.request_remote_messages(1, cx);
        }
        cx.notify();
    }

    fn clear_remote(&mut self) {
        self.remote_messages.clear();
        self.remote_messages_total = 0;
        self.remote_messages_page = 0;
        self.remote_messages_pages = 0;
        self.remote_messages_loading = false;
        self.remote_messages_error = None;
        self.remote_files.clear();
        self.remote_files_total = 0;
        self.remote_files_page = 0;
        self.remote_files_pages = 0;
        self.remote_files_loading = false;
        self.remote_files_error = None;
    }

    fn run_local(&mut self) {
        self.local_results.clear();
        self.local_error = None;
        if let (Some(archive), false) = (&self.archive, self.query.is_empty()) {
            let mut parsed = SearchQuery::parse(&self.query);
            parsed
                .filters
                .in_channels
                .extend(self.filters.in_channels.iter().cloned());
            parsed
                .filters
                .exclude_channels
                .extend(self.filters.exclude_channels.iter().cloned());
            parsed
                .filters
                .from_users
                .extend(self.filters.from_users.iter().cloned());
            parsed
                .filters
                .exclude_users
                .extend(self.filters.exclude_users.iter().cloned());
            parsed.filters.has.extend(self.filters.has.iter().copied());
            if parsed.filters.includes_user.is_none() {
                parsed.filters.includes_user = self.filters.includes_user.clone();
            }
            parsed.filters.threads_only |= self.filters.threads_only;
            let found = match archive.lock() {
                Ok(store) => store
                    .search_sorted(&self.workspace, &parsed, self.sort, RESULT_LIMIT)
                    .map_err(|e| e.to_string()),
                Err(_) => Err("the local archive is unavailable".to_owned()),
            };
            match found {
                Ok(results) => self.local_results = results,
                Err(e) => self.local_error = Some(e),
            }
        }
    }

    fn api_sort(&self) -> ApiSearchSort {
        match self.sort {
            SearchSort::Recent => ApiSearchSort::Timestamp,
            SearchSort::Relevant => ApiSearchSort::Score,
        }
    }

    /// Build the remote query string, appending any active filter-chip modifiers.
    fn remote_query_string(&self) -> String {
        let mut out = self.query.clone();
        for ch in &self.filters.in_channels {
            if !out.is_empty() {
                out.push(' ');
            }
            if ch.starts_with('C') || ch.starts_with('G') || ch.starts_with('D') {
                out.push_str(&format!("in:<#{ch}>"));
            } else {
                out.push_str(&format!("in:#{ch}"));
            }
        }
        for u in &self.filters.from_users {
            if !out.is_empty() {
                out.push(' ');
            }
            if u.starts_with('U') || u.starts_with('W') {
                out.push_str(&format!("from:<@{u}>"));
            } else {
                out.push_str(&format!("from:@{u}"));
            }
        }
        if let Some(u) = &self.filters.includes_user {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&format!("with:<@{u}>"));
        }
        if self.filters.threads_only {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str("is:thread");
        }
        out
    }

    fn request_remote_messages(&mut self, page: usize, cx: &mut Context<Self>) {
        if self.fixtures.sample || self.query.is_empty() {
            return;
        }
        let query = self.remote_query_string();
        self.remote_messages_loading = true;
        self.remote_messages_error = None;
        cx.emit(SearchEvent::RemoteMessages {
            query,
            sort: self.api_sort(),
            page,
        });
        cx.notify();
    }

    fn request_remote_files(&mut self, page: usize, cx: &mut Context<Self>) {
        if self.fixtures.sample || self.query.is_empty() {
            return;
        }
        let query = self.remote_query_string();
        self.remote_files_loading = true;
        self.remote_files_error = None;
        cx.emit(SearchEvent::RemoteFiles {
            query,
            sort: self.api_sort(),
            page,
        });
        cx.notify();
    }

    /// Receive a page from `search.modules.messages`.
    pub fn set_remote_messages(
        &mut self,
        query: &str,
        sort: ApiSearchSort,
        page: usize,
        result: Result<RemoteMessagePage, String>,
        cx: &mut Context<Self>,
    ) {
        if query != self.remote_query_string() || sort != self.api_sort() {
            return;
        }
        self.remote_messages_loading = false;
        match result {
            Ok(data) => {
                if page <= 1 {
                    self.remote_messages = data.hits;
                } else {
                    self.remote_messages.extend(data.hits);
                }
                self.remote_messages_total = data.total;
                self.remote_messages_page = data.page;
                self.remote_messages_pages = data.page_count;
                self.remote_messages_error = None;
            }
            Err(err) => {
                self.remote_messages_error = Some(err);
            }
        }
        cx.notify();
    }

    /// Receive a page from `search.modules.files`.
    pub fn set_remote_files(
        &mut self,
        query: &str,
        sort: ApiSearchSort,
        page: usize,
        result: Result<RemoteFilePage, String>,
        cx: &mut Context<Self>,
    ) {
        if query != self.remote_query_string() || sort != self.api_sort() {
            return;
        }
        self.remote_files_loading = false;
        match result {
            Ok(data) => {
                if page <= 1 {
                    self.remote_files = data.files;
                } else {
                    self.remote_files.extend(data.files);
                }
                self.remote_files_total = data.total;
                self.remote_files_page = data.page;
                self.remote_files_pages = data.page_count;
                self.remote_files_error = None;
            }
            Err(err) => {
                self.remote_files_error = Some(err);
            }
        }
        cx.notify();
    }

    fn set_source(&mut self, source: SearchSource, cx: &mut Context<Self>) {
        if self.source == source {
            return;
        }
        self.source = source;
        if source == SearchSource::Remote
            && self.remote_messages.is_empty()
            && !self.remote_messages_loading
            && !self.query.is_empty()
        {
            self.request_remote_messages(1, cx);
        } else {
            cx.notify();
        }
    }

    fn set_sort(&mut self, sort: SearchSort, cx: &mut Context<Self>) {
        if self.sort == sort {
            return;
        }
        self.sort = sort;
        self.run_local();
        self.clear_remote();
        if self.tab == SearchTab::Files {
            self.request_remote_files(1, cx);
        } else if self.source == SearchSource::Remote {
            self.request_remote_messages(1, cx);
        } else {
            cx.notify();
        }
    }

    fn set_tab(&mut self, tab: SearchTab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        if tab == SearchTab::Files
            && self.remote_files.is_empty()
            && !self.remote_files_loading
            && !self.query.is_empty()
        {
            self.request_remote_files(1, cx);
        } else {
            cx.notify();
        }
    }

    fn matching_channels(&self) -> Vec<&Conversation> {
        let needle = self.query.trim().to_lowercase();
        let mut seen = std::collections::HashSet::new();
        self.fixtures
            .channels
            .iter()
            .chain(&self.fixtures.directory)
            .filter(|c| {
                matches!(
                    c.kind(),
                    ConversationKind::PublicChannel | ConversationKind::PrivateChannel
                )
            })
            .filter(|c| seen.insert(c.id.clone()))
            .filter(|c| {
                if needle.is_empty() {
                    return true;
                }
                let name = c.name.as_deref().unwrap_or("").to_lowercase();
                let topic = c
                    .topic
                    .as_ref()
                    .map(|t| t.value.to_lowercase())
                    .unwrap_or_default();
                let purpose = c
                    .purpose
                    .as_ref()
                    .map(|p| p.value.to_lowercase())
                    .unwrap_or_default();
                name.contains(&needle) || topic.contains(&needle) || purpose.contains(&needle)
            })
            .take(50)
            .collect()
    }

    fn matching_people(&self) -> Vec<&User> {
        let needle = self.query.trim().to_lowercase();
        self.fixtures
            .users
            .iter()
            .filter(|u| !u.deleted && !u.is_bot && u.id.as_str() != "USLACKBOT")
            .filter(|u| {
                if needle.is_empty() {
                    return true;
                }
                let name = self.fixtures.name_of(u).to_lowercase();
                let handle = u.name.to_lowercase();
                let title = u.profile.title.as_deref().unwrap_or("").to_lowercase();
                let email = u.profile.email.as_deref().unwrap_or("").to_lowercase();
                name.contains(&needle)
                    || handle.contains(&needle)
                    || title.contains(&needle)
                    || email.contains(&needle)
            })
            .take(50)
            .collect()
    }

    fn matching_dms(&self) -> Vec<&Conversation> {
        let needle = self.query.trim().to_lowercase();
        self.fixtures
            .dms
            .iter()
            .filter(|c| {
                if needle.is_empty() {
                    return true;
                }
                self.fixtures
                    .conversation_label(c)
                    .to_lowercase()
                    .contains(&needle)
            })
            .take(50)
            .collect()
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, hover, foreground, link, secondary) = (
            theme.muted_foreground,
            theme.border,
            theme.accent,
            theme.foreground,
            theme.link,
            theme.secondary,
        );
        let summary = match (self.tab, self.source) {
            (SearchTab::Messages, SearchSource::Local) => match self.local_results.len() {
                0 => "Local · instant".to_owned(),
                1 => "1 local hit".to_owned(),
                n => format!("{n} local hits"),
            },
            (SearchTab::Messages, SearchSource::Remote) => {
                if self.remote_messages_loading && self.remote_messages.is_empty() {
                    "Searching Slack…".to_owned()
                } else if self.remote_messages_total > 0 {
                    format!("{} on Slack", self.remote_messages_total)
                } else {
                    format!("{} on Slack", self.remote_messages.len())
                }
            }
            (SearchTab::Files, _) => {
                if self.remote_files_loading && self.remote_files.is_empty() {
                    "Searching Slack files…".to_owned()
                } else if self.remote_files_total > 0 {
                    format!("{} files on Slack", self.remote_files_total)
                } else {
                    format!("{} files", self.remote_files.len())
                }
            }
            (SearchTab::Channels, _) => format!("{} channels", self.matching_channels().len()),
            (SearchTab::People, _) => format!("{} people", self.matching_people().len()),
            (SearchTab::Dms, _) => format!("{} DMs", self.matching_dms().len()),
        };

        let pill = |id: &'static str, label: &'static str, active: bool| {
            div()
                .id(id)
                .px_2()
                .py(px(3.))
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .when(active, |el| {
                    el.bg(secondary)
                        .text_color(foreground)
                        .font_weight(FontWeight::SEMIBOLD)
                })
                .when(!active, |el| {
                    el.text_color(muted).hover(|h| h.text_color(foreground))
                })
                .child(label)
        };

        let tab_btn = |id: &'static str, label: &'static str, active: bool| {
            div()
                .id(id)
                .h_full()
                .flex()
                .items_center()
                .px_1()
                .cursor_pointer()
                .text_sm()
                .text_color(if active { foreground } else { muted })
                .when(active, |t| {
                    t.font_weight(FontWeight::SEMIBOLD)
                        .border_b_2()
                        .border_color(link)
                })
                .hover(|h| h.text_color(foreground))
                .child(label)
        };

        v_flex()
            .flex_none()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .h(px(46.))
                    .px_4()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::Search).text_color(muted).size(px(16.)))
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::BOLD)
                            .child(format!("Results for \u{201C}{}\u{201D}", self.query)),
                    )
                    .child(div().text_xs().text_color(muted).child(summary))
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
                    ),
            )
            .child(
                h_flex()
                    .h(px(36.))
                    .px_4()
                    .gap_4()
                    .items_center()
                    .child(
                        h_flex()
                            .h_full()
                            .gap_4()
                            .child(
                                tab_btn(
                                    "search-tab-messages",
                                    "Messages",
                                    self.tab == SearchTab::Messages,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_tab(SearchTab::Messages, cx);
                                    },
                                )),
                            )
                            .when(!self.fixtures.sample, |tabs| {
                                tabs.child(
                                    tab_btn(
                                        "search-tab-files",
                                        "Files",
                                        self.tab == SearchTab::Files,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.set_tab(SearchTab::Files, cx);
                                        },
                                    )),
                                )
                            })
                            .child(
                                tab_btn(
                                    "search-tab-channels",
                                    "Channels",
                                    self.tab == SearchTab::Channels,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_tab(SearchTab::Channels, cx);
                                    },
                                )),
                            )
                            .child(
                                tab_btn(
                                    "search-tab-people",
                                    "People",
                                    self.tab == SearchTab::People,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_tab(SearchTab::People, cx);
                                    },
                                )),
                            )
                            .child(
                                tab_btn("search-tab-dms", "DMs", self.tab == SearchTab::Dms)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_tab(SearchTab::Dms, cx);
                                    })),
                            ),
                    )
                    .child(div().flex_1())
                    .when(
                        self.tab == SearchTab::Messages && !self.fixtures.sample,
                        |bar| {
                            bar.child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .p(px(2.))
                                    .rounded_md()
                                    .border_1()
                                    .border_color(border)
                                    .child(
                                        pill(
                                            "search-src-local",
                                            "Local (instant)",
                                            self.source == SearchSource::Local,
                                        )
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.set_source(SearchSource::Local, cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        pill(
                                            "search-src-remote",
                                            "Slack (remote)",
                                            self.source == SearchSource::Remote,
                                        )
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.set_source(SearchSource::Remote, cx);
                                            }),
                                        ),
                                    ),
                            )
                        },
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                pill(
                                    "search-sort-recent",
                                    "Most recent",
                                    self.sort == SearchSort::Recent,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_sort(SearchSort::Recent, cx);
                                    },
                                )),
                            )
                            .child(
                                pill(
                                    "search-sort-relevant",
                                    "Most relevant",
                                    self.sort == SearchSort::Relevant,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_sort(SearchSort::Relevant, cx);
                                    },
                                )),
                            ),
                    ),
            )
    }

    fn empty_state(&self, cx: &mut Context<Self>) -> Div {
        let (title, hint, offer_remote) = match (self.tab, self.source) {
            (SearchTab::Messages, SearchSource::Local) => {
                if self.archive.is_none() {
                    (
                        "Your local archive is empty".to_owned(),
                        "Messages you open are cached locally for instant offline search.",
                        !self.fixtures.sample && !self.query.is_empty(),
                    )
                } else if let Some(error) = &self.local_error {
                    (
                        format!("Local search failed: {error}"),
                        "Check the query syntax and try again.",
                        false,
                    )
                } else if self.query.is_empty() {
                    (
                        "Search your workspace".to_owned(),
                        "Type in the search bar and press Enter. Supports in:#ch, from:@user, -exclude, \"phrase\", has:file.",
                        false,
                    )
                } else {
                    (
                        format!("No local matches for \u{201C}{}\u{201D}", self.query),
                        "Local search only checks channels you've synced. Search Slack remotely for older history, unjoined channels, and files.",
                        !self.fixtures.sample,
                    )
                }
            }
            (SearchTab::Messages, SearchSource::Remote) => {
                if self.remote_messages_loading {
                    (
                        format!("Searching Slack for \u{201C}{}\u{201D}…", self.query),
                        "Querying Slack's full server index across workspace history and channels.",
                        false,
                    )
                } else if let Some(err) = &self.remote_messages_error {
                    (
                        format!("Remote search failed: {err}"),
                        "Switch back to Local for instant offline results.",
                        false,
                    )
                } else {
                    (
                        format!("No Slack matches for \u{201C}{}\u{201D}", self.query),
                        "Try fewer words, or narrow with in:#channel, from:@user, or -exclude.",
                        false,
                    )
                }
            }
            (SearchTab::Files, _) => {
                if self.remote_files_loading {
                    (
                        format!("Searching Slack files for \u{201C}{}\u{201D}…", self.query),
                        "Searching server-indexed file names and contents.",
                        false,
                    )
                } else if let Some(err) = &self.remote_files_error {
                    (
                        format!("File search failed: {err}"),
                        "Try again in a moment.",
                        false,
                    )
                } else {
                    (
                        format!("No files matching \u{201C}{}\u{201D}", self.query),
                        "Try a filename, extension, or keyword.",
                        false,
                    )
                }
            }
            (SearchTab::Channels | SearchTab::People | SearchTab::Dms, _) => {
                ("No matches".to_owned(), "Try another query.", false)
            }
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
            .when(offer_remote, |col| {
                col.child(
                    div().pt_2().child(
                        Button::new("search-empty-remote")
                            .primary()
                            .with_size(Size::Small)
                            .icon(IconName::Globe)
                            .label(format!(
                                "Search Slack remotely for \u{201C}{}\u{201D}",
                                self.query
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_source(SearchSource::Remote, cx);
                            })),
                    ),
                )
            })
    }

    fn message_hit_row(
        &self,
        ix: usize,
        channel: &ChannelId,
        channel_name: Option<&str>,
        message: &slackless_core::types::Message,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, hover) = (theme.muted_foreground, theme.border, theme.accent);
        let label = self
            .fixtures
            .conversation(channel)
            .map(|c| self.fixtures.conversation_label(c))
            .or_else(|| channel_name.map(|n| format!("#{n}")))
            .unwrap_or_else(|| channel.to_string());
        let open_channel = channel.clone();
        let ts = message.ts.clone();
        let row = message_row(&self.fixtures, message, None, cx);
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
                    .child(format!("{label} · {}", list_stamp(&message.ts))),
            )
            .child(row)
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(SearchEvent::Open {
                    channel: open_channel.clone(),
                    ts: ts.clone(),
                });
            }))
    }

    fn file_hit_row(&self, ix: usize, file: &File, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, hover, foreground) = (
            theme.muted_foreground,
            theme.border,
            theme.accent,
            theme.foreground,
        );
        let title = file
            .title
            .clone()
            .or_else(|| file.name.clone())
            .unwrap_or_else(|| file.id.to_string())
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&");
        let filename = file
            .name
            .clone()
            .unwrap_or_else(|| format!("{}.bin", file.id))
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&");
        let owner = file
            .user
            .as_ref()
            .map(|u| self.fixtures.user_name(u))
            .unwrap_or("Someone");
        let kind = file
            .pretty_type
            .as_deref()
            .or(file.filetype.as_deref())
            .unwrap_or("File");
        let size = file
            .size
            .map(|b| crate::composer::format_bytes(b.max(0) as usize))
            .unwrap_or_default();
        let meta = if size.is_empty() {
            format!("{kind} · Shared by {owner}")
        } else {
            format!("{kind} · {size} · Shared by {owner}")
        };
        let download_url = file
            .url_private_download
            .clone()
            .or_else(|| file.url_private.clone());
        let permalink = file.permalink.clone();
        h_flex()
            .id(("search-file", ix))
            .items_center()
            .gap_3()
            .px_4()
            .py_2p5()
            .border_b_1()
            .border_color(border)
            .hover(|el| el.bg(hover))
            .child(
                Icon::new(IconName::FileText)
                    .text_color(muted)
                    .size(px(18.))
                    .flex_none(),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(foreground)
                            .child(title),
                    )
                    .child(div().truncate().text_xs().text_color(muted).child(meta)),
            )
            .when_some(download_url, |row, url| {
                let name = filename.clone();
                row.child(
                    Button::new(("search-file-dl", ix))
                        .ghost()
                        .with_size(Size::Small)
                        .icon(IconName::Download)
                        .tooltip("Download")
                        .on_click(move |_, window, cx| {
                            crate::media::download(url.clone(), name.clone(), window, cx);
                        }),
                )
            })
            .when_some(permalink, |row, url| {
                row.child(
                    Button::new(("search-file-open", ix))
                        .ghost()
                        .with_size(Size::Small)
                        .icon(IconName::ExternalLink)
                        .tooltip("Open in Slack")
                        .on_click(move |_, _, cx| {
                            cx.open_url(&url);
                        }),
                )
            })
    }
}

impl Render for SearchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (muted, border, hover, fg, bg) = {
            let theme = cx.theme();
            (
                theme.muted_foreground,
                theme.border,
                theme.accent,
                theme.foreground,
                theme.background,
            )
        };
        let header = self.header(cx);
        let body = match (self.tab, self.source) {
            (SearchTab::Messages, SearchSource::Local) => {
                if self.local_results.is_empty() {
                    self.empty_state(cx).into_any_element()
                } else {
                    let hits: Vec<_> = self
                        .local_results
                        .iter()
                        .enumerate()
                        .map(|(ix, hit)| {
                            self.message_hit_row(ix, &hit.channel, None, &hit.message, cx)
                        })
                        .collect();
                    let query_for_cta = self.query.clone();
                    v_flex()
                        .id(SharedString::from("search-results"))
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .children(hits)
                        .when(!self.fixtures.sample && !query_for_cta.is_empty(), |list| {
                            list.child(
                                h_flex()
                                    .px_4()
                                    .py_3()
                                    .items_center()
                                    .justify_between()
                                    .border_t_1()
                                    .border_color(border)
                                    .child(
                                        div().text_xs().text_color(muted).child(
                                            "Showing instant local results. Need older history or unjoined channels?",
                                        ),
                                    )
                                    .child(
                                        Button::new("search-footer-remote")
                                            .outline()
                                            .with_size(Size::Small)
                                            .icon(IconName::Globe)
                                            .label("Search Slack remotely")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.set_source(SearchSource::Remote, cx);
                                            })),
                                    ),
                            )
                        })
                        .into_any_element()
                }
            }
            (SearchTab::Messages, SearchSource::Remote) => {
                if self.remote_messages.is_empty() {
                    self.empty_state(cx).into_any_element()
                } else {
                    let hits: Vec<_> = self
                        .remote_messages
                        .iter()
                        .enumerate()
                        .map(|(ix, hit)| {
                            self.message_hit_row(
                                ix,
                                &hit.channel,
                                hit.channel_name.as_deref(),
                                &hit.message,
                                cx,
                            )
                        })
                        .collect();
                    let has_more = self.remote_messages_page < self.remote_messages_pages;
                    let next_page = self.remote_messages_page + 1;
                    let loading = self.remote_messages_loading;
                    v_flex()
                        .id(SharedString::from("search-results-remote"))
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .children(hits)
                        .when(has_more, |list| {
                            list.child(
                                h_flex().justify_center().py_3().child(
                                    Button::new("search-remote-more")
                                        .outline()
                                        .with_size(Size::Small)
                                        .label(if loading {
                                            "Loading…"
                                        } else {
                                            "Load more from Slack"
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if !this.remote_messages_loading {
                                                this.request_remote_messages(next_page, cx);
                                            }
                                        })),
                                ),
                            )
                        })
                        .into_any_element()
                }
            }
            (SearchTab::Files, _) => {
                if self.remote_files.is_empty() {
                    self.empty_state(cx).into_any_element()
                } else {
                    let rows: Vec<_> = self
                        .remote_files
                        .iter()
                        .enumerate()
                        .map(|(ix, file)| self.file_hit_row(ix, file, cx))
                        .collect();
                    let has_more = self.remote_files_page < self.remote_files_pages;
                    let next_page = self.remote_files_page + 1;
                    let loading = self.remote_files_loading;
                    v_flex()
                        .id(SharedString::from("search-files-remote"))
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .children(rows)
                        .when(has_more, |list| {
                            list.child(
                                h_flex().justify_center().py_3().child(
                                    Button::new("search-files-more")
                                        .outline()
                                        .with_size(Size::Small)
                                        .label(if loading {
                                            "Loading…"
                                        } else {
                                            "Load more files"
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if !this.remote_files_loading {
                                                this.request_remote_files(next_page, cx);
                                            }
                                        })),
                                ),
                            )
                        })
                        .into_any_element()
                }
            }
            (SearchTab::Channels, _) => {
                let channels: Vec<Conversation> =
                    self.matching_channels().into_iter().cloned().collect();
                if channels.is_empty() {
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child("No matching channels"),
                        )
                        .into_any_element()
                } else {
                    v_flex()
                        .id(SharedString::from("search-channels-list"))
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .children(channels.into_iter().enumerate().map(|(ix, ch)| {
                            let id = ch.id.clone();
                            let icon = if ch.is_private {
                                IconName::Lock
                            } else {
                                IconName::Hash
                            };
                            let name = ch.name.clone().unwrap_or_else(|| ch.id.to_string());
                            let desc = ch
                                .purpose
                                .as_ref()
                                .map(|p| p.value.clone())
                                .filter(|s| !s.is_empty())
                                .or_else(|| ch.topic.as_ref().map(|t| t.value.clone()))
                                .unwrap_or_default();
                            h_flex()
                                .id(("search-ch", ix))
                                .items_center()
                                .gap_3()
                                .px_4()
                                .py_2p5()
                                .border_b_1()
                                .border_color(border)
                                .cursor_pointer()
                                .hover(|el| el.bg(hover))
                                .child(Icon::new(icon).size(px(16.)).text_color(muted))
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .truncate()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(fg)
                                                .child(name),
                                        )
                                        .when(!desc.is_empty(), |col| {
                                            col.child(
                                                div()
                                                    .truncate()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .child(desc),
                                            )
                                        }),
                                )
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(SearchEvent::OpenChannel(id.clone()));
                                }))
                        }))
                        .into_any_element()
                }
            }
            (SearchTab::People, _) => {
                let people: Vec<User> = self.matching_people().into_iter().cloned().collect();
                if people.is_empty() {
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child("No matching people"),
                        )
                        .into_any_element()
                } else {
                    v_flex()
                        .id(SharedString::from("search-people-list"))
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .children(people.into_iter().enumerate().map(|(ix, user)| {
                            let uid = user.id.clone();
                            let name = self.fixtures.name_of(&user).to_owned();
                            let title = user.profile.title.clone().unwrap_or_default();
                            let avatar = crate::avatars::user_chip(&self.fixtures, &uid, 32.);
                            h_flex()
                                .id(("search-person", ix))
                                .items_center()
                                .gap_3()
                                .px_4()
                                .py_2()
                                .border_b_1()
                                .border_color(border)
                                .cursor_pointer()
                                .hover(|el| el.bg(hover))
                                .child(avatar)
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .truncate()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(fg)
                                                .child(name),
                                        )
                                        .when(!title.is_empty(), |col| {
                                            col.child(
                                                div()
                                                    .truncate()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .child(title),
                                            )
                                        }),
                                )
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(SearchEvent::OpenProfile(uid.clone()));
                                }))
                        }))
                        .into_any_element()
                }
            }
            (SearchTab::Dms, _) => {
                let dms: Vec<Conversation> = self.matching_dms().into_iter().cloned().collect();
                if dms.is_empty() {
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .child(div().text_sm().text_color(muted).child("No matching DMs"))
                        .into_any_element()
                } else {
                    v_flex()
                        .id(SharedString::from("search-dms-list"))
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .children(dms.into_iter().enumerate().map(|(ix, dm)| {
                            let id = dm.id.clone();
                            let label = self.fixtures.conversation_label(&dm);
                            let avatar = crate::avatars::conversation_avatar(
                                &self.fixtures,
                                &dm,
                                28.,
                                bg,
                                cx,
                            );
                            h_flex()
                                .id(("search-dm", ix))
                                .items_center()
                                .gap_3()
                                .px_4()
                                .py_2()
                                .border_b_1()
                                .border_color(border)
                                .cursor_pointer()
                                .hover(|el| el.bg(hover))
                                .child(avatar)
                                .child(
                                    div()
                                        .flex_1()
                                        .truncate()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg)
                                        .child(label),
                                )
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(SearchEvent::OpenChannel(id.clone()));
                                }))
                        }))
                        .into_any_element()
                }
            }
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
