//! macOS backend: dock badge, app icon, About panel, status bar, spellchecker, and input/lock
//! monitors via AppKit (`objc2`). Notifications: bundled builds leave them to GPUI's
//! `SystemNotification` (`UNUserNotificationCenter`); unbundled dev builds, where that center
//! refuses to start, post through Notification Center via `osascript` (see [`Notifier`]).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::process::{Command, Stdio};
use std::ptr::NonNull;
use std::sync::{Arc, LazyLock, Mutex, Once, PoisonError};

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread as _, MainThreadMarker, MainThreadOnly as _, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAboutPanelOptionApplicationIcon, NSApplication, NSEvent, NSEventMask, NSEventPhase,
    NSEventType, NSImage, NSMenu, NSMenuItem, NSRequestUserAttentionType, NSSound, NSSpellChecker,
    NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{
    NSBundle, NSData, NSDictionary, NSDistributedNotificationCenter, NSInteger, NSNotFound,
    NSNotification, NSNumber, NSObject, NSRange, NSSize, NSString, NSUserDefaults,
};

use crate::{
    AboutPanel, AttentionRequester, BadgeSetter, FullScreenItem, IconSetter, Misspelling,
    MouseNavDirection, Notifier, SoundPlayer, SpellChecker, StatusBarAction, StatusBarItem,
    StatusBarState,
};

/// macOS integration. Cheap to construct; AppKit is only touched when a capability needs it,
/// always on the main thread (dispatched there if not).
pub struct MacPlatform;

impl MacPlatform {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MacPlatform {
    fn default() -> Self {
        Self::new()
    }
}

/// Runs `f` immediately when already on the main thread, otherwise dispatches it asynchronously
/// onto the main GCD queue so AppKit calls never panic when invoked from background threads.
fn run_on_main(f: impl FnOnce(MainThreadMarker) + Send + 'static) {
    if let Some(mtm) = MainThreadMarker::new() {
        f(mtm);
    } else {
        DispatchQueue::main().exec_async(move || {
            if let Some(mtm) = MainThreadMarker::new() {
                f(mtm);
            }
        });
    }
}

/// Unhides and brings the application to the foreground when called on the main thread.
fn activate_app() {
    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        app.unhide(None);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
}

/// AppleScript run by `osascript`: title and body arrive as argv, so no text
/// from a message is ever spliced into the script source.
const NOTIFY_SCRIPT: &str =
    "on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run";

impl Notifier for MacPlatform {
    /// Bundled app (a bundle identifier exists): no-op, because GPUI's
    /// `SystemNotification` posts through `UNUserNotificationCenter` there
    /// and owns authorization and responses (gpui-kit guide §14); posting
    /// here too would duplicate every notification.
    ///
    /// Unbundled (`cargo run`): `UNUserNotificationCenter` aborts the process
    /// without a bundle and GPUI skips delivery, so post through Notification
    /// Center with `osascript display notification` instead. The child is
    /// detached; delivery failures only lose the notification.
    fn notify(&self, title: &str, body: &str) {
        if NSBundle::mainBundle().bundleIdentifier().is_some() {
            return;
        }
        let spawned = Command::new("/usr/bin/osascript")
            .args(["-e", NOTIFY_SCRIPT, "--", title, body])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            // Reap the child off-thread so it never lingers as a zombie.
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
            }
            Err(error) => tracing::warn!(%error, "notification not posted via osascript"),
        }
    }
}

impl BadgeSetter for MacPlatform {
    fn set_badge(&self, label: Option<&str>) {
        let label = label.map(str::to_string);
        run_on_main(move |mtm| {
            let app = NSApplication::sharedApplication(mtm);
            let ns_label = label.as_deref().map(NSString::from_str);
            app.dockTile().setBadgeLabel(ns_label.as_deref());
        });
    }
}

impl IconSetter for MacPlatform {
    fn set_icon(&self, png: &[u8]) {
        let png = png.to_vec();
        run_on_main(move |mtm| {
            let Some(image) = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&png))
            else {
                tracing::warn!("failed to decode application icon PNG bytes");
                return;
            };
            // SAFETY: `image` is a valid non-nil `NSImage`, set on the main thread.
            unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
        });
    }
}

