//! Slack's Preferences window (mounted in a gpui-kit dialog by the root): a left section nav
//! and the selected section's controls on the right, covering Notifications, Navigation,
//! Home, Appearance & Themes, Messages & media, Language & region, Accessibility, Mark as
//! read and Advanced.
//!
//! Every control writes the in-memory [`Settings`] and emits [`PreferencesEvent::Changed`];
//! the root applies what the app already honors (theme mode + color theme, Enter-to-send,
//! the notification schedule shown in the DND menu). The Color Mode is remembered locally, as
//! Slack keeps it on the client; persisting the other settings and syncing them with Slack's
//! `users.prefs.set` is a follow-up, until then they reset at launch.
//!
//! `Preferences::new(&mut Window, &mut App) -> Entity<Preferences>`
//! `Preferences::settings(&self) -> &Settings`
//! `Preferences::show(&mut self, Section, &mut Context<Self>)`
//! `Preferences::set_appearance_mode(&mut self, AppearanceMode, &mut Context<Self>)`
//! emits `PreferencesEvent::Changed`.

use gpui_kit::assets::IconName;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Div, ElementId, Entity, EventEmitter, FontWeight, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    WindowAppearance, div, px,
};

use crate::theme::{self, CUSTOM_ACCENTS, CUSTOM_SIDEBARS, Palette, SlackTheme};

pub enum PreferencesEvent {
    /// Some setting changed; read [`Preferences::settings`].
    Changed,
    /// User asked to reset the local SQLite & media cache in Preferences -> Advanced.
    ResetCache,
    /// User asked to compact/VACUUM the active workspace's local SQLite store.
    CompactStorage,
    /// User asked to prune cached media/temp files on disk.
    PruneMediaCache,
    /// User asked to export captured HTTP net-log JSON (`F-SHELL-158`).
    ExportNetLog,
    /// User clicked "Test speaker" in Preferences -> Audio & video (`F-COLLAB-040`).
    TestSpeaker(Option<String>),
    /// User clicked "Check for updates" in Preferences -> Advanced (`F-SHELL-163`).
    CheckForUpdates,
    /// User clicked "Download & restart" in Preferences -> Advanced (`F-SHELL-163`).
    InstallUpdate,
}

/// Snapshot of local SQLite & media cache metrics shown in Preferences -> Advanced.
#[derive(Clone, Default)]
pub struct StorageInfo {
    pub messages: u64,
    pub revisions: u64,
    pub conversations: u64,
    pub users: u64,
    pub offline_queued: u64,
    pub db_bytes: u64,
    pub media_cache_bytes: u64,
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.2} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.1} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

/// Slack's Color Mode: Light / Dark / System (follow the computer).
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum AppearanceMode {
    Light,
    Dark,
    System,
}

impl AppearanceMode {
    /// The theme this mode shows while the computer is in `appearance`.
    pub fn resolve(self, appearance: WindowAppearance) -> SlackTheme {
        match self {
            AppearanceMode::Light => SlackTheme::Light,
            AppearanceMode::Dark => SlackTheme::Dark,
            AppearanceMode::System => match appearance {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => SlackTheme::Dark,
                WindowAppearance::Light | WindowAppearance::VibrantLight => SlackTheme::Light,
            },
        }
    }

    /// The value slackless stores the mode under.
    pub fn key(self) -> &'static str {
        match self {
            AppearanceMode::Light => "light",
            AppearanceMode::Dark => "dark",
            AppearanceMode::System => "system",
        }
    }

    /// The mode stored as `key`; `None` for anything else.
    pub fn from_key(key: &str) -> Option<Self> {
        [
            AppearanceMode::Light,
            AppearanceMode::Dark,
            AppearanceMode::System,
        ]
        .into_iter()
        .find(|mode| mode.key() == key)
    }
}

/// The Preferences sections, in Slack's order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Notifications,
    Navigation,
    Home,
    Appearance,
    Messages,
    Language,
    AudioVideo,
    Accessibility,
    MarkAsRead,
    Advanced,
}

impl Section {
    const ALL: [Section; 10] = [
        Section::Notifications,
        Section::Navigation,
        Section::Home,
        Section::Appearance,
        Section::Messages,
        Section::Language,
        Section::AudioVideo,
        Section::Accessibility,
        Section::MarkAsRead,
        Section::Advanced,
    ];

    fn label(self) -> &'static str {
        match self {
            Section::Notifications => "Notifications",
            Section::Navigation => "Navigation",
            Section::Home => "Home",
            Section::Appearance => "Appearance & Themes",
            Section::Messages => "Messages & media",
            Section::Language => "Language & region",
            Section::AudioVideo => "Audio & video",
            Section::Accessibility => "Accessibility",
            Section::MarkAsRead => "Mark as read",
            Section::Advanced => "Advanced",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Section::Notifications => IconName::Bell,
            Section::Navigation => IconName::Compass,
            Section::Home => IconName::House,
            Section::Appearance => IconName::Palette,
            Section::Messages => IconName::MessageSquare,
            Section::Language => IconName::Globe,
            Section::AudioVideo => IconName::Headphones,
            Section::Accessibility => IconName::Accessibility,
            Section::MarkAsRead => IconName::CheckCheck,
            Section::Advanced => IconName::SlidersHorizontal,
        }
    }
}

