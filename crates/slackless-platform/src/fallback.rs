//! Non-macOS fallback backend: compiles everywhere, touches no OS API.
//!
//! `Notifier`, `BadgeSetter`, `IconSetter`, `AboutPanel`, `FullScreenItem` and
//! `SoundPlayer` are true no-ops until their platform ports begin. `SecretStore` keeps secrets in-process only
//! (a `HashMap` behind a mutex) so the trait contract is exercised without a system keychain;
//! persistence arrives with each OS backend. `DeepLinkHandler` shares the
//! pure parser in [`crate::deep_link`]; nothing can be registered with an OS.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use crate::deep_link::{self, DeepLink};
use crate::{
    AboutPanel, AttentionRequester, BadgeSetter, DeepLinkHandler, FullScreenItem, IconSetter,
    Misspelling, Notifier, PlatformError, Result, SecretStore, SoundPlayer, SpellChecker,
    StatusBarAction, StatusBarItem, StatusBarState,
};

/// No-op backend for platforms without a port yet.
pub struct FallbackPlatform {
    secrets: Mutex<HashMap<(String, String), Vec<u8>>>,
    pending_link: Mutex<Option<DeepLink>>,
}

impl FallbackPlatform {
    pub fn new() -> Self {
        Self {
            secrets: Mutex::new(HashMap::new()),
            pending_link: Mutex::new(None),
        }
    }
}

impl Default for FallbackPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl Notifier for FallbackPlatform {
    /// No-op: surface notifications through the target front-end's own
    /// notification system once a port exists.
    fn notify(&self, _title: &str, _body: &str) {}
}

impl BadgeSetter for FallbackPlatform {
    /// No-op: no taskbar badge API is wired for this platform yet.
    fn set_badge(&self, _label: Option<&str>) {}
}

impl IconSetter for FallbackPlatform {
    /// No-op: no app icon API is wired for this platform yet.
    fn set_icon(&self, _png: &[u8]) {}
}

impl AboutPanel for FallbackPlatform {
    /// No-op: no About window is wired for this platform yet.
    fn show_about(&self) {}
}

impl FullScreenItem for FallbackPlatform {
    /// No-op: no system menu items to hold back on this platform yet.
    fn drop_system_full_screen_item(&self) {}
}

impl SoundPlayer for FallbackPlatform {
    /// No-op: no audio output is wired for this platform yet.
    fn play_sound(&self, _data: &[u8]) {}
}

impl SecretStore for FallbackPlatform {
    fn store_secret(&self, service: &str, name: &str, secret: &[u8]) -> Result<()> {
        self.secrets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((service.to_string(), name.to_string()), secret.to_vec());
        Ok(())
    }

    fn load_secret(&self, service: &str, name: &str) -> Result<Vec<u8>> {
        self.secrets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(service.to_string(), name.to_string()))
            .cloned()
            .ok_or_else(|| PlatformError::SecretNotFound {
                service: service.to_string(),
                name: name.to_string(),
            })
    }

    fn delete_secret(&self, service: &str, name: &str) -> Result<()> {
        self.secrets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(service.to_string(), name.to_string()));
        Ok(())
    }
}

impl DeepLinkHandler for FallbackPlatform {
    fn register(&self) -> Result<Vec<String>> {
        // Nothing to register: OS scheme delivery is wired by each port.
        Ok(Vec::new())
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

impl StatusBarItem for FallbackPlatform {
    fn update_status_bar(&self, _icon_png: Option<&[u8]>, _state: &StatusBarState) {}

    fn take_status_bar_action(&self) -> Option<StatusBarAction> {
        None
    }
}

impl AttentionRequester for FallbackPlatform {
    fn request_attention(&self, _critical: bool) {}
}

impl SpellChecker for FallbackPlatform {
    fn check_spelling(&self, _text: &str, _language: Option<&str>) -> Vec<Misspelling> {
        Vec::new()
    }

    fn spelling_guesses(&self, _word: &str, _language: Option<&str>) -> Vec<String> {
        Vec::new()
    }

    fn learn_word(&self, _word: &str) {}
}

pub(crate) fn install_mouse_nav_monitor(
    _on_nav: impl Fn(crate::MouseNavDirection) + Send + Sync + 'static,
) {
}

pub(crate) fn install_screen_lock_monitor(_on_lock: impl Fn() + Send + Sync + 'static) {}
