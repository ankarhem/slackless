//! Local FTS5 search execution, remote Slack query formatting, fixture archive seeding,
//! and client-side channel / people / DM matching.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use gpui_kit::gpui::Context;
use slackless_core::SharedStore;
use slackless_core::api::SearchSort as ApiSearchSort;
use slackless_core::search::{
    WorkspaceSearchTarget, discover_local_archives, search_across_stores, search_disk_archives,
    search_huddle_transcripts_across_stores, search_huddle_transcripts_in_disk_archives,
};
use slackless_core::store::{SearchFilters, SearchQuery, SearchSort, Store};
use slackless_core::types::{Conversation, ConversationKind, TeamId, User};

use super::state::{RESULT_LIMIT, SearchEvent, SearchScope, SearchSource, SearchTab, SearchView};
use crate::fixtures::Fixtures;

impl SearchView {
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
        self.run_local(cx);
        if self.source == SearchSource::Remote && !self.query.is_empty() {
            self.request_remote_messages(1, cx);
        }
        cx.notify();
    }

    /// Re-run the active local FTS5 search (for example after a file transcript finishes indexing).
    pub fn refresh_local(&mut self, cx: &mut Context<Self>) {
        if !self.query.is_empty() {
            self.run_local(cx);
            cx.notify();
        }
    }

    pub(super) fn run_local(&mut self, cx: &mut Context<Self>) {
        self.local_gen = self.local_gen.wrapping_add(1);
        let generation = self.local_gen;
        self.local_results.clear();
        self.transcript_results.clear();
        self.local_error = None;
        let (Some(archive), false) = (self.archive.clone(), self.query.is_empty()) else {
            return;
        };
        let mut parsed = SearchQuery::parse(&self.query);
        parsed
            .filters
            .in_channels
            .extend(self.filters.in_channels.iter().cloned());
        parsed
            .filters
            .exclude_channels
            .extend(self.filters.exclude_channels.iter().cloned());
        if parsed.filters.in_channels.is_empty() {
            parsed
                .filters
                .exclude_channels
                .extend(self.excluded_channels.iter().cloned());
        }
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
        parsed.filters.include_deleted |= self.filters.include_deleted || self.include_deleted;
        let workspace = self.workspace.clone();
        let workspace_name = self
            .fixtures
            .workspaces
            .iter()
            .find(|t| t.id == workspace)
            .map(|t| t.name.clone())
            .filter(|n| !n.trim().is_empty());
        let scope = self.scope;
        let is_sample = self.fixtures.sample;
        let targets: Vec<WorkspaceSearchTarget> = match scope {
            SearchScope::CurrentWorkspace => vec![WorkspaceSearchTarget {
                workspace: workspace.clone(),
                workspace_name,
                store: archive,
            }],
            SearchScope::AllWorkspaces => {
                if self.workspace_sources.is_empty() {
                    vec![WorkspaceSearchTarget {
                        workspace: workspace.clone(),
                        workspace_name,
                        store: archive,
                    }]
                } else {
                    self.workspace_sources
                        .iter()
                        .map(|src| WorkspaceSearchTarget {
                            workspace: src.team_id.clone(),
                            workspace_name: (!src.name.trim().is_empty()).then(|| src.name.clone()),
                            store: src.store.clone(),
                        })
                        .collect()
                }
            }
        };
        let sort = self.sort;
        let query = self.query.clone();
        cx.spawn(async move |this, cx| {
            let (found, transcripts) = cx
                .background_executor()
                .spawn(async move {
                    let mut msgs_res = search_across_stores(&targets, &parsed, sort, RESULT_LIMIT)
                        .map(|r| r.hits)
                        .map_err(|e| e.to_string());
                    let mut trs = search_huddle_transcripts_across_stores(
                        &targets,
                        &query,
                        None,
                        RESULT_LIMIT,
                    );
                    // In live cross-workspace search, also pick up any additional stored `.sqlite`
                    // archives on disk that are not currently mounted as active/parked bridges.
                    if scope == SearchScope::AllWorkspaces && !is_sample {
                        let mounted: HashSet<_> =
                            targets.iter().map(|t| t.workspace.clone()).collect();
                        let extra: Vec<_> = discover_local_archives()
                            .into_iter()
                            .filter(|(ws, _)| !mounted.contains(ws))
                            .collect();
                        if !extra.is_empty() {
                            if let Ok(ref mut hits) = msgs_res
                                && let Ok(disk_res) =
                                    search_disk_archives(&extra, &parsed, sort, RESULT_LIMIT)
                            {
                                hits.extend(disk_res.hits);
                                match sort {
                                    SearchSort::Recent => {
                                        hits.sort_by(|a, b| b.message.ts.cmp(&a.message.ts));
                                    }
                                    SearchSort::Relevant => {
                                        hits.sort_by(|a, b| {
                                            let ra = a.rank.unwrap_or(f64::INFINITY);
                                            let rb = b.rank.unwrap_or(f64::INFINITY);
                                            ra.total_cmp(&rb)
                                                .then_with(|| b.message.ts.cmp(&a.message.ts))
                                        });
                                    }
                                }
                                hits.truncate(RESULT_LIMIT);
                            }
                            let disk_trs = search_huddle_transcripts_in_disk_archives(
                                &extra,
                                &query,
                                None,
                                RESULT_LIMIT,
                            );
                            if !disk_trs.is_empty() {
                                trs.extend(disk_trs);
                                trs.sort_by(|a, b| {
                                    b.segment
                                        .at_ms
                                        .cmp(&a.segment.at_ms)
                                        .then_with(|| b.segment.id.cmp(&a.segment.id))
                                });
                                trs.truncate(RESULT_LIMIT);
                            }
                        }
                    }
                    (msgs_res, trs)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.local_gen != generation {
                    return;
                }
                view.transcript_results = transcripts;
                match found {
                    Ok(results) => view.local_results = results,
                    Err(e) => view.local_error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn api_sort(&self) -> ApiSearchSort {
        match self.sort {
            SearchSort::Recent => ApiSearchSort::Timestamp,
            SearchSort::Relevant => ApiSearchSort::Score,
        }
    }

    /// Build the remote query string, appending any active filter-chip modifiers.
    pub(super) fn remote_query_string(&self) -> String {
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

    pub(super) fn request_remote_messages(&mut self, page: usize, cx: &mut Context<Self>) {
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

    pub(super) fn request_remote_files(&mut self, page: usize, cx: &mut Context<Self>) {
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

    pub(super) fn matching_channels(&self) -> Vec<&Conversation> {
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
            .take(RESULT_LIMIT)
            .collect()
    }

    pub(super) fn matching_people(&self) -> Vec<&User> {
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
            .take(RESULT_LIMIT)
            .collect()
    }

    pub(super) fn matching_dms(&self) -> Vec<&Conversation> {
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
            .take(RESULT_LIMIT)
            .collect()
    }
}

/// Fixture mode's archive: an in-memory store holding the first fixture workspace (users,
/// conversations, history and thread replies), searched exactly like a synced archive.
pub fn fixture_archive(fixtures: &Fixtures) -> Option<SharedStore> {
    let workspace = fixtures.workspaces.first()?.id.clone();
    fixture_archive_for(fixtures, &workspace)
}

/// Fixture mode's archive seeded explicitly under `workspace` (used for workspace index > 0 and
/// cross-workspace sample search so each workspace's messages live under its own `TeamId`).
pub fn fixture_archive_for(fixtures: &Fixtures, workspace: &TeamId) -> Option<SharedStore> {
    let seed = || -> slackless_core::store::Result<Store> {
        let mut store = Store::open_in_memory()?;
        if let Some(team) = fixtures.workspaces.iter().find(|t| t.id == *workspace) {
            store.upsert_team(workspace, team)?;
        }
        for user in &fixtures.users {
            store.upsert_user(workspace, user)?;
        }
        for conversation in fixtures
            .channels
            .iter()
            .chain(&fixtures.dms)
            .chain(&fixtures.directory)
        {
            store.upsert_conversation(workspace, conversation)?;
        }
        let replies = fixtures
            .threads
            .iter()
            .map(|((channel, _), replies)| (channel, replies));
        for (channel, history) in fixtures.messages.iter().chain(replies) {
            for message in history {
                store.upsert_message(workspace, channel, message)?;
            }
        }
        for (file_id, text) in &fixtures.file_transcripts {
            store.upsert_file_transcript(workspace, file_id, "local", text)?;
        }
        Ok(store)
    };
    match seed() {
        Ok(store) => Some(Arc::new(Mutex::new(store))),
        Err(e) => {
            tracing::warn!(error = %e, "could not build the fixture search archive");
            None
        }
    }
}

/// Find the first case-insensitive match for any search term in `query` inside `transcript` and
/// return a ~120-char word-boundary context snippet around it.
pub(crate) fn transcript_match_excerpt(transcript: &str, query: &str) -> Option<String> {
    const WINDOW_CHARS: usize = 120;
    const LEAD_CHARS: usize = 40;
    const WORD_SNAP_LIMIT: usize = 20;

    let parsed = SearchQuery::parse(query);
    let terms: Vec<String> = parsed
        .terms
        .into_iter()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() {
        return None;
    }

    let collapsed = transcript.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }

    let chars: Vec<char> = collapsed.chars().collect();
    // Build a lowercase string paired with each lowercase char's source index in `chars` so
    // Unicode case-folding that expands a char never causes mid-character slicing panics.
    let mut lowered = String::with_capacity(collapsed.len());
    let mut byte_to_char_ix: Vec<usize> = Vec::with_capacity(collapsed.len());
    for (ci, ch) in chars.iter().enumerate() {
        for lc in ch.to_lowercase() {
            lowered.push(lc);
            byte_to_char_ix.resize(lowered.len(), ci);
        }
    }

    let mut best: Option<(usize, usize)> = None;
    for term in &terms {
        if let Some(byte_pos) = lowered.find(term.as_str()) {
            let end_byte = (byte_pos + term.len()).min(lowered.len());
            let start_char = byte_to_char_ix[byte_pos];
            let end_char = if end_byte == 0 {
                start_char
            } else {
                (byte_to_char_ix[end_byte - 1] + 1).min(chars.len())
            };
            if best.is_none_or(|(prev_start, _)| start_char < prev_start) {
                best = Some((start_char, end_char));
            }
        }
    }

    let (match_start, match_end) = best?;
    let total = chars.len();
    if total <= WINDOW_CHARS {
        return Some(collapsed);
    }

    let mut win_start = match_start.saturating_sub(LEAD_CHARS);
    let mut win_end = (win_start + WINDOW_CHARS).min(total);
    if win_end == total {
        win_start = total.saturating_sub(WINDOW_CHARS);
    }

    if win_start > 0 {
        let search_limit = (win_start + WORD_SNAP_LIMIT).min(match_start);
        if let Some(offset) = chars[win_start..search_limit]
            .iter()
            .position(|c| *c == ' ')
        {
            win_start += offset + 1;
        }
    }
    if win_end < total {
        let search_floor = win_end.saturating_sub(WORD_SNAP_LIMIT).max(match_end);
        if let Some(offset) = chars[search_floor..win_end].iter().rposition(|c| *c == ' ') {
            win_end = search_floor + offset;
        }
    }

    let slice: String = chars[win_start..win_end].iter().collect();
    let trimmed = slice.trim();
    let mut out = String::with_capacity(trimmed.len() + 6);
    if win_start > 0 {
        out.push('\u{2026}');
    }
    out.push_str(trimmed);
    if win_end < total {
        out.push('\u{2026}');
    }
    Some(out)
}
