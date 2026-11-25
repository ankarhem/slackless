//! OS integration behind traits; macOS first (ARCHITECTURE: slackless-platform).
//!
//! One trait per OS capability, composed under [`DesktopPlatform`] and selected at compile time
//! through [`current`]:
//!
//! | trait | capability | macOS backend |
//! |---|---|---|
//! | [`Notifier`] | post a system notification | unbundled: Notification Center via `osascript`; bundled: left to GPUI's `SystemNotification` (guide §14) |
//! | [`BadgeSetter`] | dock unread badge | `NSApplication.dockTile.setBadgeLabel` via `objc2` (GPUI has no dock-badge API) |
//! | [`IconSetter`] | app icon | `NSApplication.setApplicationIconImage` via `objc2` |
//! | [`AboutPanel`] | the app's About window | `NSApplication.orderFrontStandardAboutPanel` |
//! | [`FullScreenItem`] | the menu bar's full-screen item | `NSFullScreenMenuItemEverywhere` registered off |
//! | [`SoundPlayer`] | play a sound (notification sounds) | `NSSound` from the sound's bytes |
//! | [`StatusBarItem`] | macOS menu-bar status item & quick menu | `NSStatusBar` / `NSStatusItem` / `NSMenu` via `objc2` |
//! | [`AttentionRequester`] | dock / taskbar user attention bounce | `NSApplication.requestUserAttention` |
//! | [`SpellChecker`] | native OS spellchecker | `NSSpellChecker.sharedSpellChecker` |
//!
//! Other platforms get a compiling, non-OS fallback backend whose capabilities are no-ops.
//! Deep-link parsing lives in the pure [`parse_deep_link`] / [`DeepLink`] module.

use std::path::PathBuf;

mod deep_link;
mod fallback;
pub mod logging;
#[cfg(target_os = "macos")]
mod macos;
mod macos_overlay;

pub use deep_link::{DeepLink, SlackLink, SlackLinkKind, WebClientLink, parse_deep_link};
pub use logging::{default_log_path, init_file_logging};
pub use macos_overlay::{
    DesktopDrawOverlay, OverlayCursorState, OverlayDrawStroke, OverlayPenEvent, OverlayPenHandler,
    configure_floating_panel,
};

/// Application data directory (`~/Library/Application Support/slackless` on macOS,
/// `$XDG_DATA_HOME/slackless` or `~/.local/share/slackless` on Linux,
/// `%APPDATA%\slackless` on Windows).
pub fn app_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("slackless"))
}

/// Application cache directory (`~/Library/Caches/slackless` on macOS,
/// `$XDG_CACHE_HOME/slackless` or `~/.cache/slackless` on Linux,
/// `%LOCALAPPDATA%\slackless` on Windows).
pub fn app_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("slackless"))
}

/// Application configuration directory (`~/Library/Application Support/slackless` on macOS,
/// `$XDG_CONFIG_HOME/slackless` or `~/.config/slackless` on Linux,
/// `%APPDATA%\slackless` on Windows).
pub fn app_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("slackless"))
}

/// Durable SQLite archive path for one workspace (`<app_data_dir>/<team_id>.sqlite`).
pub fn archive_store_path(team_id: impl AsRef<str>) -> Option<PathBuf> {
    app_data_dir().map(|dir| dir.join(format!("{}.sqlite", team_id.as_ref())))
}

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

/// Install an event-driven listener for clicks on the OS menu-bar / tray status item so the GUI
/// never needs a background polling timer. Safe to call once at app startup.
pub fn install_status_bar_action_monitor(
    on_action: impl Fn(StatusBarAction) + Send + Sync + 'static,
) {
    #[cfg(target_os = "macos")]
    macos::install_status_bar_action_monitor(on_action);
    #[cfg(not(target_os = "macos"))]
    fallback::install_status_bar_action_monitor(on_action);
}

