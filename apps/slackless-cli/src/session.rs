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

/// Whether `id` is a plain workspace (team) id. It names `<data>/<id>.sqlite`, so path
/// separators, dots, and spaces are refused.
pub(crate) fn is_valid_workspace_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A workspace id from user input (`--workspace`, an MCP argument), trimmed and validated.
pub(crate) fn validate_workspace_id(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if is_valid_workspace_id(trimmed) {
        Ok(trimmed.to_owned())
    } else {
        Err(format!(
            "invalid `workspace` '{trimmed}': must contain only ASCII letters, digits, '_', or '-'"
        ))
    }
}

/// The workspace local commands read: `--workspace`, else the first stored session's, else the
/// only archive on disk. `None` means nothing is known; local commands then report empty
/// results instead of guessing among several archives.
pub fn resolve_workspace(flag: Option<&str>) -> Option<TeamId> {
    if let Some(flag) = flag {
        return Some(TeamId::from(flag));
    }
    let sessions = SessionStore::new()
        .and_then(|store| store.load())
        .unwrap_or_default();
    if !sessions.is_empty() {
        return resolve_workspace_from(None, &sessions, &[]);
    }
    let archive_ids = archive::stored_team_ids();
    resolve_workspace_from(None, &sessions, &archive_ids)
}

pub(crate) fn resolve_workspace_from(
    flag: Option<&str>,
    sessions: &[Session],
    archive_ids: &[TeamId],
) -> Option<TeamId> {
    if let Some(flag) = flag {
        return Some(TeamId::from(flag));
    }
    if let Some(session) = sessions.first() {
        return Some(session.team_id.clone());
    }
    (archive_ids.len() == 1).then(|| archive_ids[0].clone())
}

/// The session write commands act with: the flagged workspace's, else the first stored one.
pub fn resolve_session(flag: Option<&str>) -> Result<Session, NotSignedIn> {
    let sessions = SessionStore::new()
        .and_then(|store| store.load())
        .map_err(|_| NotSignedIn)?;
    resolve_session_from(flag, &sessions)
}

pub(crate) fn resolve_session_from(
    flag: Option<&str>,
    sessions: &[Session],
) -> Result<Session, NotSignedIn> {
    let session = match flag {
        Some(flag) => sessions
            .iter()
            .find(|session| session.team_id.as_str() == flag),
        None => sessions.first(),
    };
    match session {
        Some(session) if session.has_secrets() => Ok(session.clone()),
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
                .then(|| Archive::open(&session.team_id).ok().flatten())
                .flatten()
                .and_then(|archive| archive.message_count(&session.team_id).ok())
                .unwrap_or(0);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn session(team: &str, token: &str, cookie: &str) -> Session {
        Session::new(
            format!("https://{team}.slack.com/"),
            team,
            token,
            cookie,
            "U1",
        )
    }

    #[test]
    fn resolve_workspace_from_precedence() {
        let s1 = session("T_S1", "xoxc-1", "xoxd-1");
        let s2 = session("T_S2", "xoxc-2", "xoxd-2");
        let a1 = TeamId::from("T_A1");
        let a2 = TeamId::from("T_A2");

        // Flag always wins.
        assert_eq!(
            resolve_workspace_from(
                Some("T_FLAG"),
                std::slice::from_ref(&s1),
                std::slice::from_ref(&a1),
            ),
            Some(TeamId::from("T_FLAG"))
        );
        // Without flag, first stored session wins over archives.
        assert_eq!(
            resolve_workspace_from(None, &[s1.clone(), s2], std::slice::from_ref(&a1)),
            Some(TeamId::from("T_S1"))
        );
        // With no sessions and exactly one archive, picks that archive.
        assert_eq!(
            resolve_workspace_from(None, &[], std::slice::from_ref(&a1)),
            Some(TeamId::from("T_A1"))
        );
        // With no sessions and 0 or >=2 archives, returns None rather than guessing.
        assert_eq!(resolve_workspace_from(None, &[], &[]), None);
        assert_eq!(resolve_workspace_from(None, &[], &[a1, a2]), None);
    }

    #[test]
    fn resolve_session_from_matches_flag_and_requires_secrets() {
        let s1 = session("T1", "xoxc-1", "xoxd-1");
        let s2 = session("T2", "xoxc-2", "xoxd-2");
        let no_token = session("T3", "", "xoxd-3");
        let no_cookie = session("T4", "xoxc-4", "");

        // No flag picks first session when it has secrets.
        assert_eq!(
            resolve_session_from(None, &[s1.clone(), s2.clone()])
                .unwrap()
                .team_id,
            TeamId::from("T1")
        );
        // Flag picks matching session.
        assert_eq!(
            resolve_session_from(Some("T2"), &[s1.clone(), s2.clone()])
                .unwrap()
                .team_id,
            TeamId::from("T2")
        );
        // Unknown flag fails.
        assert!(resolve_session_from(Some("T_NOPE"), std::slice::from_ref(&s1)).is_err());
        // Empty sessions fails.
        assert!(resolve_session_from(None, &[]).is_err());
        // Missing token or cookie is rejected even when matched.
        assert!(resolve_session_from(Some("T3"), &[s1.clone(), no_token.clone()]).is_err());
        assert!(resolve_session_from(Some("T4"), &[s1.clone(), no_cookie]).is_err());
        // If first session lacks secrets and no flag is set, fails.
        assert!(resolve_session_from(None, &[no_token, s1]).is_err());

        assert!(NotSignedIn.to_string().contains("slackless auth login"));
    }
}
