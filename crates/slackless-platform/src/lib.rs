//! OS integration behind traits; macOS first (ARCHITECTURE: slackless-platform).
//!
//! One trait per OS capability, selected at compile time through [`current`]:
//!
//! | trait | capability | macOS backend |
//! |---|---|---|
//! | [`Notifier`] | post a system notification | unbundled: Notification Center via `osascript`; bundled: left to GPUI's `SystemNotification` (guide §14) |
//! | [`BadgeSetter`] | dock unread badge | `NSApplication.dockTile.setBadgeLabel` via `objc2` (GPUI has no dock-badge API) |
//! | [`IconSetter`] | app icon | `NSApplication.setApplicationIconImage` via `objc2` |
//! | [`AboutPanel`] | the app's About window | `NSApplication.orderFrontStandardAboutPanel` |
//! | [`FullScreenItem`] | the menu bar's full-screen item | `NSFullScreenMenuItemEverywhere` registered off |
//! | [`SoundPlayer`] | play a sound (notification sounds) | `NSSound` from the sound's bytes |
//! | [`SecretStore`] | named secrets | Keychain generic passwords via `security-framework` |
//! | [`DeepLinkHandler`] | `slack://` and `https://app.slack.com/client` links | bundle `Info.plist` scheme check + the pure [`DeepLink`] parser |
//! | [`StatusBarItem`] | macOS menu-bar status item & quick menu | `NSStatusBar` / `NSStatusItem` / `NSMenu` via `objc2` |
//!
//! Other platforms get a compiling, non-OS fallback backend so the crate
//! builds everywhere; its `SecretStore` is in-process memory, its notifier,
//! badge setter, icon setter, About panel, full-screen item, sound player and
//! status-bar item are no-ops.

mod deep_link;
#[cfg(not(target_os = "macos"))]
mod fallback;
pub mod logging;
#[cfg(target_os = "macos")]
mod macos;

pub use deep_link::{DeepLink, SlackLink, SlackLinkKind, WebClientLink, parse_deep_link};
pub use logging::{default_log_path, init_file_logging};

/// Direction of a hardware mouse Back/Forward navigation gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseNavDirection {
    Back,
    Forward,
}

/// Install a macOS local `NSEvent` monitor for swipe-based mouse Back/Forward events that
/// GPUI's macOS backend drops (`NSEventType::Swipe` with `phase == None`, sent by Logitech
/// Options+ / Magic Mouse / 3-finger swipe). Safe to call once at app startup; no-op on
/// non-macOS platforms.
pub fn install_mouse_nav_monitor(on_nav: impl Fn(MouseNavDirection) + Send + Sync + 'static) {
    #[cfg(target_os = "macos")]
    macos::install_mouse_nav_monitor(on_nav);
    #[cfg(not(target_os = "macos"))]
    fallback::install_mouse_nav_monitor(on_nav);
}

/// Install a macOS distributed notification observer for `com.apple.screenIsLocked`
/// (`F-COLLAB-059`, auto-leave active huddle when the computer locks). Safe to call once at app
/// startup; no-op on non-macOS platforms.
pub fn install_screen_lock_monitor(on_lock: impl Fn() + Send + Sync + 'static) {
    #[cfg(target_os = "macos")]
    macos::install_screen_lock_monitor(on_lock);
    #[cfg(not(target_os = "macos"))]
    fallback::install_screen_lock_monitor(on_lock);
}

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

/// Show the system About window: the app's icon, name and version. Call it on the
/// main thread.
///
/// macOS: `NSApplication.orderFrontStandardAboutPanel`, which reads the name and
/// version from the app's Info.plist (embedded in the binary for unbundled runs).
pub trait AboutPanel {
    fn show_about(&self);
}

/// Leave the menu bar's full-screen item to the app, whose menus carry their own (Slack's
/// Toggle Full Screen). Call it before the menus are set.
///
/// macOS: AppKit adds "Enter Full Screen" to the View menu unless the
/// `NSFullScreenMenuItemEverywhere` default is off; it is registered off, not persisted.
pub trait FullScreenItem {
    fn drop_system_full_screen_item(&self);
}

/// Play a sound from its encoded bytes (an mp3), replacing one still playing. Call it on
/// the main thread.
pub trait SoundPlayer {
    fn play_sound(&self, data: &[u8]);
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

/// User actions triggered from the OS menu-bar / tray status item (`F-SHELL-168`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusBarAction {
    /// Bring the main window to the front (unhiding if hidden in tray).
    ShowWindow,
    /// Switch to the workspace at `index` in the workspace rail.
    SwitchWorkspace(usize),
    /// Toggle Do Not Disturb / pause notifications.
    ToggleDnd,
    /// Quit the application cleanly.
    Quit,
}

/// Snapshot rendered into the OS menu-bar / tray status item and its menu.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusBarState {
    /// Whether the status item is shown in the OS menu bar.
    pub visible: bool,
    /// Total unread mentions/DMs across workspaces.
    pub unread_mentions: u32,
    /// Whether any workspace has unread messages.
    pub has_unreads: bool,
    /// Whether notifications are currently paused / DND is active.
    pub dnd_active: bool,
    /// Workspaces listed in the menu: `(name, mentions, has_unreads)`.
    pub workspaces: Vec<(String, u32, bool)>,
}

/// Persistent OS menu-bar / system-tray status item (`F-SHELL-168`, `F-SHELL-141`).
///
/// macOS: `NSStatusBar::systemStatusBar()` status item + `NSMenu`, updated on the main thread.
pub trait StatusBarItem {
    /// Create, update, or hide the menu-bar status item according to `state`.
    fn update_status_bar(&self, icon_png: Option<&[u8]>, state: &StatusBarState);

    /// Pop the next user action clicked in the status-bar menu, if any.
    fn take_status_bar_action(&self) -> Option<StatusBarAction>;
}

/// Request user attention in the OS dock / taskbar (`F-SHELL-175`).
///
/// macOS: `NSApplication.requestUserAttention` (informational bounce once, or critical bounce
/// until activated).
pub trait AttentionRequester {
    fn request_attention(&self, critical: bool);
}

/// A misspelled word span returned by [`SpellChecker`] (UTF-8 byte offsets into the checked text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Misspelling {
    pub range: std::ops::Range<usize>,
}

/// Native OS spellchecker (`F-SHELL-098`, `F-SHELL-099`).
///
/// macOS: `NSSpellChecker.sharedSpellChecker`.
pub trait SpellChecker {
    /// Return UTF-8 byte ranges of misspelled words in `text`. `language` is an optional Apple /
    /// BCP-47 language code (e.g. `"en_US"`, `"sv"`); `None` uses OS automatic detection.
    fn check_spelling(&self, text: &str, language: Option<&str>) -> Vec<Misspelling>;

    /// Suggested replacements for a misspelled word.
    fn spelling_guesses(&self, word: &str, language: Option<&str>) -> Vec<String>;

    /// Add `word` to the user's OS dictionary ("Learn Spelling").
    fn learn_word(&self, word: &str);
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
pub fn current() -> impl Notifier
+ BadgeSetter
+ IconSetter
+ AboutPanel
+ FullScreenItem
+ SoundPlayer
+ SecretStore
+ DeepLinkHandler
+ StatusBarItem
+ AttentionRequester
+ SpellChecker
+ Send
+ Sync
+ 'static {
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
        assert_eq!(platform.take_status_bar_action(), None);
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