/// Install a POSIX `SIGTERM`/`SIGINT` signal monitor that invokes `on_terminate` on a helper
/// thread so the application can run graceful quit cleanup (e.g. leaving active huddles before
/// exiting). A second signal falls back to the OS default action so termination can never hang.
/// Safe to call once at app startup; no-op on non-Unix platforms.
pub fn install_termination_monitor(on_terminate: impl Fn() + Send + Sync + 'static) {
    #[cfg(unix)]
    unix_termination::install(on_terminate);
    #[cfg(not(unix))]
    let _ = on_terminate;
}

#[cfg(unix)]
mod unix_termination {
    use std::sync::Once;
    use std::sync::atomic::{AtomicI32, Ordering};

    static WRITE_FD: AtomicI32 = AtomicI32::new(-1);
    static INSTALL_ONCE: Once = Once::new();

    extern "C" fn handle_signal(sig: libc::c_int) {
        // Async-signal-safe self-pipe write only; SA_RESETHAND already restores SIG_DFL
        // so a second SIGTERM/SIGINT terminates cleanly.
        let fd = WRITE_FD.load(Ordering::Relaxed);
        if fd >= 0 {
            let byte: u8 = sig as u8;
            // SAFETY: `write` is async-signal-safe, `fd` is a valid non-blocking pipe write end,
            // and `&byte` points to 1 readable byte on the stack.
            unsafe {
                let _ = libc::write(fd, (&raw const byte).cast::<libc::c_void>(), 1);
            }
        }
    }

    fn set_cloexec(fd: libc::c_int) {
        // SAFETY: `fcntl(F_GETFD / F_SETFD)` on a freshly created pipe descriptor.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 {
                let _ = libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }

    fn set_nonblock(fd: libc::c_int) {
        // SAFETY: `fcntl(F_GETFL / F_SETFL)` on a freshly created pipe descriptor.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 {
                let _ = libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }
    }

    pub(crate) fn install(on_terminate: impl Fn() + Send + Sync + 'static) {
        INSTALL_ONCE.call_once(|| {
            let mut fds = [-1 as libc::c_int; 2];
            // SAFETY: `fds` is a valid 2-element array for `pipe`.
            if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
                return;
            }
            let read_fd = fds[0];
            let write_fd = fds[1];
            set_cloexec(read_fd);
            set_cloexec(write_fd);
            set_nonblock(write_fd);

            WRITE_FD.store(write_fd, Ordering::Release);

            // SAFETY: `sigaction` is a C POD struct zero-initialized before setting fields;
            // `handle_signal` has the required `extern "C" fn(c_int)` ABI.
            unsafe {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = handle_signal as *const () as libc::sighandler_t;
                sa.sa_flags = libc::SA_RESETHAND;
                libc::sigemptyset(&raw mut sa.sa_mask);
                libc::sigaction(libc::SIGTERM, &raw const sa, std::ptr::null_mut());
                libc::sigaction(libc::SIGINT, &raw const sa, std::ptr::null_mut());
            }

            let spawned = std::thread::Builder::new()
                .name("slackless-signal".into())
                .spawn(move || {
                    let mut buf = [0u8; 8];
                    loop {
                        // SAFETY: `read_fd` is owned by this thread and `buf` is valid for writes.
                        let n = unsafe {
                            libc::read(read_fd, buf.as_mut_ptr().cast::<libc::c_void>(), buf.len())
                        };
                        if n > 0 {
                            on_terminate();
                            break;
                        }
                        if n == 0 {
                            break;
                        }
                        if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                            continue;
                        }
                        break;
                    }
                    // SAFETY: `read_fd` is exclusively owned by this thread and no longer needed.
                    unsafe {
                        libc::close(read_fd);
                    }
                });
            if spawned.is_err() {
                WRITE_FD.store(-1, Ordering::Release);
                // SAFETY: thread failed to spawn so nobody else holds either pipe end.
                unsafe {
                    libc::close(read_fd);
                    libc::close(write_fd);
                }
            }
        });
    }
}

