//! In-app search: instant local-first SQLite FTS5 search (`slackless_store::Store::search_sorted`)
//! by default, with opt-in remote Slack search (`search.modules.messages` & `search.modules.files`)
//! when you need older history, unjoined channels, or server-indexed files.
//!
//! `SearchView::new(Arc<Fixtures>, Option<SharedStore>, &mut App) -> Entity<SearchView>`
//! `SearchView::run(&mut self, query: String, &mut Context<Self>)`
//! emits `SearchEvent`.

mod query;
mod render;
mod state;
#[cfg(test)]
mod tests;

pub use query::{fixture_archive, fixture_archive_for};
#[cfg(test)]
pub use state::SearchScope;
pub use state::{SearchEvent, SearchView, SearchWorkspaceSource};
