//! Session resolution: which workspace and stored credentials the CLI acts with.

use slackless_core::auth::{Session, SessionStore};
use slackless_core::types::{TeamId, UserId};

use crate::archive::{self, Archive};

/// No stored session (or one without secrets): writes are unavailable until sign-in.
#[derive(Debug)]
pub struct NotSignedIn;

impl std::fmt::Display for NotSignedIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("not signed in: no stored session with credentials; run `slackless auth login`")
    }
}

/// The workspace local commands read: `--workspace`, else the first stored session's, else the
/// only archive on disk. `None` means nothing is known; local commands then report empty
/// results instead of guessing among several archives.
pub fn resolve_workspace(flag: Option<&str>) -> Option<TeamId> {
    if let Some(flag) = flag {
        return Some(TeamId::from(flag));
    }
    if let Ok(sessions) = SessionStore::new().and_then(|store| store.load()) {
        if let Some(session) = sessions.first() {
            return Some(session.team_id.clone());
        }
    }
    let mut ids = archive::stored_team_ids();
    (ids.len() == 1).then(|| ids.remove(0))
}

/// The session write commands act with: the flagged workspace's, else the first stored one.
pub fn resolve_session(flag: Option<&str>) -> Result<Session, NotSignedIn> {
    let sessions = SessionStore::new()
        .and_then(|store| store.load())
        .map_err(|_| NotSignedIn)?;
    let session = match flag {
        Some(flag) => sessions
            .into_iter()
            .find(|session| session.team_id.as_str() == flag),
        None => sessions.into_iter().next(),
    };
    match session {
        Some(session) if session.has_secrets() => Ok(session),
        _ => Err(NotSignedIn),
    }
}

/// One stored session plus its archive summary, for `auth status`.
pub struct StatusEntry {
    pub team_id: TeamId,
    pub workspace_url: String,
    pub user_id: UserId,
    pub has_secrets: bool,
    pub archive_path: String,
    pub archive_exists: bool,
    pub messages: u64,
}

/// Every stored session with its local archive summary; empty when nothing is stored.
pub fn status_entries() -> Vec<StatusEntry> {
    let Ok(sessions) = SessionStore::new().and_then(|store| store.load()) else {
        return Vec::new();
    };
    sessions
        .into_iter()
        .map(|session| {
            let path = archive::store_path(&session.team_id);
            let archive_exists = path.as_ref().is_some_and(|path| path.is_file());
            let messages = archive_exists
                .then(|| Archive::open(&session.team_id))
                .flatten()
                .map_or(0, |archive| archive.message_count(&session.team_id));
            StatusEntry {
                team_id: session.team_id.clone(),
                workspace_url: session.workspace_url.clone(),
                user_id: session.user_id.clone(),
                has_secrets: session.has_secrets(),
                archive_path: path.as_ref().map_or_else(
                    || "(no home directory)".to_owned(),
                    |path| path.display().to_string(),
                ),
                archive_exists,
                messages,
            }
        })
        .collect()
}