pub const NOTIFY_LEVELS: [&str; 3] = [
    "All new messages",
    "Direct messages, mentions & keywords",
    "Nothing",
];
/// Mobile push handoff delay choices (`push_idle_wait`, `F-CORE-155`).
pub const MOBILE_HANDOFF: [&str; 7] = [
    "As soon as I'm inactive",
    "After 1 minute",
    "After 2 minutes",
    "After 5 minutes",
    "After 10 minutes",
    "After 15 minutes",
    "After 30 minutes",
];
const SOUNDS: [&str; 6] = ["Knock Brush", "Ding", "Plink", "Hummus", "Boing", "None"];
/// Where [`SOUNDS`] play from, in the same order: Slack's own notification sounds on its CDN
/// (its web client's `knock_brush`, `b2`, `confirm_delivery`, `hummus` and `animal_stick`
/// files), fetched the first time one plays; "None" plays nothing.
pub const SOUND_FILES: [Option<&str>; 6] = [
    Some("https://a.slack-edge.com/bv1-13/knock_brush-ac2b6e8.mp3"),
    Some("https://a.slack-edge.com/bv1-13/b2-5fd58e3.mp3"),
    Some("https://a.slack-edge.com/bv1-13/confirm_delivery-7da5529.mp3"),
    Some("https://a.slack-edge.com/bv1-13/hummus-2e2613c.mp3"),
    Some("https://a.slack-edge.com/bv1-13/animal_stick-caaec1f.mp3"),
    None,
];
pub const SCHEDULE_DAYS: [&str; 3] = ["Every day", "Weekdays", "Custom"];
pub const HOURS: [&str; 24] = [
    "12:00 AM", "1:00 AM", "2:00 AM", "3:00 AM", "4:00 AM", "5:00 AM", "6:00 AM", "7:00 AM",
    "8:00 AM", "9:00 AM", "10:00 AM", "11:00 AM", "12:00 PM", "1:00 PM", "2:00 PM", "3:00 PM",
    "4:00 PM", "5:00 PM", "6:00 PM", "7:00 PM", "8:00 PM", "9:00 PM", "10:00 PM", "11:00 PM",
];
const TAB_STYLES: [&str; 2] = ["Icons & text", "Icons only"];
const SORTS: [&str; 3] = ["Alphabetically", "By most recent activity", "By priority"];
const SHOW: [&str; 4] = [
    "All conversations",
    "Unreads only",
    "Mentions only",
    "Custom by section",
];
const SKIN_TONES: [&str; 6] = [
    "\u{270B}",
    "\u{270B}\u{1F3FB}",
    "\u{270B}\u{1F3FC}",
    "\u{270B}\u{1F3FD}",
    "\u{270B}\u{1F3FE}",
    "\u{270B}\u{1F3FF}",
];
const DENSITY: [&str; 2] = ["Clean", "Compact"];
const NAMES: [&str; 2] = ["Full & display names", "Just display names"];
const LANGUAGES: [&str; 7] = [
    "English (US)",
    "English (UK)",
    "Svenska",
    "Deutsch",
    "Fran\u{e7}ais",
    "Espa\u{f1}ol",
    "\u{65E5}\u{672C}\u{8A9E}",
];
const ZONES: [&str; 5] = [
    "(UTC+01:00) Stockholm",
    "(UTC+00:00) London",
    "(UTC-05:00) Eastern Time (US & Canada)",
    "(UTC-08:00) Pacific Time (US & Canada)",
    "(UTC+09:00) Tokyo",
];
const ZOOM: [&str; 6] = ["80%", "90%", "100%", "110%", "125%", "150%"];
const MARK_READ: [&str; 3] = [
    "Start me where I left off, and mark the channel read",
    "Start me at the newest message, and mark the channel read",
    "Start me at the newest message, but leave unseen messages unread",
];
/// Option 0 is `enter_sends == true`.
const ENTER_OPTIONS: [&str; 2] = [
    "Send the message (Shift+Enter starts a new line)",
    "Start a new line (\u{2318}+Enter sends)",
];
const OPEN_LINKS: [&str; 3] = ["Default browser", "Safari", "Firefox"];

