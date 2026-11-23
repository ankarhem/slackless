//! macOS backend: dock badge, app icon and About panel via AppKit (`objc2`), secrets via the Keychain
//! (`security-framework`), deep-link schemes read from the bundle's
//! `Info.plist`. Notifications: bundled builds leave them to GPUI's
//! `SystemNotification` (`UNUserNotificationCenter`); unbundled dev builds,
//! where that center refuses to start, post through Notification Center via
//! `osascript` (see [`Notifier`]).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex, PoisonError};

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread as _, MainThreadMarker, MainThreadOnly as _, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAboutPanelOptionApplicationIcon, NSApplication, NSImage, NSMenu, NSMenuItem,
    NSRequestUserAttentionType, NSSound, NSSpellChecker, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{
    NSArray, NSBundle, NSData, NSDictionary, NSInteger, NSNotFound, NSNumber, NSObject, NSRange,
    NSSize, NSString, NSUserDefaults,
};
use security_framework::passwords::{self, PasswordOptions};

use crate::deep_link::{self, DeepLink};
use crate::{
    AboutPanel, AttentionRequester, BadgeSetter, DeepLinkHandler, FullScreenItem, IconSetter,
    Misspelling, Notifier, PlatformError, Result, SecretStore, SoundPlayer, SpellChecker,
    StatusBarAction, StatusBarItem, StatusBarState,
};

/// `errSecItemNotFound`, the documented OSStatus for a missing keychain item.
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// macOS integration. Cheap to construct; AppKit is only touched when a
/// capability needs it, always on the main thread (dispatched there if not).
pub struct MacPlatform {
    pending_link: Mutex<Option<DeepLink>>,
}

impl MacPlatform {
    pub fn new() -> Self {
        Self {
            pending_link: Mutex::new(None),
        }
    }
}

impl Default for MacPlatform {
    fn default() -> Self {
        Self::new()
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
            .args(["-e", NOTIFY_SCRIPT, title, body])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            // Reap the child off-thread so it never lingers as a zombie.
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
            }
            Err(error) => eprintln!("slackless-platform: notification not posted: {error}"),
        }
    }
}

impl BadgeSetter for MacPlatform {
    fn set_badge(&self, label: Option<&str>) {
        // NSApplication/dockTile must be touched on the main thread; hop to
        // the main queue when called from a worker thread (the GUI's tokio
        // bridge, backfill tasks, …). The label is moved as a plain `String`
        // so the closure stays `Send`.
        let label = label.map(str::to_string);
        if MainThreadMarker::new().is_some() {
            apply_dock_badge(label.as_deref().map(NSString::from_str).as_deref());
        } else {
            DispatchQueue::main().exec_async(move || {
                apply_dock_badge(label.as_deref().map(NSString::from_str).as_deref())
            });
        }
    }
}

fn apply_dock_badge(label: Option<&NSString>) {
    let Some(mtm) = MainThreadMarker::new() else {
        // Only reachable if the main-queue hop above never ran (no run loop);
        // silently skip rather than misreport.
        return;
    };
    // sharedApplication/dockTile take a MainThreadMarker, so they are safe to
    // call; we are on the main thread here by construction.
    let app = NSApplication::sharedApplication(mtm);
    let dock_tile = app.dockTile();
    dock_tile.setBadgeLabel(label);
}

impl IconSetter for MacPlatform {
    fn set_icon(&self, png: &[u8]) {
        let mtm = MainThreadMarker::new().expect("the app icon is set on the main thread");
        let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(png))
            .expect("the app icon is a PNG AppKit can read");
        // SAFETY: a non-nil image, set on the main thread.
        unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
    }
}

impl AboutPanel for MacPlatform {
    fn show_about(&self) {
        let mtm = MainThreadMarker::new().expect("the About panel opens on the main thread");
        let app = NSApplication::sharedApplication(mtm);
        // Unbundled, the standard panel shows the executable folder's icon: hand it the app's.
        let Some(icon) = app.applicationIconImage() else {
            return app.orderFrontStandardAboutPanel(None);
        };
        let icon: &AnyObject = &icon;
        // SAFETY: an AppKit constant, read on the main thread.
        let key = unsafe { NSAboutPanelOptionApplicationIcon };
        let options = NSDictionary::from_slices(&[key], &[icon]);
        // SAFETY: an About panel option key to an NSImage, the dictionary the method takes.
        unsafe { app.orderFrontStandardAboutPanelWithOptions(&options) };
    }
}

