//! The persisted session record.

use serde::{Deserialize, Serialize};
use slackless_types::{TeamId, UserId};

/// One signed-in Slack web-client session for a single workspace.
///
/// This is exactly the credential set worth persisting (`docs/research/api-auth.md`): the
/// `xoxc` token and the `d` cookie, plus the identifiers needed to route API calls and show
/// which account/workspace it is. Everything else is re-derivable from `auth.test` /
/// `client.userBoot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Workspace URL, e.g. `https://acme.slack.com/`. The API base is this plus `api/`.
    pub workspace_url: String,
    /// Workspace (`T…`) or enterprise (`E…`) id.
    pub team_id: TeamId,
    /// The `xoxc-…` session token sent as the `token` form field.
    ///
    /// Defaulted so a record whose secrets live in the Keychain (and are therefore blanked on
    /// disk) still deserializes.
    #[serde(default)]
    pub xoxc_token: String,
    /// The `d` cookie value (`xoxd-…`), sent as `Cookie: d=…`. Secret; see `xoxc_token`.
    #[serde(default)]
    pub d_cookie: String,
    /// The signed-in user's id (`U…`/`W…`). Empty until a successful `auth.test` fills it.
    #[serde(default)]
    pub user_id: UserId,
}

impl Session {
    /// Builds a session from its parts.
    pub fn new(
        workspace_url: impl Into<String>,
        team_id: impl Into<TeamId>,
        xoxc_token: impl Into<String>,
        d_cookie: impl Into<String>,
        user_id: impl Into<UserId>,
    ) -> Self {
        Self {
            workspace_url: workspace_url.into(),
            team_id: team_id.into(),
            xoxc_token: xoxc_token.into(),
            d_cookie: d_cookie.into(),
            user_id: user_id.into(),
        }
    }

    /// The REST API base for this workspace, e.g. `https://acme.slack.com/api/`.
    pub fn api_base(&self) -> String {
        format!("{}/api/", self.workspace_url.trim_end_matches('/'))
    }

    /// Whether both secret fields are present.
    pub fn has_secrets(&self) -> bool {
        !self.xoxc_token.is_empty() && !self.d_cookie.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_base_is_workspace_url_plus_api() {
        let s = Session::new("https://acme.slack.com/", "T1", "xoxc-x", "xoxd-y", "U1");
        assert_eq!(s.api_base(), "https://acme.slack.com/api/");
        // Trailing slash is normalized, not doubled.
        let s2 = Session::new("https://acme.slack.com", "T1", "x", "y", "U1");
        assert_eq!(s2.api_base(), "https://acme.slack.com/api/");
    }

    #[test]
    fn roundtrips_through_json() {
        let s = Session::new("https://acme.slack.com/", "T1", "xoxc-x", "xoxd-y", "U1");
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Session>(&json).unwrap(), s);
    }

    #[test]
    fn blanked_secrets_still_deserialize() {
        let json = r#"{"workspace_url":"https://acme.slack.com/","team_id":"T1"}"#;
        let s: Session = serde_json::from_str(json).unwrap();
        assert!(!s.has_secrets());
        assert_eq!(s.team_id.as_str(), "T1");
    }
}
