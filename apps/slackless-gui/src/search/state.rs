//! Search view state, enums, events, and configuration / pagination setters.

use std::sync::Arc;

use gpui_kit::gpui::{App, AppContext as _, Context, Entity, EventEmitter};
use slackless_core::SharedStore;
use slackless_core::api::{
    RemoteFilePage, RemoteMessageHit, RemoteMessagePage, SearchSort as ApiSearchSort,
};
use slackless_core::search::{UnifiedSearchHit, UnifiedTranscriptHit};
use slackless_core::store::{SearchFilters, SearchSort};
use slackless_core::types::{ChannelId, File, TeamId, Ts, UserId};

use crate::fixtures::Fixtures;

/// Most local hits listed per query.
pub(super) const RESULT_LIMIT: usize = 50;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchTab {
    Messages,
    Transcripts,
    Files,
    Channels,
    People,
    Dms,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SearchSource {
    Local,
    Remote,
}

/// Whether local FTS5 search targets only the active workspace or all stored workspaces.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SearchScope {
    #[default]
    CurrentWorkspace,
    AllWorkspaces,
}

/// One workspace store registered for cross-workspace local FTS5 search.
#[derive(Clone)]
pub struct SearchWorkspaceSource {
    pub team_id: TeamId,
    pub name: String,
    pub store: SharedStore,
}

pub enum SearchEvent {
    Close,
    Open {
        channel: ChannelId,
        ts: Ts,
    },
    /// Jump to a message in another workspace (switches workspace, selects channel, reveals `ts`).
    OpenInWorkspace {
        workspace: TeamId,
        channel: ChannelId,
        ts: Ts,
    },
    /// Open a channel or DM matched in the Channels / DMs search tabs.
    OpenChannel(ChannelId),
    /// Open a person's profile matched in the People search tab.
    OpenProfile(UserId),
    /// Open a locally archived huddle transcript matched in the Huddles search tab.
    OpenTranscript {
        call_id: String,
        channel: ChannelId,
        highlight_segment_id: Option<i64>,
        highlight_query: Option<String>,
    },
    /// Open a locally archived huddle transcript belonging to another workspace.
    OpenTranscriptInWorkspace {
        workspace: TeamId,
        call_id: String,
        channel: ChannelId,
        highlight_segment_id: Option<i64>,
        highlight_query: Option<String>,
    },
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
    pub(super) fixtures: Arc<Fixtures>,
    pub(super) archive: Option<SharedStore>,
    pub(super) workspace: TeamId,
    /// All known workspace archives (active + parked live workspaces, or all fixture workspaces)
    /// searched when `scope == SearchScope::AllWorkspaces`.
    pub(super) workspace_sources: Vec<SearchWorkspaceSource>,
    pub(super) scope: SearchScope,
    pub(super) query: String,
    pub(super) filters: SearchFilters,
    pub(super) tab: SearchTab,
    pub(super) source: SearchSource,
    pub(super) sort: SearchSort,
    pub(super) remote_auto: bool,
    /// Instant local FTS5 hits (single-workspace or unified cross-workspace).
    pub(super) local_results: Vec<UnifiedSearchHit>,
    pub(super) local_error: Option<String>,
    /// Monotonic counter so stale async local FTS5 queries are discarded when a newer query starts.
    pub(super) local_gen: u64,
    /// Instant local FTS5 huddle transcript hits (text-only; EN + SV diacritic-folding).
    pub(super) transcript_results: Vec<UnifiedTranscriptHit>,
    /// Opt-in remote Slack message hits (`search.modules.messages`).
    pub(super) remote_messages: Vec<RemoteMessageHit>,
    pub(super) remote_messages_total: usize,
    pub(super) remote_messages_page: usize,
    pub(super) remote_messages_pages: usize,
    pub(super) remote_messages_loading: bool,
    pub(super) remote_messages_error: Option<String>,
    /// Opt-in remote Slack file hits (`search.modules.files`).
    pub(super) remote_files: Vec<File>,
    pub(super) remote_files_total: usize,
    pub(super) remote_files_page: usize,
    pub(super) remote_files_pages: usize,
    pub(super) remote_files_loading: bool,
    pub(super) remote_files_error: Option<String>,
    /// Channel names excluded from default local search results (Preferences -> Advanced).
    pub(super) excluded_channels: Vec<String>,
    /// Include locally preserved deleted messages in local FTS5 search results.
    pub(super) include_deleted: bool,
}

impl EventEmitter<SearchEvent> for SearchView {}

impl SearchView {
    pub fn new(
        fixtures: Arc<Fixtures>,
        archive: Option<SharedStore>,
        cx: &mut App,
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
            workspace_sources: Vec::new(),
            scope: SearchScope::CurrentWorkspace,
            query: String::new(),
            filters: SearchFilters::default(),
            tab: SearchTab::Messages,
            source: SearchSource::Local,
            sort: SearchSort::Recent,
            remote_auto: false,
            local_results: Vec::new(),
            local_error: None,
            local_gen: 0,
            transcript_results: Vec::new(),
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
            excluded_channels: Vec::new(),
            include_deleted: false,
        })
    }

    /// Register all available workspace stores (active + parked live stores or sample workspaces)
    /// for cross-workspace local FTS5 search.
    pub fn set_workspace_sources(
        &mut self,
        sources: Vec<SearchWorkspaceSource>,
        cx: &mut Context<Self>,
    ) {
        self.workspace_sources = sources;
        if self.scope == SearchScope::AllWorkspaces && !self.query.is_empty() {
            self.run_local(cx);
        }
        cx.notify();
    }

    /// Switch local FTS5 search scope between the active workspace and all stored workspaces.
    pub fn set_scope(&mut self, scope: SearchScope, cx: &mut Context<Self>) {
        if self.scope == scope {
            return;
        }
        self.scope = scope;
        if scope == SearchScope::AllWorkspaces {
            self.source = SearchSource::Local;
        }
        if !self.query.is_empty() {
            self.run_local(cx);
        }
        cx.notify();
    }

    /// Honor Preferences → Advanced → Search exclusions (comma-separated channel names).
    pub fn set_excluded_channels(&mut self, csv: &str) {
        self.excluded_channels = csv
            .split(',')
            .map(|s| s.trim().trim_start_matches('#').to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
    }

    /// Honor Preferences → Advanced → Privacy & archive ("Show deleted messages").
    pub fn set_include_deleted(&mut self, include: bool) {
        self.include_deleted = include;
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

    pub(super) fn clear_remote(&mut self) {
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

    pub(super) fn set_source(&mut self, source: SearchSource, cx: &mut Context<Self>) {
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

    pub(super) fn set_sort(&mut self, sort: SearchSort, cx: &mut Context<Self>) {
        if self.sort == sort {
            return;
        }
        self.sort = sort;
        self.run_local(cx);
        self.clear_remote();
        if self.tab == SearchTab::Files {
            self.request_remote_files(1, cx);
        } else if self.source == SearchSource::Remote {
            self.request_remote_messages(1, cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn set_tab(&mut self, tab: SearchTab, cx: &mut Context<Self>) {
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
}