impl AboutPanel for MacPlatform {
    fn show_about(&self) {
        run_on_main(|mtm| {
            let app = NSApplication::sharedApplication(mtm);
            // Unbundled, the standard panel shows the executable folder's icon: hand it the app's.
            let Some(icon) = app.applicationIconImage() else {
                app.orderFrontStandardAboutPanel(None);
                return;
            };
            let icon: &AnyObject = &icon;
            // SAFETY: `NSAboutPanelOptionApplicationIcon` is an immutable AppKit constant read on
            // the main thread.
            let key = unsafe { NSAboutPanelOptionApplicationIcon };
            let options = NSDictionary::from_slices(&[key], &[icon]);
            // SAFETY: `options` maps the About panel icon key to an `NSImage` on the main thread.
            unsafe { app.orderFrontStandardAboutPanelWithOptions(&options) };
        });
    }
}

impl FullScreenItem for MacPlatform {
    fn drop_system_full_screen_item(&self) {
        let key = NSString::from_str("NSFullScreenMenuItemEverywhere");
        let off = NSNumber::numberWithBool(false);
        let off: &AnyObject = &off;
        let defaults = NSDictionary::from_slices(&[&*key], &[off]);
        // SAFETY: `defaults` maps an `NSString` key to an `NSNumber`, as `registerDefaults:` expects.
        unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&defaults) };
    }
}

thread_local! {
    /// The sound playing now: an NSSound stops when released, so it is kept until the next.
    static PLAYING: RefCell<Option<Retained<NSSound>>> = const { RefCell::new(None) };
}

impl SoundPlayer for MacPlatform {
    fn play_sound(&self, data: &[u8]) {
        let Some(sound) = NSSound::initWithData(NSSound::alloc(), &NSData::with_bytes(data)) else {
            tracing::warn!(len = data.len(), "not a playable sound");
            return;
        };
        sound.play();
        PLAYING.with(|playing| *playing.borrow_mut() = Some(sound));
    }
}

type StatusBarCallback = Arc<dyn Fn(StatusBarAction) + Send + Sync + 'static>;

#[derive(Default)]
struct StatusBarDispatchState {
    pending: VecDeque<StatusBarAction>,
    callback: Option<StatusBarCallback>,
}

/// Synchronized queue/callback handoff for status-bar menu actions.
/// Holding one mutex across draining `pending` and installing `callback` prevents actions
/// emitted concurrently during startup from being stranded in `pending`.
#[derive(Default)]
struct StatusBarDispatch {
    state: Mutex<StatusBarDispatchState>,
}

impl StatusBarDispatch {
    fn emit(&self, action: StatusBarAction) {
        let cb = {
            let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(cb) = guard.callback.as_ref() {
                Some(Arc::clone(cb))
            } else {
                guard.pending.push_back(action);
                return;
            }
        };
        if let Some(cb) = cb {
            cb(action);
        }
    }

    fn install(&self, on_action: impl Fn(StatusBarAction) + Send + Sync + 'static) {
        let cb: StatusBarCallback = Arc::new(on_action);
        let drained: Vec<StatusBarAction> = {
            let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            guard.callback = Some(Arc::clone(&cb));
            guard.pending.drain(..).collect()
        };
        for action in drained {
            cb(action);
        }
    }

    fn take(&self) -> Option<StatusBarAction> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pending
            .pop_front()
    }
}

static STATUS_DISPATCH: LazyLock<StatusBarDispatch> = LazyLock::new(StatusBarDispatch::default);

fn emit_status_bar_action(action: StatusBarAction) {
    STATUS_DISPATCH.emit(action);
}

pub(crate) fn install_status_bar_action_monitor(
    on_action: impl Fn(StatusBarAction) + Send + Sync + 'static,
) {
    STATUS_DISPATCH.install(on_action);
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "SlacklessStatusBarTarget"]
    #[ivars = ()]
    struct StatusBarTarget;

    impl StatusBarTarget {
        #[unsafe(method(showWindow:))]
        fn show_window(&self, _sender: Option<&AnyObject>) {
            emit_status_bar_action(StatusBarAction::ShowWindow);
            activate_app();
        }

        #[unsafe(method(switchWorkspace:))]
        fn switch_workspace(&self, sender: Option<&NSMenuItem>) {
            let tag = sender.map(|item| item.tag()).unwrap_or(0);
            if tag >= 0 {
                emit_status_bar_action(StatusBarAction::SwitchWorkspace(tag as usize));
            }
            activate_app();
        }

        #[unsafe(method(toggleDnd:))]
        fn toggle_dnd(&self, _sender: Option<&AnyObject>) {
            emit_status_bar_action(StatusBarAction::ToggleDnd);
        }

        #[unsafe(method(quitApp:))]
        fn quit_app(&self, _sender: Option<&AnyObject>) {
            emit_status_bar_action(StatusBarAction::Quit);
        }
    }
);