/// Every preference, in memory. Pure-choice settings are indexes into the option tables
/// next to their controls; what the app acts on is typed.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    // Notifications
    pub desktop_notifications: bool,
    pub notify_level: usize,
    pub mobile_different: bool,
    /// Mobile notification level override when `mobile_different` is checked (`F-SHELL-043`).
    pub mobile_notify_level: usize,
    /// Mobile handoff idle wait index into [`MOBILE_HANDOFF`] (`push_idle_wait`, `F-CORE-155`).
    pub mobile_handoff: usize,
    /// Automatically pause notifications while a custom status is active (`F-SHELL-017`).
    pub status_pauses_notifications: bool,
    pub thread_replies: bool,
    /// "Also notify me about" extras (`F-SHELL-045`): `@channel` / `@here` mentions, huddle invites, and Slackbot/app reminders.
    pub notify_channel_mentions: bool,
    pub notify_huddle_invites: bool,
    pub notify_reminders: bool,
    pub keywords: String,
    /// Comma-separated VIP handles, display names, or user IDs that break through DND/Focus (`F-SHELL-056`, `F-SHELL-057`).
    pub vip_users: String,
    /// Dedicated notification sound for VIP messages (`F-SHELL-059`).
    pub vip_sound: usize,
    /// Allow urgent direct messages to break through Do Not Disturb & Focus mode (`F-SHELL-038`).
    pub urgent_dm_override: bool,
    /// Automatically set `"In focus mode"` status while Focus mode is active (`F-SHELL-039`).
    pub focus_auto_status: bool,
    /// Sync active macOS Calendar busy events to an automatic `"In a meeting"` status (`F-SHELL-024`).
    pub calendar_status_sync: bool,
    /// Channels where the user follows all new threads automatically (`F-CORE-111`).
    pub follow_all_threads_channels: Vec<String>,
    pub sound: usize,
    pub dm_sound: usize,
    pub show_preview: bool,
    pub mute_sounds: bool,
    pub show_dock_badge: bool,
    /// Show a badge dot on any unread message, not only DMs & mentions (`F-SHELL-178`).
    pub badge_on_unreads: bool,
    /// Bounce the dock icon on mentions & DMs while window is inactive (`F-SHELL-175`).
    pub bounce_dock: bool,
    pub schedule_enabled: bool,
    pub schedule_days: usize,
    /// Per-weekday flags (`Sun..Sat`, index 0 = Sunday) used when `schedule_days == 2` ("Custom") (`F-SHELL-034`, `F-SHELL-035`).
    pub schedule_custom_days: [bool; 7],
    pub schedule_start: usize,
    pub schedule_end: usize,
    // Navigation
    pub show_home: bool,
    pub show_dms: bool,
    pub show_activity: bool,
    pub show_files: bool,
    pub show_later: bool,
    /// Custom order of the 5 configurable rail tabs (`0=Home, 1=DMs, 2=Activity, 3=Files, 4=Later`; `F-CORE-014`).
    pub nav_order: [usize; 5],
    pub tab_style: usize,
    // Home
    pub show: usize,
    pub sort: usize,
    pub unreads_section: bool,
    pub separate_private: bool,
    pub profile_photos: bool,
    pub sidebar_compact: bool,
    /// Keep muted conversations visible in their sidebar sections instead of hiding them (`F-CORE-022`).
    pub show_muted_channels: bool,
    // Appearance & Themes
    pub mode: AppearanceMode,
    pub palette: Palette,
    pub window_gradient: bool,
    pub darker_sidebars: bool,
    pub font: usize,
    pub skin_tone: usize,
    pub jumbomoji: bool,
    pub convert_emoticons: bool,
    // Messages & media
    pub density: usize,
    pub names: usize,
    pub clock_24h: bool,
    pub emoji_text_mode: bool,
    pub inline_media: bool,
    pub link_previews: bool,
    pub large_files: bool,
    pub enter_sends: bool,
    pub format_markup: bool,
    /// Show the three one-click reaction emojis on the message hover bar (`show_quick_reactions`).
    pub show_quick_reactions: bool,
    /// Up to three one-click reaction shortcodes (without colons) shown on message hover.
    pub quick_reaction_emojis: [String; 3],
    // Language & region
    pub language: usize,
    pub timezone_auto: bool,
    pub timezone: usize,
    pub spellcheck: bool,
    // Accessibility
    pub zoom: usize,
    pub animate_emoji: bool,
    pub underline_links: bool,
    pub reduce_motion: bool,
    pub announce_incoming: bool,
    /// Hide extra chrome (workspace rail, decorative gradients) for a clean single-column feel (`F-SHELL-109`).
    pub simplified_layout: bool,
    /// Enable translucent macOS window vibrancy tint (`F-SHELL-110`).
    pub window_transparency: bool,
    // Mark as read
    pub mark_read: usize,
    pub confirm_mark_all: bool,
    /// Keep incoming/opened messages unread while the window is in the background (`F-SHELL-112`).
    pub no_bg_mark_read: bool,
    // Advanced
    pub confirm_unsend: bool,
    pub warn_external: bool,
    /// Comma-separated URL schemes allowed without an explicit protocol warning (`F-SHELL-193`).
    pub protocol_allowlist: String,
    pub launch_at_login: bool,
    /// Show a persistent menu-bar / tray icon with unread counts & quick actions (`F-SHELL-168`).
    pub show_menu_bar_icon: bool,
    /// Hide to menu bar instead of quitting when closing the main window (`F-SHELL-141`).
    pub close_to_tray: bool,
    pub hardware_acceleration: bool,
    pub debug_logging: bool,
    pub open_links: usize,
    /// Opt-in: automatically query Slack's remote search alongside instant local FTS5.
    pub remote_search_auto: bool,
    /// Whether `Cmd+F` scopes search to the open conversation (`true`) or searches all of Slack (`false`).
    pub search_in_channel: bool,
    /// Comma-separated channel names excluded from default local search results.
    pub excluded_channels: String,
    /// Whether the leftmost workspace switcher rail is shown (`Cmd+Shift+S`).
    pub show_workspace_rail: bool,
    /// Custom download folder path (empty string uses `~/Downloads`).
    pub download_dir: String,
    /// Prompt for a save location before every file download (`F-SHELL-190`).
    pub ask_download_location: bool,
    /// Optional HTTP/HTTPS/SOCKS proxy URL applied to Slack API requests (`F-SHELL-192`).
    pub proxy_url: String,
    /// Capture redacted HTTP request/response diagnostics in memory (`F-SHELL-158`).
    pub net_log_enabled: bool,
    // Audio & video (Huddles)
    /// Preferred microphone name (`None` = macOS system default).
    pub huddle_input_device: Option<String>,
    /// Preferred speaker name (`None` = macOS system default).
    pub huddle_output_device: Option<String>,
    /// Pure-Rust Sonora WebRTC AEC3 acoustic echo cancellation.
    pub huddle_aec: bool,
    /// Pure-Rust Sonora noise suppression.
    pub huddle_ns: bool,
    /// Pure-Rust Sonora automatic gain control (AGC2).
    pub huddle_agc: bool,
    /// Start every huddle with microphone muted (`F-COLLAB-049`).
    pub huddle_join_muted: bool,
    /// Automatically leave active huddle when the computer screen locks (`F-COLLAB-059`).
    pub huddle_leave_on_lock: bool,
    /// Automatically pop out the dedicated Huddle Window when joining a huddle.
    pub huddle_auto_open_window: bool,
    /// Preferred camera device index (`None` = system default camera 0, `F-COLLAB-046`).
    pub huddle_camera_device: Option<usize>,
    /// Enable software portrait background blur by default when turning on camera (`F-COLLAB-038`).
    pub huddle_blur_background: bool,
    /// Play synthesized join, leave, and incoming-invite ring chimes (`F-COLLAB-060`).
    pub huddle_sounds: bool,
    /// Show floating emoji reaction pills in the Huddle Window (`F-COLLAB-057`).
    pub huddle_reaction_animations: bool,
    /// Automatically enable live captions overlay when joining a huddle (`F-COLLAB-054`).
    pub huddle_auto_captions: bool,
    /// Play gentle ambient music when waiting alone in a huddle (`F-COLLAB-058`).
    pub huddle_music_when_alone: bool,
}

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
    cx.set_global(DisplayPrefs {
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
    });
    let keywords: Vec<String> = settings
        .keywords
        .split([',', '\n'])
        .map(|k| k.trim().to_lowercase())
        .filter(|k| k.len() >= 2)
        .collect();
    cx.set_global(KeywordPrefs(keywords));
    cx.set_global(QuickReactionPrefs(settings.resolved_quick_reactions()));
}

