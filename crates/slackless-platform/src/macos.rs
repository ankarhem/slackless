//! macOS backend: dock badge and app icon via AppKit (`objc2`), secrets via the Keychain
//! (`security-framework`), deep-link schemes read from the bundle's
//! `Info.plist`. Notifications: bundled builds leave them to GPUI's
//! `SystemNotification` (`UNUserNotificationCenter`); unbundled dev builds,
//! where that center refuses to start, post through Notification Center via
//! `osascript` (see [`Notifier`]).

use std::process::{Command, Stdio};
use std::sync::Mutex;

use dispatch2::DispatchQueue;
use objc2::AnyThread as _;
use objc2::MainThreadMarker;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::{NSArray, NSBundle, NSData, NSDictionary, NSString};
use security_framework::passwords::{self, PasswordOptions};

use crate::deep_link::{self, DeepLink};
use crate::{
    BadgeSetter, DeepLinkHandler, IconSetter, Notifier, PlatformError, Result, SecretStore,
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
            .expect("pending link mutex poisoned") = Some(link.clone());
        Ok(link)
    }

    fn take_link(&self) -> Option<DeepLink> {
        self.pending_link
            .lock()
            .expect("pending link mutex poisoned")
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
