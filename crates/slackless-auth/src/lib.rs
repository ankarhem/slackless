//! Slack web-client session acquisition, validation, storage and import.
//!
//! A *session* is the minimal credential set worth persisting ([`Session`]): an `xoxc` token, the
//! `d` cookie, and the identifiers that route and label it. See `docs/research/api-auth.md`.
//!
//! What this crate provides:
//!
//! * [`Session`] + [`SessionStore`] — multi-workspace persistence to a JSON file under the config
//!   dir, with the secret fields mirrored into the macOS Keychain when available.
//! * [`validate`] — checks a session with `auth.test`, mapping `invalid_auth`/`token_revoked` to
//!   [`AuthError::NeedsReauth`].
//! * `embedded_sign_in` (feature `webview`) / [`capture_sessions`] — the first-party WebKit
//!   sign-in: slackless acquires its own session from a webview window it owns, turning the
//!   webview's own `localConfig_v2` + `d` cookie into [`Session`]s.
//!
//! ## Secret storage
//!
//! Keychain reads and writes go through Apple's `/usr/bin/security` tool (see `keychain.rs`), so
//! rebuilt binaries never re-trigger a Keychain password prompt.
//!
//! This crate reads only slackless's **own** webview session; it never reads or decrypts
//! credentials from any other installed application.
//!
//! ## Patchability
//!
//! Slack's private surface changes without notice. The `localConfig_v2` capture ignores unknown
//! fields, response modelling is delegated to `slackless_api` (whose structs default every field
//! and keep unknown keys), and the few Slack strings this crate relies on route through
//! `slackless_api` / `slackless_types`.

pub use slackless_types as types;

mod embedded;
mod error;
mod keychain;
mod private_file;
mod secrets;
mod session;
mod store;
mod validate;

#[cfg(feature = "webview")]
pub use embedded::embedded_sign_in;
pub use embedded::{SignInRequest, WebviewCapture, capture_sessions};
pub use error::{AuthError, Result};
pub use secrets::{AI_SUMMARY_API_KEY_ACCOUNT, SecretStore};
pub use session::Session;
pub use store::SessionStore;
pub use validate::validate;

/// Re-export of `auth.test`'s response, returned by [`validate`].
pub use slackless_api::AuthTest;