impl Settings {
    /// Whether the notification schedule lets notifications through now (always without
    /// one): inside its hours, on its days ("Custom" days count as every day here).
    pub fn notifications_allowed_now(&self) -> bool {
        if !self.schedule_enabled {
            return true;
        }
        let utc = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs() as i64);
        let local = utc + crate::messages::local_offset_now();
        let hour = (local.rem_euclid(86_400) / 3_600) as usize;
        // 1970-01-01 was a Thursday; 0 is Sunday.
        let weekday = (local.div_euclid(86_400) + 4).rem_euclid(7) as usize;
        let on_day = match self.schedule_days {
            0 => true,
            1 => (1..=5).contains(&weekday),
            _ => self
                .schedule_custom_days
                .get(weekday)
                .copied()
                .unwrap_or(true),
        };
        let (start, end) = (self.schedule_start, self.schedule_end);
        let in_hours = if start <= end {
            (start..end).contains(&hour)
        } else {
            hour >= start || hour < end
        };
        on_day && in_hours
    }
    /// Returns true if `user_id`, `name`, or `display_name` matches any configured VIP entry (`F-SHELL-056`).
    pub fn is_vip(&self, user_id: &str, name: &str, display_name: &str) -> bool {
        if self.vip_users.trim().is_empty() {
            return false;
        }
        self.vip_users
            .split([',', '\n'])
            .map(|entry| entry.trim().trim_start_matches('@'))
            .filter(|entry| !entry.is_empty())
            .any(|vip| {
                vip.eq_ignore_ascii_case(user_id)
                    || (!name.is_empty() && vip.eq_ignore_ascii_case(name))
                    || (!display_name.is_empty() && vip.eq_ignore_ascii_case(display_name))
            })
    }

    /// Returns true if an incoming DM should break through DND/Focus as urgent (`F-SHELL-038`).
    pub fn is_urgent_dm_breakthrough(&self, is_dm: bool, text: &str) -> bool {
        if !self.urgent_dm_override || !is_dm {
            return false;
        }
        let lower = text.to_ascii_lowercase();
        lower.contains("[urgent]")
            || lower.contains("urgent:")
            || lower.starts_with("urgent ")
            || text.contains('\u{1F6A8}')
            || lower.contains(":rotating_light:")
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            desktop_notifications: true,
            notify_level: 1,
            mobile_different: false,
            mobile_notify_level: 1,
            mobile_handoff: 0,
            status_pauses_notifications: false,
            thread_replies: true,
            notify_channel_mentions: true,
            notify_huddle_invites: true,
            notify_reminders: true,
            keywords: "deploy, incident".to_owned(),
            vip_users: String::new(),
            vip_sound: 1,
            urgent_dm_override: true,
            focus_auto_status: true,
            calendar_status_sync: false,
            follow_all_threads_channels: Vec::new(),
            sound: 0,
            dm_sound: 0,
            show_preview: true,
            mute_sounds: false,
            show_dock_badge: true,
            badge_on_unreads: false,
            bounce_dock: true,
            schedule_enabled: true,
            schedule_days: 1,
            schedule_custom_days: [false, true, true, true, true, true, false],
            schedule_start: 9,
            schedule_end: 18,
            show_home: true,
            show_dms: true,
            show_activity: true,
            show_files: true,
            show_later: true,
            nav_order: [0, 1, 2, 3, 4],
            tab_style: 0,
            show: 0,
            sort: 0,
            unreads_section: false,
            separate_private: false,
            profile_photos: true,
            sidebar_compact: false,
            show_muted_channels: false,
            mode: AppearanceMode::Dark,
            palette: Palette::Default,
            window_gradient: true,
            darker_sidebars: false,
            font: theme::DEFAULT_FONT,
            skin_tone: 0,
            jumbomoji: true,
            convert_emoticons: true,
            density: 0,
            names: 0,
            clock_24h: false,
            emoji_text_mode: false,
            inline_media: true,
            link_previews: true,
            large_files: true,
            enter_sends: true,
            format_markup: true,
            show_quick_reactions: true,
            quick_reaction_emojis: crate::actions::DEFAULT_QUICK_REACTIONS.map(String::from),
            language: 0,
            timezone_auto: true,
            timezone: 0,
            spellcheck: true,
            zoom: 2,
            animate_emoji: true,
            underline_links: false,
            reduce_motion: false,
            announce_incoming: false,
            simplified_layout: false,
            window_transparency: false,
            mark_read: 0,
            confirm_mark_all: true,
            no_bg_mark_read: true,
            confirm_unsend: true,
            warn_external: true,
            protocol_allowlist: "https, http, mailto, slack".to_owned(),
            launch_at_login: false,
            show_menu_bar_icon: true,
            close_to_tray: false,
            hardware_acceleration: true,
            debug_logging: false,
            open_links: 0,
            remote_search_auto: false,
            search_in_channel: true,
            excluded_channels: String::new(),
            show_workspace_rail: true,
            download_dir: String::new(),
            ask_download_location: false,
            proxy_url: String::new(),
            net_log_enabled: false,
            huddle_input_device: None,
            huddle_output_device: None,
            huddle_aec: true,
            huddle_ns: true,
            huddle_agc: true,
            huddle_join_muted: false,
            huddle_leave_on_lock: true,
            huddle_auto_open_window: false,
            huddle_camera_device: None,
            huddle_blur_background: false,
            huddle_sounds: true,
            huddle_reaction_animations: true,
            huddle_auto_captions: false,
            huddle_music_when_alone: true,
        }
    }
}