impl StatusBarTarget {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        // SAFETY: `NSObject::init` on a freshly allocated subclass instance.
        unsafe { msg_send![super(this), init] }
    }
}

struct LiveStatusItem {
    item: Retained<NSStatusItem>,
    target: Retained<StatusBarTarget>,
}

thread_local! {
    static STATUS_ITEM: RefCell<Option<LiveStatusItem>> = const { RefCell::new(None) };
}

impl StatusBarItem for MacPlatform {
    fn update_status_bar(&self, icon_png: Option<&[u8]>, state: &StatusBarState) {
        let icon = icon_png.map(|b| b.to_vec());
        let state = state.clone();
        run_on_main(move |mtm| apply_status_bar(mtm, icon.as_deref(), &state));
    }

    fn take_status_bar_action(&self) -> Option<StatusBarAction> {
        STATUS_DISPATCH.take()
    }
}

fn apply_status_bar(mtm: MainThreadMarker, icon_png: Option<&[u8]>, state: &StatusBarState) {
    let bar = NSStatusBar::systemStatusBar();
    if !state.visible {
        STATUS_ITEM.with(|slot| {
            if let Some(live) = slot.borrow_mut().take() {
                bar.removeStatusItem(&live.item);
            }
        });
        return;
    }

    STATUS_ITEM.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let live = borrow.get_or_insert_with(|| {
            let item = bar.statusItemWithLength(NSVariableStatusItemLength);
            let target = StatusBarTarget::new();
            LiveStatusItem { item, target }
        });

        if let Some(button) = live.item.button(mtm) {
            if let Some(png) = icon_png
                && let Some(img) = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(png))
            {
                img.setSize(NSSize::new(18., 18.));
                img.setTemplate(false);
                button.setImage(Some(&img));
            }
            let title = if state.unread_mentions > 0 {
                format!(" {}", state.unread_mentions)
            } else if state.dnd_active {
                " \u{1F319}".to_owned()
            } else if state.has_unreads {
                " \u{2022}".to_owned()
            } else {
                String::new()
            };
            button.setTitle(&NSString::from_str(&title));
        }

        let menu = NSMenu::new(mtm);
        let target_obj: &AnyObject = &live.target;
        let add = |menu: &NSMenu, title: &str, action: objc2::runtime::Sel, tag: NSInteger| {
            // SAFETY: valid selector on `StatusBarTarget` retained for the menu's lifetime.
            let mi = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(mtm),
                    &NSString::from_str(title),
                    Some(action),
                    &NSString::from_str(""),
                )
            };
            // SAFETY: `target_obj` lives inside `LiveStatusItem` for at least as long as the
            // status item's menu holds `mi`.
            unsafe { mi.setTarget(Some(target_obj)) };
            mi.setTag(tag);
            menu.addItem(&mi);
        };

        add(&menu, "Open Slackless", sel!(showWindow:), -1);
        if !state.workspaces.is_empty() {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            for (ix, (name, mentions, has_unreads)) in state.workspaces.iter().enumerate() {
                let label = if *mentions > 0 {
                    format!("{name} ({mentions})")
                } else if *has_unreads {
                    format!("{name} \u{2022}")
                } else {
                    name.clone()
                };
                add(&menu, &label, sel!(switchWorkspace:), ix as NSInteger);
            }
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let dnd_label = if state.dnd_active {
            "Resume notifications"
        } else {
            "Pause notifications (1 hour)"
        };
        add(&menu, dnd_label, sel!(toggleDnd:), -1);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        add(&menu, "Quit Slackless", sel!(quitApp:), -1);

        live.item.setMenu(Some(&menu));
    });
}

impl AttentionRequester for MacPlatform {
    fn request_attention(&self, critical: bool) {
        run_on_main(move |mtm| {
            let app = NSApplication::sharedApplication(mtm);
            let kind = if critical {
                NSRequestUserAttentionType::CriticalRequest
            } else {
                NSRequestUserAttentionType::InformationalRequest
            };
            let _ = app.requestUserAttention(kind);
        });
    }
}

