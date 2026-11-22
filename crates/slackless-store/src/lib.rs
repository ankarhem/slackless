//! Durable local-first SQLite store (rusqlite, bundled -> FTS5): the rendering source of truth.
//!
//! - `message_revision`: immutable, append-only log of every message revision observed, keyed
//!   `(workspace_id, channel_id, ts, edit_ts)`. Edits and deletions are new revisions; triggers
//!   reject UPDATE/DELETE, so retention never removes history (extra-features X-1).
//! - `message_current`: latest revision per message, which the UI reads; `reaction`,
//!   `conversation`, `user` hold current state.
//! - `message_fts`: FTS5 over current message text + file names, synced by triggers; queried
//!   with Slack-style modifiers parsed by [`query`] (X-2).
//! - `team`, `channel_section`, `custom_emoji`, `prefs`: sidebar data (team record, channel
//!   sections, custom emoji, user prefs) hydrated at boot from `client.userBoot`,
//!   `users.channelSections.list` and `emoji.list`.
//!
//! SQLite is blocking: drive [`Store`] from a dedicated thread or `spawn_blocking`.
//! Slack JSON is (de)serialized only through `slackless_types`, whose structs carry the
//! patchability rules (lenient fields, catch-all enums, centralized endpoints).

pub use slackless_types as types;

pub mod query;
mod schema;
mod store;

pub use query::{Date, Has, SearchFilters, SearchQuery, SearchSort};
pub use schema::SCHEMA_VERSION;
pub use store::{MemberCounts, Revision, Store, StoredMessage};

/// Store errors.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("database schema version {found} is newer than this build supports ({supported})")]
    FutureSchema { found: u32, supported: u32 },
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Proves the bundled SQLite has FTS5 compiled in (extra-features X-2).
    #[test]
    fn fts5_available() {
        let store = Store::open_in_memory().unwrap();
        store
            .conn
            .execute_batch(
                "CREATE VIRTUAL TABLE t USING fts5(body);
                 INSERT INTO t(body) VALUES ('hello slackless world');",
            )
            .unwrap();
        let n: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM t WHERE t MATCH 'slackless'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
}