/// Error surfaced by the fallible platform capabilities.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlatformError {
    /// The input is not a URL this crate handles.
    #[error("not a recognized deep link: {0}")]
    BadLink(String),
}

/// Result alias for the platform crate.
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

/// Set the app's dock/taskbar icon from PNG bytes.
///
/// macOS: `NSApplication.setApplicationIconImage` (dispatched to the main queue when called off
/// the main thread). Unbundled runs have no bundle icon, so this is where they get one.
pub trait IconSetter {
    fn set_icon(&self, png: &[u8]);
}

/// Show the system About window: the app's icon, name and version.
///
/// macOS: `NSApplication.orderFrontStandardAboutPanel` (dispatched to the main queue when called
/// off the main thread), reading name and version from the app's Info.plist.
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

/// Cohesive desktop OS integration supertrait combining every platform capability.
///
/// Front-ends can accept `impl DesktopPlatform` or `dyn DesktopPlatform` instead of listing
/// individual capability bounds.
pub trait DesktopPlatform:
    Notifier
    + BadgeSetter
    + IconSetter
    + AboutPanel
    + FullScreenItem
    + SoundPlayer
    + StatusBarItem
    + AttentionRequester
    + SpellChecker
    + Send
    + Sync
    + 'static
{
}

impl<T> DesktopPlatform for T where
    T: Notifier
        + BadgeSetter
        + IconSetter
        + AboutPanel
        + FullScreenItem
        + SoundPlayer
        + StatusBarItem
        + AttentionRequester
        + SpellChecker
        + Send
        + Sync
        + 'static
{
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
pub fn current() -> impl DesktopPlatform {
    Platform::new()
}

/// An in-memory, no-op platform backend that never touches OS APIs (notifications, dock badges,
/// or AppKit). Intended for headless tests on all operating systems.
pub fn noop() -> impl DesktopPlatform {
    fallback::FallbackPlatform::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `current()` and `noop()` must construct on every platform, satisfy `DesktopPlatform`
    /// (including object safety), and allow safe headless calls without panicking.
    #[test]
    fn current_and_noop_satisfy_desktop_platform() {
        let platform: Box<dyn DesktopPlatform> = Box::new(current());
        assert_eq!(platform.take_status_bar_action(), None);

        let noop_platform: Box<dyn DesktopPlatform> = Box::new(noop());
        assert_eq!(noop_platform.take_status_bar_action(), None);
        assert!(noop_platform.check_spelling("helo", None).is_empty());
    }

    #[test]
    fn app_directories_and_archive_store_path_are_consistent() {
        let data = app_data_dir().expect("data dir");
        let cache = app_cache_dir().expect("cache dir");
        let config = app_config_dir().expect("config dir");

        assert_eq!(data.file_name().and_then(|s| s.to_str()), Some("slackless"));
        assert_eq!(
            cache.file_name().and_then(|s| s.to_str()),
            Some("slackless")
        );
        assert_eq!(
            config.file_name().and_then(|s| s.to_str()),
            Some("slackless")
        );

        #[cfg(target_os = "macos")]
        {
            assert!(
                data.ends_with("Library/Application Support/slackless"),
                "unexpected macOS data dir: {data:?}"
            );
            assert!(
                config.ends_with("Library/Application Support/slackless"),
                "unexpected macOS config dir: {config:?}"
            );
            assert!(
                cache.ends_with("Library/Caches/slackless"),
                "unexpected macOS cache dir: {cache:?}"
            );
        }

        let archive = archive_store_path("T0123ABC").expect("archive path");
        assert_eq!(archive, data.join("T0123ABC.sqlite"));

        let team_id = slackless_types::TeamId::from("T9999XYZ");
        assert_eq!(
            archive_store_path(&team_id).expect("archive path for TeamId"),
            data.join("T9999XYZ.sqlite")
        );
    }
}