/// Precomputes an O(N) lookup table mapping every UTF-16 code-unit offset in `text` to its
/// corresponding UTF-8 byte offset (`None` at the trailing half of a surrogate pair).
fn utf16_to_utf8_table(text: &str, total_u16: usize) -> Vec<Option<usize>> {
    let mut table = vec![None; total_u16 + 1];
    let mut u16_pos = 0usize;
    for (byte_ix, ch) in text.char_indices() {
        if let Some(slot) = table.get_mut(u16_pos) {
            *slot = Some(byte_ix);
        }
        u16_pos += ch.len_utf16();
    }
    if let Some(slot) = table.get_mut(u16_pos) {
        *slot = Some(text.len());
    }
    table
}

impl SpellChecker for MacPlatform {
    fn check_spelling(&self, text: &str, language: Option<&str>) -> Vec<Misspelling> {
        if text.trim().is_empty() {
            return Vec::new();
        }
        let Some(_mtm) = MainThreadMarker::new() else {
            return Vec::new();
        };
        let checker = NSSpellChecker::sharedSpellChecker();
        let ns_text = NSString::from_str(text);
        let ns_lang = language.map(NSString::from_str);
        let total_u16 = ns_text.length();
        let u16_to_u8 = utf16_to_utf8_table(text, total_u16);
        let mut offset: usize = 0;
        let mut out = Vec::new();
        while offset < total_u16 {
            // SAFETY: valid `NSString` pointers; null `wordCount` out-param is allowed by AppKit.
            let found = unsafe {
                checker.checkSpellingOfString_startingAt_language_wrap_inSpellDocumentWithTag_wordCount(
                    &ns_text,
                    offset as NSInteger,
                    ns_lang.as_deref(),
                    false,
                    0,
                    std::ptr::null_mut(),
                )
            };
            if found.location == NSNotFound as usize || found.length == 0 || found.location < offset
            {
                break;
            }
            let end_u16 = found.location.saturating_add(found.length);
            if let (Some(start), Some(end)) = (
                u16_to_u8.get(found.location).copied().flatten(),
                u16_to_u8.get(end_u16).copied().flatten(),
            ) && start < end
            {
                out.push(Misspelling { range: start..end });
            }
            offset = end_u16.max(offset + 1);
            if out.len() >= 64 {
                break;
            }
        }
        out
    }

    fn spelling_guesses(&self, word: &str, language: Option<&str>) -> Vec<String> {
        if word.trim().is_empty() {
            return Vec::new();
        }
        let Some(_mtm) = MainThreadMarker::new() else {
            return Vec::new();
        };
        let checker = NSSpellChecker::sharedSpellChecker();
        let ns_word = NSString::from_str(word);
        let ns_lang = language.map(NSString::from_str);
        let range = NSRange::new(0, ns_word.length());
        let Some(guesses) = checker.guessesForWordRange_inString_language_inSpellDocumentWithTag(
            range,
            &ns_word,
            ns_lang.as_deref(),
            0,
        ) else {
            return Vec::new();
        };
        guesses.iter().take(5).map(|s| s.to_string()).collect()
    }

    fn learn_word(&self, word: &str) {
        if word.trim().is_empty() {
            return;
        }
        let Some(_mtm) = MainThreadMarker::new() else {
            return;
        };
        let checker = NSSpellChecker::sharedSpellChecker();
        checker.learnWord(&NSString::from_str(word));
    }
}

static MOUSE_NAV_MONITOR: Once = Once::new();

pub(crate) fn install_mouse_nav_monitor(
    on_nav: impl Fn(MouseNavDirection) + Send + Sync + 'static,
) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    MOUSE_NAV_MONITOR.call_once(|| {
        let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit passes a valid non-null `NSEvent` pointer to the local event monitor.
            let ev = unsafe { event.as_ref() };
            // GPUI's macOS backend already turns `NSEventType::Swipe` with `NSEventPhase::Ended`
            // into `MouseButton::Navigate`, but silently drops `phase == None` swipes (which
            // Logitech Options+, Magic Mouse, and swipe-based mice emit). Handle only `None`
            // here so we never double-fire.
            if ev.r#type() == NSEventType::Swipe && ev.phase() == NSEventPhase::None {
                let dx = ev.deltaX();
                if dx > 0.0 {
                    on_nav(MouseNavDirection::Back);
                    return std::ptr::null_mut();
                } else if dx < 0.0 {
                    on_nav(MouseNavDirection::Forward);
                    return std::ptr::null_mut();
                }
            }
            event.as_ptr()
        });
        // SAFETY: Installed once on the main thread; the returned monitor token is intentionally
        // leaked (`std::mem::forget`) so the monitor remains registered for the process lifetime.
        if let Some(monitor) = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::Swipe, &block)
        } {
            std::mem::forget(monitor);
        }
    });
}