impl Settings {
    /// Clean, normalized list of one-click reaction shortcodes (empty when disabled).
    pub fn resolved_quick_reactions(&self) -> Vec<String> {
        if !self.show_quick_reactions {
            return Vec::new();
        }
        let custom: Vec<String> = self
            .quick_reaction_emojis
            .iter()
            .map(|raw| raw.trim().trim_matches(':').to_owned())
            .filter(|name| !name.is_empty())
            .collect();
        if custom.is_empty() {
            crate::actions::DEFAULT_QUICK_REACTIONS
                .map(String::from)
                .to_vec()
        } else {
            custom
        }
    }

    /// Apple / BCP-47 language tag for the chosen language preference (`F-SHELL-099`).
    pub fn spellcheck_language(&self) -> Option<&'static str> {
        const CODES: [&str; 7] = ["en_US", "en_GB", "sv", "de", "fr", "es", "ja"];
        CODES.get(self.language).copied()
    }

    /// Manual UTC offset in seconds when `timezone_auto` is off (`None` = system timezone).
    pub fn timezone_offset_secs(&self) -> Option<i64> {
        if self.timezone_auto {
            return None;
        }
        const OFFSETS: [i64; 5] = [3_600, 0, -18_000, -28_800, 32_400];
        Some(OFFSETS.get(self.timezone).copied().unwrap_or(0))
    }

    /// Slack's `push_idle_wait` value in seconds for [`Self::mobile_handoff`] (`F-CORE-155`).
    pub fn mobile_handoff_secs(&self) -> u32 {
        const SECS: [u32; 7] = [0, 60, 120, 300, 600, 900, 1_800];
        SECS.get(self.mobile_handoff).copied().unwrap_or(0)
    }

    /// Slack's `global_desktop` notification level key for [`Self::notify_level`] (`F-CORE-147`).
    pub fn notify_level_wire(&self) -> &'static str {
        Self::level_wire(self.notify_level)
    }

    /// Slack's `global_mobile` notification level key (`F-SHELL-043`).
    pub fn mobile_notify_level_wire(&self) -> &'static str {
        Self::level_wire(if self.mobile_different {
            self.mobile_notify_level
        } else {
            self.notify_level
        })
    }

    fn level_wire(index: usize) -> &'static str {
        match index {
            0 => "everything",
            2 => "nothing",
            _ => "mentions_dms",
        }
    }

    /// Slack's `dnd_days` wire value for [`Self::schedule_days`].
    pub fn schedule_days_wire(&self) -> &'static str {
        match self.schedule_days {
            0 => "every_day",
            2 => "custom",
            _ => "weekdays",
        }
    }

    /// `Weekdays, 9:00 AM – 6:00 PM`, or `Off` without a schedule.
    pub fn schedule_summary(&self) -> String {
        if !self.schedule_enabled {
            return "Off".to_owned();
        }
        format!(
            "{}, {} \u{2013} {}",
            SCHEDULE_DAYS[self.schedule_days], HOURS[self.schedule_start], HOURS[self.schedule_end]
        )
    }
}

/// Accessor for a checkbox's flag.
type Flag = fn(&mut Settings) -> &mut bool;
/// Accessor for a radio group's / select's option index.
type Choice = fn(&mut Settings) -> &mut usize;

