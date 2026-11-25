//! Local FTS5 search execution, remote Slack query formatting, fixture archive seeding,
//! and client-side channel / people / DM matching.

use std::sync::{Arc, Mutex};

use gpui_kit::gpui::Context;
use slackless_core::SharedStore;
use slackless_core::api::SearchSort as ApiSearchSort;
use slackless_core::store::{SearchFilters, SearchQuery, SearchSort, Store};
use slackless_core::types::{Conversation, ConversationKind, User};

use super::state::{RESULT_LIMIT, SearchEvent, SearchSource, SearchTab, SearchView};
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
        let sort = self.sort;
        let query = self.query.clone();
        cx.spawn(async move |this, cx| {
            let (found, transcripts) = cx
                .background_executor()
                .spawn(async move {
                    match archive.lock() {
                        Ok(store) => {
                            let msgs = store
                                .search_sorted(&workspace, &parsed, sort, RESULT_LIMIT)
                                .map_err(|e| e.to_string());
                            let trs = store
                                .search_huddle_transcripts(
                                    &workspace,
                                    &query,
                                    None,
                                    None,
                                    RESULT_LIMIT,
                                )
                                .unwrap_or_default();
                            (msgs, trs)
                        }
                        Err(_) => (
                            Err("the local archive is unavailable".to_owned()),
                            Vec::new(),
                        ),
                    }
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
            tracing::warn!(error = %e, "could not build the fixture search archive");
            None
        }
    }
}
