//! OS integration behind traits; macOS first (ARCHITECTURE: slackless-platform).
//!
//! One trait per OS capability, selected at compile time through [`current`]:
//!
//! | trait | capability | macOS backend |
//! |---|---|---|
//! | [`Notifier`] | post a system notification | unbundled: Notification Center via `osascript`; bundled: left to GPUI's `SystemNotification` (guide §14) |
//! | [`BadgeSetter`] | dock unread badge | `NSApplication.dockTile.setBadgeLabel` via `objc2` (GPUI has no dock-badge API) |
//! | [`IconSetter`] | app icon | `NSApplication.setApplicationIconImage` via `objc2` |
//! | [`SecretStore`] | named secrets | Keychain generic passwords via `security-framework` |
//! | [`DeepLinkHandler`] | `slack://` and `https://app.slack.com/client` links | bundle `Info.plist` scheme check + the pure [`DeepLink`] parser |
//!
//! Other platforms get a compiling, non-OS fallback backend so the crate
//! builds everywhere; its `SecretStore` is in-process memory, its notifier,
//! badge setter and icon setter are no-ops.
//!
//! A `StatusItem` trait (`NSStatusItem`, guide §14) is planned but not yet
//! modeled; it will land here alongside [`BadgeSetter`].

mod deep_link;
#[cfg(not(target_os = "macos"))]
mod fallback;
#[cfg(target_os = "macos")]
mod macos;

pub use deep_link::{DeepLink, SlackLink, SlackLinkKind, WebClientLink, parse_deep_link};

/// Error surfaced by the fallible platform capabilities.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlatformError {
    /// The requested secret does not exist (load/delete on a fresh name).
    #[error("no secret `{name}` under service `{service}`")]
    SecretNotFound { service: String, name: String },
    /// The OS secret store rejected the operation (macOS: Keychain status).
    #[error("OS secret store error: {0}")]
    SecretStore(String),
    /// The input is not a URL this crate handles.
    #[error("not a recognized deep link: {0}")]
    BadLink(String),
}

/// Result alias for the platform traits.
pub type Result<T> = std::result::Result<T, PlatformError>;

/// Post a system notification (title + body).
///
/// macOS note: GPUI's `SystemNotification` (gpui-kit guide §14) delivers
/// through `UNUserNotificationCenter`, which only works from an app bundle.
/// The GUI posts every notification through both GPUI and this trait; the
/// macOS backend stays silent when bundled (GPUI delivers) and posts via
/// Notification Center (`osascript`) when unbundled (GPUI cannot), so each
/// notification shows exactly once either way.
pub trait Notifier {
    fn notify(&self, title: &str, body: &str);
}

/// Set the dock/taskbar unread badge; `None` clears it.
///
/// macOS note: GPUI has no dock-tile API (gpui-kit guide §14), so this is
/// implemented with `objc2`:
/// `NSApplication.sharedApplication().dockTile().setBadgeLabel()`.
/// Calls from non-main threads are dispatched onto the main queue.
pub trait BadgeSetter {
    fn set_badge(&self, label: Option<&str>);
}

/// Set the app's dock/taskbar icon from PNG bytes. Call it on the main thread.
///
/// macOS: `NSApplication.setApplicationIconImage`. Unbundled runs have no
/// bundle icon, so this is where they get one; a bundle's icon is replaced
/// the same way while the app runs.
pub trait IconSetter {
    fn set_icon(&self, png: &[u8]);
}

/// Store, load, and delete a named secret (macOS: Keychain generic password).
///
/// Secrets are addressed by a `service` (the app/workspace scope) plus a
/// `name` (e.g. `"workspace:T0123"`), so several secrets can coexist.
pub trait SecretStore {
    /// Create or update the secret `name` under `service`.
    fn store_secret(&self, service: &str, name: &str, secret: &[u8]) -> Result<()>;

    /// Read the secret `name` under `service`.
    ///
    /// A missing entry is [`PlatformError::SecretNotFound`], never a panic.
    fn load_secret(&self, service: &str, name: &str) -> Result<Vec<u8>>;

    /// Remove the secret `name` under `service`.
    ///
    /// Deleting an absent secret is not an error (idempotent).
    fn delete_secret(&self, service: &str, name: &str) -> Result<()>;
}