pub struct Preferences {
    section: Section,
    settings: Settings,
    /// The select whose option list is expanded, by id.
    open_select: Option<&'static str>,
    keywords: Entity<TextareaState>,
    vip_input: Entity<TextareaState>,
    excluded_channels: Entity<TextareaState>,
    protocol_allowlist_input: Entity<TextareaState>,
    proxy_input: Entity<TextareaState>,
    quick_reaction_inputs: [Entity<TextareaState>; 3],
    custom_sidebar_hex: Entity<TextareaState>,
    custom_accent_hex: Entity<TextareaState>,
    slack_theme_import: Entity<TextareaState>,
    storage_stats: Option<StorageInfo>,
    cli_status_note: Option<String>,
    update_state: crate::updater::UpdateState,
    available_update: Option<crate::updater::ReleaseInfo>,
    audio_devices: slackless_core::huddle::AudioDevices,
    cameras: Vec<slackless_core::huddle::CameraDeviceInfo>,
    audio_loading: bool,
    audio_probe_gen: u64,
    mic_probe: Option<slackless_core::huddle::MicLevelProbe>,
    mic_level: f32,
    camera_preview_on: bool,
    camera_probe_gen: u64,
    camera_probe: Option<slackless_core::huddle::CameraPreviewProbe>,
    camera_frame: Option<std::sync::Arc<gpui_kit::gpui::RenderImage>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PreferencesEvent> for Preferences {}

impl Preferences {
    pub fn new(window: &mut Window, cx: &mut gpui_kit::gpui::App) -> Entity<Preferences> {
        cx.new(|cx| {
            let settings = Settings::default();
            let keywords = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("Add keywords, separated by commas")
                    .auto_grow(1, 3);
                state.set_value(settings.keywords.clone(), window, cx);
                state
            });
            let vip_input = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("Add VIP names, @handles, or user IDs (comma-separated)")
                    .auto_grow(1, 2);
                state.set_value(settings.vip_users.clone(), window, cx);
                state
            });
            let excluded_channels = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("e.g. random, alerts, noisy-bot")
                    .auto_grow(1, 2);
                state.set_value(settings.excluded_channels.clone(), window, cx);
                state
            });
            let protocol_allowlist_input = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("https, http, mailto, slack")
                    .auto_grow(1, 1);
                state.set_value(settings.protocol_allowlist.clone(), window, cx);
                state
            });
            let proxy_input = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("http://127.0.0.1:8080 or socks5://127.0.0.1:1080")
                    .auto_grow(1, 1);
                state.set_value(settings.proxy_url.clone(), window, cx);
                state
            });
            let quick_reaction_inputs: [Entity<TextareaState>; 3] = std::array::from_fn(|ix| {
                let initial = settings.quick_reaction_emojis[ix].clone();
                let placeholder = crate::actions::DEFAULT_QUICK_REACTIONS[ix];
                cx.new(|cx| {
                    let mut state = TextareaState::new(window, cx)
                        .placeholder(placeholder)
                        .auto_grow(1, 1);
                    state.set_value(initial, window, cx);
                    state
                })
            });
            let custom_sidebar_hex = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("#3F0E40")
                    .auto_grow(1, 1);
                state.set_value("#3F0E40".to_owned(), window, cx);
                state
            });
            let custom_accent_hex = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("#1164A3")
                    .auto_grow(1, 1);
                state.set_value("#1164A3".to_owned(), window, cx);
                state
            });
            let slack_theme_import = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("#3F0E40,#350D36,#1164A3,#FFFFFF,...")
                    .auto_grow(1, 2)
            });
            let subscriptions = vec![
                cx.subscribe_in(
                    &keywords,
                    window,
                    |this: &mut Preferences, keywords, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            this.settings.keywords = keywords.read(cx).value().to_string();
                            cx.emit(PreferencesEvent::Changed);
                        }
                    },
                ),
                cx.subscribe_in(
                    &vip_input,
                    window,
                    |this: &mut Preferences, field, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            this.settings.vip_users = field.read(cx).value().to_string();
                            cx.emit(PreferencesEvent::Changed);
                        }
                    },
                ),
                cx.subscribe_in(
                    &excluded_channels,
                    window,
                    |this: &mut Preferences, field, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            this.settings.excluded_channels = field.read(cx).value().to_string();
                            cx.emit(PreferencesEvent::Changed);
                        }
                    },
                ),
                cx.subscribe_in(
                    &protocol_allowlist_input,
                    window,
                    |this: &mut Preferences, field, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            this.settings.protocol_allowlist = field.read(cx).value().to_string();
                            cx.emit(PreferencesEvent::Changed);
                        }
                    },
                ),
                cx.subscribe_in(
                    &proxy_input,
                    window,
                    |this: &mut Preferences, field, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            this.settings.proxy_url = field.read(cx).value().trim().to_owned();
                            cx.emit(PreferencesEvent::Changed);
                        }
                    },
                ),
            ];
            let mut subscriptions = subscriptions;
            for (ix, input) in quick_reaction_inputs.iter().enumerate() {
                subscriptions.push(cx.subscribe_in(
                    input,
                    window,
                    move |this: &mut Preferences, field, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            let val = field.read(cx).value().trim().trim_matches(':').to_owned();
                            if this.settings.quick_reaction_emojis[ix] != val {
                                this.settings.quick_reaction_emojis[ix] = val;
                                cx.emit(PreferencesEvent::Changed);
                                cx.notify();
                            }
                        }
                    },
                ));
            }
            Preferences {
                section: Section::Notifications,
                settings,
                open_select: None,
                keywords,
                vip_input,
                excluded_channels,
                protocol_allowlist_input,
                proxy_input,
                quick_reaction_inputs,
                custom_sidebar_hex,
                custom_accent_hex,
                slack_theme_import,
                storage_stats: None,
                cli_status_note: None,
                update_state: crate::updater::UpdateState::Idle,
                available_update: None,
                audio_devices: slackless_core::huddle::AudioDevices::default(),
                cameras: Vec::new(),
                audio_loading: false,
                audio_probe_gen: 0,
                mic_probe: None,
                mic_level: 0.0,
                camera_preview_on: false,
                camera_probe_gen: 0,
                camera_probe: None,
                camera_frame: None,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Update the local SQLite & media cache metrics shown in Preferences -> Advanced.
    pub fn set_storage_stats(&mut self, stats: Option<StorageInfo>, cx: &mut Context<Self>) {
        self.storage_stats = stats;
        cx.notify();
    }

    /// Mirror the live auto-updater state into Preferences -> Advanced -> Software updates.
    pub fn set_update_state(
        &mut self,
        state: crate::updater::UpdateState,
        available: Option<crate::updater::ReleaseInfo>,
        cx: &mut Context<Self>,
    ) {
        self.update_state = state;
        self.available_update = available;
        cx.notify();
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Restore persisted settings at launch without emitting `Changed`.
    pub fn load_settings(
        &mut self,
        settings: Settings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keywords = settings.keywords.clone();
        let vip_users = settings.vip_users.clone();
        let excluded = settings.excluded_channels.clone();
        let protocols = settings.protocol_allowlist.clone();
        let proxy = settings.proxy_url.clone();
        let quick_emojis = settings.quick_reaction_emojis.clone();
        slackless_core::api::set_proxy_url(Some(proxy.clone()));
        slackless_core::api::set_net_log_enabled(settings.net_log_enabled);
        let rgb_pair = settings.palette.rgb_pair();
        self.settings = settings;
        self.keywords
            .update(cx, |state, cx| state.set_value(keywords, window, cx));
        self.vip_input
            .update(cx, |state, cx| state.set_value(vip_users, window, cx));
        self.excluded_channels
            .update(cx, |state, cx| state.set_value(excluded, window, cx));
        self.protocol_allowlist_input
            .update(cx, |state, cx| state.set_value(protocols, window, cx));
        self.proxy_input
            .update(cx, |state, cx| state.set_value(proxy, window, cx));
        for (ix, val) in quick_emojis.into_iter().enumerate() {
            self.quick_reaction_inputs[ix].update(cx, |state, cx| state.set_value(val, window, cx));
        }
        if let Some((sb, ac)) = rgb_pair {
            self.custom_sidebar_hex.update(cx, |state, cx| {
                state.set_value(theme::format_hex_rgb(sb), window, cx)
            });
            self.custom_accent_hex.update(cx, |state, cx| {
                state.set_value(theme::format_hex_rgb(ac), window, cx)
            });
        }
        cx.notify();
    }

    /// Jump to `section` (the DND menu opens Notifications).
    pub fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        self.open_select = None;
        if section == Section::AudioVideo {
            self.refresh_audio_devices_and_probe(cx);
        } else {
            self.stop_mic_probe(cx);
        }
        cx.notify();
    }

    /// Called when the Preferences window is re-opened on its remembered section: resumes the
    /// background audio probe only if currently viewing Audio & video.
    pub fn on_reopen(&mut self, cx: &mut Context<Self>) {
        if self.section == Section::AudioVideo && self.mic_probe.is_none() && !self.audio_loading {
            self.refresh_audio_devices_and_probe(cx);
        }
    }

    /// Release the live microphone & camera preview probes off the UI thread (called when leaving
    /// Audio & video or closing Preferences so hardware teardown never stalls the main thread).
    pub fn stop_mic_probe(&mut self, cx: &mut Context<Self>) {
        self.audio_probe_gen = self.audio_probe_gen.wrapping_add(1);
        self.audio_loading = false;
        if let Some(old) = self.mic_probe.take() {
            std::thread::spawn(move || drop(old));
        }
        self.mic_level = 0.0;
        self.stop_camera_preview(cx);
    }

    fn stop_camera_preview(&mut self, cx: &mut Context<Self>) {
        self.camera_probe_gen = self.camera_probe_gen.wrapping_add(1);
        self.camera_preview_on = false;
        if let Some(old) = self.camera_probe.take() {
            std::thread::spawn(move || drop(old));
        }
        if let Some(old) = self.camera_frame.take() {
            cx.drop_image(old, None);
        }
    }

    fn start_camera_preview(&mut self, cx: &mut Context<Self>) {
        self.stop_camera_preview(cx);
        self.camera_preview_on = true;
        let probe_gen = self.camera_probe_gen;
        let device_index = self.settings.huddle_camera_device;
        let blur = self.settings.huddle_blur_background;
        cx.spawn(async move |this, cx| {
            let probe = cx
                .background_executor()
                .spawn(async move {
                    slackless_core::huddle::CameraPreviewProbe::start(device_index, blur).ok()
                })
                .await;

            let started = this
                .update(cx, |prefs, cx| {
                    if prefs.camera_probe_gen != probe_gen || prefs.section != Section::AudioVideo {
                        if let Some(stale) = probe {
                            std::thread::spawn(move || drop(stale));
                        }
                        return false;
                    }
                    prefs.camera_probe = probe;
                    if prefs.camera_probe.is_none() {
                        prefs.camera_preview_on = false;
                    }
                    cx.notify();
                    prefs.camera_probe.is_some()
                })
                .unwrap_or(false);

            if !started {
                return;
            }
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(80))
                    .await;
                let keep = this
                    .update(cx, |prefs, cx| {
                        if prefs.camera_probe_gen != probe_gen {
                            return false;
                        }
                        if let Some(probe) = &prefs.camera_probe {
                            probe.set_blur(prefs.settings.huddle_blur_background);
                            if let Some(frame) = probe.latest_frame() {
                                if let Some(img) = crate::huddle_dock::frame_to_render_image(&frame)
                                {
                                    if let Some(old) = prefs.camera_frame.replace(img) {
                                        cx.drop_image(old, None);
                                    }
                                    cx.notify();
                                }
                            }
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    fn refresh_audio_devices_and_probe(&mut self, cx: &mut Context<Self>) {
        self.audio_probe_gen = self.audio_probe_gen.wrapping_add(1);
        let probe_gen = self.audio_probe_gen;
        self.audio_loading = true;
        if let Some(old) = self.mic_probe.take() {
            std::thread::spawn(move || drop(old));
        }
        let wanted = self.settings.huddle_input_device.clone();
        cx.spawn(async move |this, cx| {
            let (devices, cameras, probe) = cx
                .background_executor()
                .spawn(async move {
                    let devices = slackless_core::huddle::list_audio_devices();
                    let cameras = slackless_core::huddle::list_cameras();
                    let probe =
                        slackless_core::huddle::MicLevelProbe::start(wanted.as_deref()).ok();
                    (devices, cameras, probe)
                })
                .await;

            let started = this
                .update(cx, |prefs, cx| {
                    if prefs.audio_probe_gen != probe_gen || prefs.section != Section::AudioVideo {
                        if let Some(stale) = probe {
                            std::thread::spawn(move || drop(stale));
                        }
                        return false;
                    }
                    prefs.audio_devices = devices;
                    prefs.cameras = cameras;
                    prefs.audio_loading = false;
                    prefs.mic_probe = probe;
                    cx.notify();
                    prefs.mic_probe.is_some()
                })
                .unwrap_or(false);

            if !started {
                return;
            }
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(80))
                    .await;
                let keep = this
                    .update(cx, |prefs, cx| {
                        if prefs.audio_probe_gen != probe_gen {
                            return false;
                        }
                        if let Some(probe) = &prefs.mic_probe {
                            prefs.mic_level = probe.level();
                            cx.notify();
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    /// Show zoom level `level` (Cmd+= / Cmd+- / Cmd+0, or the remembered one) and apply it.
    pub fn set_zoom(&mut self, level: usize, cx: &mut Context<Self>) {
        if self.settings.zoom != level {
            self.settings.zoom = level;
            cx.emit(PreferencesEvent::Changed);
            cx.notify();
        }
    }

    /// Start on the remembered Color Mode and Font and the color theme Slack stores for the
    /// workspace, so the pane shows the look already in effect and a later change applies
    /// over it.
    pub fn set_appearance(
        &mut self,
        mode: AppearanceMode,
        palette: Palette,
        font: usize,
        cx: &mut Context<Self>,
    ) {
        let settings = &mut self.settings;
        if (settings.mode, settings.palette, settings.font) != (mode, palette, font) {
            (settings.mode, settings.palette, settings.font) = (mode, palette, font);
            cx.notify();
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(PreferencesEvent::Changed);
        cx.notify();
    }
}

impl Render for Preferences {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = self.render_body(cx);
        let nav = self.nav(cx);
        let title = self.section.label();
        h_flex()
            .h_full()
            .min_h(px(560.))
            .w_full()
            .items_start()
            .child(nav)
            .child(
                v_flex()
                    .id("prefs-body")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .px_6()
                    .py_4()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .pb_4()
                            .child(title),
                    )
                    .child(body),
            )
    }
}

/// Standalone native OS window wrapper hosting [`Preferences`] (`F-CORE-011`) so theme,
/// font, and layout changes can be previewed side-by-side with the main workspace window.
struct PreferencesWindow {
    preferences: Entity<Preferences>,
}

impl Drop for PreferencesWindow {
    fn drop(&mut self) {
        // Release any live microphone probe when the Preferences window closes.
        let _ = &self.preferences;
    }
}

impl Render for PreferencesWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::TitleBar;
        let theme = cx.theme();
        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                TitleBar::new()
                    .h(px(38.))
                    .bg(theme.background)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        h_flex().flex_1().h_full().pr_4().items_center().child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child("Preferences"),
                        ),
                    ),
            )
            .child(div().flex_1().min_h_0().child(self.preferences.clone()))
    }
}

/// Open Preferences in a standalone native GPUI window (`920 x 640`, `F-CORE-011`), centered
/// on `display_id` (the main window's display) or the primary display if `None`.
pub fn open_preferences_window(
    preferences: Entity<Preferences>,
    display_id: Option<gpui_kit::gpui::DisplayId>,
    cx: &mut gpui_kit::gpui::App,
) -> Option<gpui_kit::gpui::AnyWindowHandle> {
    use gpui_kit::component::TitleBar;
    use gpui_kit::gpui::{Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, size};
    let bounds = Bounds::centered(display_id, size(px(920.), px(640.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id,
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("Preferences")),
            traffic_light_position: Some(point(px(8.), px(12.))),
            ..TitleBar::title_bar_options()
        }),
        ..TitleBar::window_options()
    };
    gpui_kit::open_window(options, cx, move |window, cx| {
        window.set_window_title("Preferences — slackless");
        window.activate_window();
        let prefs_close = preferences.clone();
        window.on_window_should_close(cx, move |_, cx| {
            prefs_close.update(cx, |p, cx| p.stop_mic_probe(cx));
            true
        });
        cx.new(|_| PreferencesWindow { preferences })
    })
    .ok()
    .map(|(handle, _)| handle)
}

mod sections;
#[cfg(test)]
mod tests;
