//! Session validation against Slack's `auth.test`.

use slackless_api::{AuthTest, SlackApi};

use crate::error::{AuthError, Result};
use crate::session::Session;

/// Validates `session` by calling `auth.test` on its workspace.
///
/// Returns the `auth.test` payload (who/where the session is) on success. A session that Slack
/// rejects with `invalid_auth` / `token_revoked` (and the other invalidation codes) maps to
/// [`AuthError::NeedsReauth`] so the caller can route the user back to sign-in; any other API or
/// transport failure surfaces as [`AuthError::Api`].
pub async fn validate(session: &Session) -> Result<AuthTest> {
    let api = SlackApi::new(
        &session.api_base(),
        session.xoxc_token.clone(),
        &session.d_cookie,
    )?;
    match api.auth_test().await {
        Ok(test) => Ok(test),
        Err(e) if e.is_auth_expired() => Err(AuthError::NeedsReauth(e.to_string())),
        Err(e) => Err(AuthError::Api(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn session_for(server: &MockServer) -> Session {
        // The mock server's base URL stands in for the workspace URL; `api_base` appends `api/`.
        Session::new(server.uri(), "T1", "xoxc-1-2-3-abc", "xoxd-abc", "")
    }

    #[tokio::test]
    async fn ok_returns_auth_test() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "url": "https://acme.slack.com/",
                "team_id": "T1",
                "user_id": "U42",
            })))
            .mount(&server)
            .await;

        let test = validate(&session_for(&server)).await.unwrap();
        assert_eq!(test.user_id.unwrap().as_str(), "U42");
    }

    #[tokio::test]
    async fn invalid_auth_maps_to_needs_reauth() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false,
                "error": "invalid_auth",
            })))
            .mount(&server)
            .await;

        let err = validate(&session_for(&server)).await.unwrap_err();
        assert!(
            matches!(err, AuthError::NeedsReauth(ref e) if e.contains("invalid_auth")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn token_revoked_maps_to_needs_reauth() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false,
                "error": "token_revoked",
            })))
            .mount(&server)
            .await;

        let err = validate(&session_for(&server)).await.unwrap_err();
        assert!(matches!(err, AuthError::NeedsReauth(_)), "{err:?}");
    }

    #[tokio::test]
    async fn other_errors_stay_api_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false,
                "error": "ratelimited",
            })))
            .mount(&server)
            .await;

        let err = validate(&session_for(&server)).await.unwrap_err();
        assert!(matches!(err, AuthError::Api(_)), "{err:?}");
    }
}
