//! GPUI global caches derived from [`Settings`] (`DisplayPrefs`, `KeywordPrefs`,
//! `QuickReactionPrefs`) so message lists, threads, search, and hover toolbars update live.

use gpui_kit::gpui::{App, Global};

use super::settings::Settings;

/// Active rendering preferences cached as a GPUI global so every message list, thread panel,
/// search view, and unread card reflects Preferences changes live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayPrefs {
    pub compact: bool,
    pub clock_24h: bool,
    pub display_names_only: bool,
    pub jumbomoji: bool,
    pub convert_emoticons: bool,
    pub emoji_text_mode: bool,
    pub inline_media: bool,
    pub large_files: bool,
    pub link_previews: bool,
    pub underline_links: bool,
    pub animate_emoji: bool,
    pub reduce_motion: bool,
    pub show_deleted_messages: bool,
}

impl Default for DisplayPrefs {
    fn default() -> Self {
        Self {
            compact: false,
            clock_24h: false,
            display_names_only: false,
            jumbomoji: true,
            convert_emoticons: true,
            emoji_text_mode: false,
            inline_media: true,
            large_files: true,
            link_previews: true,
            underline_links: false,
            animate_emoji: true,
            reduce_motion: false,
            show_deleted_messages: false,
        }
    }
}

impl Global for DisplayPrefs {}

pub fn display_prefs(cx: &App) -> DisplayPrefs {
    cx.try_global::<DisplayPrefs>().copied().unwrap_or_default()
}

pub struct KeywordPrefs(pub Vec<String>);

impl Global for KeywordPrefs {}

pub struct QuickReactionPrefs(pub Vec<String>);

impl Global for QuickReactionPrefs {}

/// Active one-click reaction shortcodes for message hover toolbars. Reads the live global
/// `QuickReactionPrefs` when installed, falling back to `fallback` (e.g. in isolated tests).
pub fn active_quick_reactions(fallback: &[String], cx: &App) -> Vec<String> {
    match cx.try_global::<QuickReactionPrefs>() {
        Some(prefs) => prefs.0.clone(),
        None => fallback.to_vec(),
    }
}

pub fn matches_highlight_keyword(text: &str, cx: &App) -> bool {
    let Some(prefs) = cx.try_global::<KeywordPrefs>() else {
        return false;
    };
    if prefs.0.is_empty() || text.is_empty() {
        return false;
    }
    let lower = text.to_lowercase();
    prefs.0.iter().any(|kw| lower.contains(kw.as_str()))
}

pub fn set_display_prefs(settings: &Settings, cx: &mut App) {
    let next = DisplayPrefs {
        compact: settings.density == 1,
        clock_24h: settings.clock_24h,
        display_names_only: settings.names == 1,
        jumbomoji: settings.jumbomoji,
        convert_emoticons: settings.convert_emoticons,
        emoji_text_mode: settings.emoji_text_mode,
        inline_media: settings.inline_media,
        large_files: settings.large_files,
        link_previews: settings.link_previews,
        underline_links: settings.underline_links,
        animate_emoji: settings.animate_emoji,
        reduce_motion: settings.reduce_motion,
        show_deleted_messages: settings.show_deleted_messages,
    };
    let display_changed = cx.try_global::<DisplayPrefs>().copied() != Some(next);
    let keywords: Vec<String> = settings
        .keywords
        .split([',', '\n'])
        .map(|k| k.trim().to_lowercase())
        .filter(|k| k.len() >= 2)
        .collect();
    let keywords_changed = cx
        .try_global::<KeywordPrefs>()
        .is_none_or(|prefs| prefs.0 != keywords);
    let reactions = settings.resolved_quick_reactions();
    let reactions_changed = cx
        .try_global::<QuickReactionPrefs>()
        .is_none_or(|prefs| prefs.0 != reactions);
    // Replacing a global re-renders whatever observes it; only replace what changed.
    if display_changed {
        cx.set_global(next);
    }
    if keywords_changed {
        cx.set_global(KeywordPrefs(keywords));
    }
    if reactions_changed {
        cx.set_global(QuickReactionPrefs(reactions));
    }
    if display_changed {
        cx.refresh_windows();
    }
}
