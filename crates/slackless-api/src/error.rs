use std::time::Duration;

use thiserror::Error;

use crate::types::ResponseError;

/// Slack error strings that mean the stored session is no longer valid and the user must
/// sign in again (`docs/research/api-auth.md`, "Invalidation error codes").
const AUTH_EXPIRED_ERRORS: &[&str] = &[
    "invalid_auth",
    "not_authed",
    "token_revoked",
    "token_expired",
    "account_inactive",
];

/// Errors from the Slack web API client.
#[derive(Debug, Error)]
pub enum ApiError {
    /// Slack returned ok:false with this error string.
    #[error("slack api error: {0}")]
    Slack(String),
    /// The session (xoxc token / d cookie) was rejected; carries Slack's error string
    /// (`invalid_auth`, `token_revoked`, ...). Re-authenticate.
    #[error("slack session expired: {0}")]
    AuthExpired(String),
    /// Still rate limited (HTTP 429) after the bounded retries.
    #[error("rate limited by slack, retry after {retry_after:?}")]
    RateLimited { retry_after: Duration },
    /// A non-2xx HTTP status whose body was not a Slack JSON envelope.
    #[error("unexpected http status {status}: {body}")]
    Status { status: u16, body: String },
    /// A file URL answered with an HTML page (Slack's sign-in redirect) instead of the file.
    #[error("file download was not authorized (got an html page)")]
    FileAuthRejected,
    /// Refused to send the session cookie to a host that is not Slack.
    #[error("refusing to send credentials to non-slack url: {0}")]
    UntrustedUrl(String),
    /// Invalid base URL or file URL.
    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),
    /// Transport failure.
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    /// The body was not JSON, or `ok: true` but the payload did not match the expected shape.
    #[error("malformed slack response: {0}")]
    Decode(#[from] serde_json::Error),
    /// The workspace returned a legacy non-Chime huddle backend (Screenhero) instead of `free_willy`.
    #[error("unsupported huddle backend: {0}")]
    UnsupportedHuddleBackend(String),
}

impl ApiError {
    /// Classifies a Slack `error` string.
    pub fn from_slack_error(error: impl Into<String>) -> Self {
        let error = error.into();
        if AUTH_EXPIRED_ERRORS.contains(&error.as_str()) {
            Self::AuthExpired(error)
        } else {
            Self::Slack(error)
        }
    }

    /// Whether the user has to sign in again.
    pub fn is_auth_expired(&self) -> bool {
        matches!(self, Self::AuthExpired(_))
    }
}

impl From<ResponseError> for ApiError {
    fn from(err: ResponseError) -> Self {
        match err {
            ResponseError::Slack(envelope) => {
                Self::from_slack_error(envelope.error.unwrap_or_else(|| "unknown_error".to_owned()))
            }
            ResponseError::Json(e) => Self::Decode(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ApiResponse;

    #[test]
    fn classifies_all_auth_expired_errors_and_missing_error_string() {
        for code in AUTH_EXPIRED_ERRORS {
            let err = ApiError::from_slack_error(*code);
            assert!(err.is_auth_expired(), "{code} must be auth expired");
            assert!(matches!(&err, ApiError::AuthExpired(s) if s == code));
        }

        let other = ApiError::from_slack_error("channel_not_found");
        assert!(!other.is_auth_expired());
        assert!(matches!(other, ApiError::Slack(s) if s == "channel_not_found"));

        let missing: ApiError =
            ApiResponse::<serde_json::Value>::from_value(serde_json::json!({"ok": false}))
                .unwrap_err()
                .into();
        assert!(matches!(missing, ApiError::Slack(s) if s == "unknown_error"));
    }
}