/// Register the app for deep links and surface parsed ones.
///
/// Two entry points are modeled (ARCHITECTURE: slackless-platform):
/// `slack://…` (Slack app links) and `https://app.slack.com/client/…`
/// (web-client links). Parsing is pure — see [`DeepLink`] — so front-ends can
/// work with it directly; this trait covers OS registration and hand-off.
pub trait DeepLinkHandler {
    /// Register/verify deep-link support and return the URL schemes the OS
    /// can already deliver to this app.
    ///
    /// macOS: reads the running bundle's `Info.plist` (`CFBundleURLTypes`).
    /// An empty list means the bundle does not declare `slack` yet — typical
    /// for unbundled `cargo run` dev builds — and the scheme must be added to
    /// the app bundle before macOS will hand over `slack://` links.
    fn register(&self) -> Result<Vec<String>>;

    /// Parse a URL handed over by the OS/front-end and remember it as the
    /// latest pending link. Returns the parsed [`DeepLink`].
    fn handle_url(&self, url: &str) -> Result<DeepLink>;

    /// Take the most recently handled link, if any (clears it).
    fn take_link(&self) -> Option<DeepLink>;
}

#[cfg(target_os = "macos")]
type Platform = macos::MacPlatform;
#[cfg(not(target_os = "macos"))]
type Platform = fallback::FallbackPlatform;

/// The platform integration for the current OS.
///
/// Cheap to construct and `Send + Sync`; construct it once and share it.
/// Nothing touches AppKit until a capability actually needs it, so this is
/// safe on headless threads too.
pub fn current()
-> impl Notifier + BadgeSetter + IconSetter + SecretStore + DeepLinkHandler + Send + Sync + 'static
{
    Platform::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `current()` must construct on every platform and the always-safe
    /// capability calls must not panic (this path touches no OS API; `notify`
    /// is left out because unbundled macOS runs post a real notification).
    #[test]
    fn current_is_constructible_and_safe_to_call() {
        let platform = current();
        assert_eq!(platform.take_link(), None);
    }

    /// Keychain round-trip: store → load → delete → gone.
    ///
    /// Ignored because headless CI has no keychain access; run it locally
    /// with `cargo test -p slackless-platform -- --ignored`.
    #[test]
    #[ignore = "requires macOS keychain access (CI/headless has none)"]
    fn keychain_roundtrip() {
        let platform = current();
        let service = "slackless-platform-test";
        let name = "roundtrip";
        let secret = b"xoxd-roundtrip-payload".to_vec();

        // Start from a known state.
        let _ = platform.delete_secret(service, name);
        platform
            .store_secret(service, name, &secret)
            .expect("store");
        assert_eq!(platform.load_secret(service, name).expect("load"), secret);

        platform.delete_secret(service, name).expect("delete");
        assert_eq!(
            platform.load_secret(service, name).unwrap_err(),
            PlatformError::SecretNotFound {
                service: service.to_string(),
                name: name.to_string(),
            }
        );
    }

    /// Deep-link hand-off through the `current()` selector itself; this path
    /// is pure (parser + in-process state), so it runs everywhere.
    #[test]
    fn deep_link_roundtrip_through_current() {
        let platform = current();
        let url = "https://app.slack.com/client/T0123ABC/C0456DEF";
        let link = platform.handle_url(url).expect("web client link parses");
        assert_eq!(
            link,
            DeepLink::WebClient(WebClientLink {
                team_id: Some("T0123ABC".into()),
                channel_id: Some("C0456DEF".into()),
                thread_ts: None,
                raw: url.into(),
            })
        );
        assert_eq!(platform.take_link(), Some(link));
        assert_eq!(platform.take_link(), None);

        assert!(platform.handle_url("this is not a url").is_err());
    }

    /// The fallback secret store keeps its in-process contract exercised on
    /// platforms without a real keychain backend.
    #[cfg(all(test, not(target_os = "macos")))]
    mod fallback_store {
        use super::*;

        #[test]
        fn in_memory_secret_roundtrip() {
            let platform = current();
            platform.store_secret("svc", "name", b"shh").expect("store");
            assert_eq!(platform.load_secret("svc", "name").expect("load"), b"shh");
            platform.delete_secret("svc", "name").expect("delete");
            assert_eq!(
                platform.load_secret("svc", "name").unwrap_err(),
                PlatformError::SecretNotFound {
                    service: "svc".into(),
                    name: "name".into(),
                }
            );
        }
    }
}
