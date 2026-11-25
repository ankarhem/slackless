//! Non-macOS fallback backend: compiles everywhere, touches no OS API.
//!
//! All capabilities are true no-ops until platform ports begin.

use crate::{
    AboutPanel, AttentionRequester, BadgeSetter, FullScreenItem, IconSetter, Misspelling, Notifier,
    SoundPlayer, SpellChecker, StatusBarAction, StatusBarItem, StatusBarState,
};

/// No-op backend for platforms without a port yet (and for headless tests).
#[derive(Default)]
pub struct FallbackPlatform;

impl FallbackPlatform {
    pub fn new() -> Self {
        Self
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

#[cfg(any(test, not(target_os = "macos")))]
pub(crate) fn install_mouse_nav_monitor(
    _on_nav: impl Fn(crate::MouseNavDirection) + Send + Sync + 'static,
) {
}

#[cfg(any(test, not(target_os = "macos")))]
pub(crate) fn install_screen_lock_monitor(_on_lock: impl Fn() + Send + Sync + 'static) {}

#[cfg(any(test, not(target_os = "macos")))]
pub(crate) fn install_status_bar_action_monitor(
    _on_action: impl Fn(StatusBarAction) + Send + Sync + 'static,
) {
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_noop_capabilities_work_safely() {
        let p = FallbackPlatform::new();

        p.notify("title", "body");
        p.set_badge(Some("3"));
        p.set_badge(None);
        p.set_icon(&[]);
        p.show_about();
        p.drop_system_full_screen_item();
        p.play_sound(&[]);
        p.update_status_bar(None, &StatusBarState::default());
        assert_eq!(p.take_status_bar_action(), None);
        p.request_attention(false);
        p.request_attention(true);
        assert!(p.check_spelling("helo", Some("en_US")).is_empty());
        assert!(p.spelling_guesses("helo", None).is_empty());
        p.learn_word("slackless");

        install_mouse_nav_monitor(|_| {});
        install_screen_lock_monitor(|| {});
        install_status_bar_action_monitor(|_| {});
    }
}