static SCREEN_LOCK_MONITOR: Once = Once::new();

pub(crate) fn install_screen_lock_monitor(on_lock: impl Fn() + Send + Sync + 'static) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    SCREEN_LOCK_MONITOR.call_once(|| {
        let center = NSDistributedNotificationCenter::defaultCenter();
        let name = NSString::from_str("com.apple.screenIsLocked");
        let block = RcBlock::new(move |_notif: NonNull<NSNotification>| {
            on_lock();
        });
        // SAFETY: Installed once on the main thread with a valid notification name and block;
        // the returned observer token is leaked so it stays registered for the process lifetime.
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(&name), None, None, &block)
        };
        std::mem::forget(observer);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(text: &str, utf16_offset: usize) -> Option<usize> {
        let total = text.encode_utf16().count();
        let table = utf16_to_utf8_table(text, total);
        table.get(utf16_offset).copied().flatten()
    }

    #[test]
    fn utf16_to_utf8_table_handles_ascii_multibyte_surrogates_and_bounds() {
        // Empty string
        assert_eq!(lookup("", 0), Some(0));
        assert_eq!(lookup("", 1), None);

        // Pure ASCII
        assert_eq!(lookup("hello", 0), Some(0));
        assert_eq!(lookup("hello", 3), Some(3));
        assert_eq!(lookup("hello", 5), Some(5));
        assert_eq!(lookup("hello", 6), None);

        // Multi-byte BMP chars: 'é' (2 UTF-8 bytes, 1 UTF-16 unit), '中' (3 UTF-8 bytes, 1 UTF-16 unit)
        let bmp = "aé中b";
        assert_eq!(lookup(bmp, 0), Some(0));
        assert_eq!(lookup(bmp, 1), Some(1));
        assert_eq!(lookup(bmp, 2), Some(3));
        assert_eq!(lookup(bmp, 3), Some(6));
        assert_eq!(lookup(bmp, 4), Some(7));
        assert_eq!(lookup(bmp, 5), None);

        // Emoji surrogate pair: '😀' is 4 UTF-8 bytes and 2 UTF-16 code units.
        let emoji = "a😀b";
        assert_eq!(lookup(emoji, 0), Some(0));
        assert_eq!(lookup(emoji, 1), Some(1));
        assert_eq!(
            lookup(emoji, 2),
            None,
            "mid-surrogate offset must return None"
        );
        assert_eq!(lookup(emoji, 3), Some(5));
        assert_eq!(lookup(emoji, 4), Some(6));
        assert_eq!(lookup(emoji, 5), None);
    }

    #[test]
    fn status_bar_dispatch_queues_before_install_drains_atomically_and_forwards_after() {
        let dispatch = StatusBarDispatch::default();

        // Queued actions before monitor installation are returned FIFO by `take`.
        dispatch.emit(StatusBarAction::ShowWindow);
        dispatch.emit(StatusBarAction::ToggleDnd);
        assert_eq!(dispatch.take(), Some(StatusBarAction::ShowWindow));

        // Queue another action, then install monitor: remaining queued actions drain into callback.
        dispatch.emit(StatusBarAction::SwitchWorkspace(2));
        let received = Arc::new(Mutex::new(Vec::new()));
        let rec_clone = Arc::clone(&received);
        dispatch.install(move |action| {
            rec_clone.lock().unwrap().push(action);
        });

        // After installation, subsequent actions bypass the queue and go straight to the callback.
        dispatch.emit(StatusBarAction::Quit);
        assert_eq!(dispatch.take(), None);
        assert_eq!(
            *received.lock().unwrap(),
            vec![
                StatusBarAction::ToggleDnd,
                StatusBarAction::SwitchWorkspace(2),
                StatusBarAction::Quit,
            ]
        );
    }
}