impl FullScreenItem for MacPlatform {
    fn drop_system_full_screen_item(&self) {
        let key = NSString::from_str("NSFullScreenMenuItemEverywhere");
        let off = NSNumber::numberWithBool(false);
        let off: &AnyObject = &off;
        let defaults = NSDictionary::from_slices(&[&*key], &[off]);
        // SAFETY: NSString keys to an NSNumber, the dictionary registerDefaults: takes.
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
            eprintln!("slackless-platform: not a playable sound");
            return;
        };
        sound.play();
        PLAYING.with(|playing| *playing.borrow_mut() = Some(sound));
    }
}

impl SecretStore for MacPlatform {
    fn store_secret(&self, service: &str, name: &str, secret: &[u8]) -> Result<()> {
        passwords::set_generic_password(service, name, secret)
            .map_err(|e| PlatformError::SecretStore(e.to_string()))
    }

    fn load_secret(&self, service: &str, name: &str) -> Result<Vec<u8>> {
        passwords::generic_password(PasswordOptions::new_generic_password(service, name)).map_err(
            |e| {
                if e.code() == ERR_SEC_ITEM_NOT_FOUND {
                    PlatformError::SecretNotFound {
                        service: service.to_string(),
                        name: name.to_string(),
                    }
                } else {
                    PlatformError::SecretStore(e.to_string())
                }
            },
        )
    }

    fn delete_secret(&self, service: &str, name: &str) -> Result<()> {
        match passwords::delete_generic_password(service, name) {
            Ok(()) => Ok(()),
            // Idempotent delete: removing an absent secret is fine.
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(PlatformError::SecretStore(e.to_string())),
        }
    }
}

impl DeepLinkHandler for MacPlatform {
    fn register(&self) -> Result<Vec<String>> {
        Ok(declared_url_schemes())
    }

    fn handle_url(&self, url: &str) -> Result<DeepLink> {
        let link = deep_link::parse_deep_link(url)?;
        *self
            .pending_link
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(link.clone());
        Ok(link)
    }

    fn take_link(&self) -> Option<DeepLink> {
        self.pending_link
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

/// URL schemes the running bundle declares in its `Info.plist`
/// (`CFBundleURLTypes`). Empty for unbundled dev runs — the `slack` scheme
/// must be declared there before macOS will deliver such links to the app.
fn declared_url_schemes() -> Vec<String> {
    let bundle = NSBundle::mainBundle();
    let Some(url_types) =
        bundle.objectForInfoDictionaryKey(&NSString::from_str("CFBundleURLTypes"))
    else {
        return Vec::new();
    };
    let Ok(url_types) = url_types.downcast::<NSArray<AnyObject>>() else {
        // Malformed CFBundleURLTypes: treat as no declared schemes.
        return Vec::new();
    };

    let mut schemes = Vec::new();
    for url_type in url_types.iter() {
        // Each entry is a dictionary of link-type metadata.
        let Ok(url_type) = url_type.downcast::<NSDictionary<AnyObject, AnyObject>>() else {
            continue;
        };
        let Some(scheme_list) = url_type
            .objectForKey(NSString::from_str("CFBundleURLSchemes").as_ref())
            .and_then(|list| list.downcast::<NSArray<AnyObject>>().ok())
        else {
            continue;
        };
        for scheme in scheme_list.iter() {
            let Ok(scheme) = scheme.downcast::<NSString>() else {
                continue;
            };
            schemes.push(scheme.to_string());
        }
    }
    schemes
}

static STATUS_ACTIONS: LazyLock<Mutex<VecDeque<StatusBarAction>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "SlacklessStatusBarTarget"]
    #[ivars = ()]
    struct StatusBarTarget;

