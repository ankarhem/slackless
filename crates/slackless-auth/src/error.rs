//! Error type for session acquisition, validation and storage.

use thiserror::Error;

/// Everything that can go wrong acquiring, validating, storing or importing a session.
#[derive(Debug, Error)]
pub enum AuthError {
    /// `auth.test` reported the session is no longer valid (`invalid_auth`, `token_revoked`,
    /// ...). The user must sign in again. Carries Slack's error string.
    #[error("session needs re-authentication: {0}")]
    NeedsReauth(String),

    /// A Slack API call failed for a reason other than an expired session.
    #[error(transparent)]
    Api(#[from] slackless_api::ApiError),

    /// Reading or writing the on-disk session store failed.
    #[error("session store i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// The session store file was not valid JSON.
    #[error("session store is corrupt: {0}")]
    Json(#[from] serde_json::Error),

    /// No OS config directory could be located for the default store path.
    #[error("could not locate a config directory for the session store")]
    NoConfigDir,

    /// The user closed the sign-in window before a session was captured.
    #[error("sign-in was cancelled")]
    SignInCancelled,

    /// The sign-in window or its webview could not be created.
    #[error("sign-in window error: {0}")]
    SignInWindow(String),

    /// A macOS Keychain / secret-store operation failed.
    #[error("keychain error: {0}")]
    Keychain(String),
}

/// Result alias for this crate.
pub type Result<T, E = AuthError> = std::result::Result<T, E>;