    impl StatusBarTarget {
        #[unsafe(method(showWindow:))]
        fn show_window(&self, _sender: Option<&AnyObject>) {
            STATUS_ACTIONS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back(StatusBarAction::ShowWindow);
            if let Some(mtm) = MainThreadMarker::new() {
                let app = NSApplication::sharedApplication(mtm);
                app.unhide(None);
                #[allow(deprecated)]
                app.activateIgnoringOtherApps(true);
            }
        }

        #[unsafe(method(switchWorkspace:))]
        fn switch_workspace(&self, sender: Option<&NSMenuItem>) {
            let tag = sender.map(|item| item.tag()).unwrap_or(0);
            if tag >= 0 {
                STATUS_ACTIONS
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push_back(StatusBarAction::SwitchWorkspace(tag as usize));
            }
            if let Some(mtm) = MainThreadMarker::new() {
                let app = NSApplication::sharedApplication(mtm);
                app.unhide(None);
                #[allow(deprecated)]
                app.activateIgnoringOtherApps(true);
            }
        }

        #[unsafe(method(toggleDnd:))]
        fn toggle_dnd(&self, _sender: Option<&AnyObject>) {
            STATUS_ACTIONS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back(StatusBarAction::ToggleDnd);
        }

        #[unsafe(method(quitApp:))]
        fn quit_app(&self, _sender: Option<&AnyObject>) {
            STATUS_ACTIONS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back(StatusBarAction::Quit);
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
        if MainThreadMarker::new().is_some() {
            apply_status_bar(icon.as_deref(), &state);
        } else {
            DispatchQueue::main().exec_async(move || {
                apply_status_bar(icon.as_deref(), &state);
            });
        }
    }

    fn take_status_bar_action(&self) -> Option<StatusBarAction> {
        STATUS_ACTIONS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }
}

fn apply_status_bar(icon_png: Option<&[u8]>, state: &StatusBarState) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
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
            if let Some(png) = icon_png {
                if let Some(img) = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(png))
                {
                    img.setSize(NSSize::new(18., 18.));
                    img.setTemplate(false);
                    button.setImage(Some(&img));
                }
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
        let apply = move || {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let app = NSApplication::sharedApplication(mtm);
            let kind = if critical {
                NSRequestUserAttentionType::CriticalRequest
            } else {
                NSRequestUserAttentionType::InformationalRequest
            };
            let _ = app.requestUserAttention(kind);
        };
        if MainThreadMarker::new().is_some() {
            apply();
        } else {
            DispatchQueue::main().exec_async(apply);
        }
    }
}

/// Convert a UTF-16 code-unit offset into a UTF-8 byte offset inside `text`.
fn utf16_to_utf8_offset(text: &str, utf16_offset: usize) -> Option<usize> {
    let mut u16_pos = 0usize;
    for (byte_ix, ch) in text.char_indices() {
        if u16_pos == utf16_offset {
            return Some(byte_ix);
        }
        u16_pos += ch.len_utf16();
        if u16_pos > utf16_offset {
            return None;
        }
    }
    (u16_pos == utf16_offset).then_some(text.len())
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
        let total_u16 = text.encode_utf16().count();
        let mut offset: usize = 0;
        let mut out = Vec::new();
        while offset < total_u16 {
            // SAFETY: valid NSString pointers, null wordCount out-param allowed by AppKit.
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
                utf16_to_utf8_offset(text, found.location),
                utf16_to_utf8_offset(text, end_u16),
            ) {
                if start < end {
                    out.push(Misspelling { range: start..end });
                }
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
        let range = NSRange::new(0, word.encode_utf16().count());
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

use std::ptr::NonNull;
use std::sync::Once;

use block2::RcBlock;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventPhase, NSEventType};

use crate::MouseNavDirection;

static MOUSE_NAV_MONITOR: Once = Once::new();

pub(crate) fn install_mouse_nav_monitor(
    on_nav: impl Fn(MouseNavDirection) + Send + Sync + 'static,
) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    MOUSE_NAV_MONITOR.call_once(|| {
        let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
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
        // Keep the monitor alive for the lifetime of the process.
        if let Some(monitor) = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::Swipe, &block)
        } {
            std::mem::forget(monitor);
        }
    });
}
