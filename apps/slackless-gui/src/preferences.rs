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

fn format_bytes(bytes: u64) -> String {
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
    /// Allow urgent direct messages to break through Do Not Disturb & Focus mode (`F-SHELL-038`).
    pub urgent_dm_override: bool,
    /// Automatically set `"In focus mode"` status while Focus mode is active (`F-SHELL-039`).
    pub focus_auto_status: bool,
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
    // Mark as read
    pub mark_read: usize,
    pub confirm_mark_all: bool,
    /// Keep incoming/opened messages unread while the window is in the background (`F-SHELL-112`).
    pub no_bg_mark_read: bool,
    // Advanced
    pub confirm_unsend: bool,
    pub warn_external: bool,
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
            urgent_dm_override: true,
            focus_auto_status: true,
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
            mark_read: 0,
            confirm_mark_all: true,
            no_bg_mark_read: true,
            confirm_unsend: true,
            warn_external: true,
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
            huddle_music_when_alone: false,
        }
    }
}

impl Settings {
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
    proxy_input: Entity<TextareaState>,
    custom_sidebar_hex: Entity<TextareaState>,
    custom_accent_hex: Entity<TextareaState>,
    slack_theme_import: Entity<TextareaState>,
    storage_stats: Option<StorageInfo>,
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
            let proxy_input = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("http://127.0.0.1:8080 or socks5://127.0.0.1:1080")
                    .auto_grow(1, 1);
                state.set_value(settings.proxy_url.clone(), window, cx);
                state
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
            Preferences {
                section: Section::Notifications,
                settings,
                open_select: None,
                keywords,
                vip_input,
                excluded_channels,
                proxy_input,
                custom_sidebar_hex,
                custom_accent_hex,
                slack_theme_import,
                storage_stats: None,
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
        let proxy = settings.proxy_url.clone();
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
        self.proxy_input
            .update(cx, |state, cx| state.set_value(proxy, window, cx));
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
            self.stop_mic_probe();
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
    pub fn stop_mic_probe(&mut self) {
        self.audio_probe_gen = self.audio_probe_gen.wrapping_add(1);
        self.audio_loading = false;
        if let Some(old) = self.mic_probe.take() {
            std::thread::spawn(move || drop(old));
        }
        self.mic_level = 0.0;
        self.stop_camera_preview();
    }

    fn stop_camera_preview(&mut self) {
        self.camera_probe_gen = self.camera_probe_gen.wrapping_add(1);
        self.camera_preview_on = false;
        if let Some(old) = self.camera_probe.take() {
            std::thread::spawn(move || drop(old));
        }
        self.camera_frame = None;
    }

    fn start_camera_preview(&mut self, cx: &mut Context<Self>) {
        self.stop_camera_preview();
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

    fn nav(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (foreground, muted, hover, active, active_fg, border) = (
            theme.foreground,
            theme.muted_foreground,
            theme.accent,
            theme.sidebar_accent,
            theme.sidebar_accent_foreground,
            theme.border,
        );
        v_flex()
            .w(px(220.))
            .flex_none()
            .h_full()
            .gap_0p5()
            .p_2()
            .border_r_1()
            .border_color(border)
            .children(Section::ALL.iter().enumerate().map(|(ix, &section)| {
                let selected = section == self.section;
                let color = if selected { active_fg } else { foreground };
                h_flex()
                    .id(("prefs-section", ix))
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(color)
                    .when(selected, |row| {
                        row.bg(active).font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!selected, |row| row.hover(|el| el.bg(hover)))
                    .child(
                        Icon::new(section.icon())
                            .text_color(if selected { active_fg } else { muted })
                            .size(px(16.)),
                    )
                    .child(section.label())
                    .on_click(cx.listener(move |this, _, _, cx| this.show(section, cx)))
            }))
    }

    /// A titled group of controls, with an optional description under the title.
    fn group(
        &self,
        title: &str,
        hint: Option<&str>,
        body: impl IntoElement,
        cx: &Context<Self>,
    ) -> Div {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        v_flex()
            .gap_2()
            .pb_5()
            .mb_5()
            .border_b_1()
            .border_color(border)
            .child(div().font_weight(FontWeight::BOLD).child(title.to_owned()))
            .children(hint.map(|hint| div().text_sm().text_color(muted).child(hint.to_owned())))
            .child(body)
    }

    fn checkbox(
        &mut self,
        id: &'static str,
        label: &'static str,
        flag: Flag,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let on = *flag(&mut self.settings);
        let theme = cx.theme();
        let (fill, fill_fg, border) = (
            theme.sidebar_accent,
            theme.sidebar_accent_foreground,
            theme.input,
        );
        h_flex()
            .id(id)
            .items_center()
            .gap_2()
            .py_0p5()
            .cursor_pointer()
            .text_sm()
            .child(
                div()
                    .size(px(16.))
                    .flex_none()
                    .rounded(px(3.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .when(on, |b| b.bg(fill).border_color(fill))
                    .when(!on, |b| b.border_color(border))
                    .when(on, |b| {
                        b.child(Icon::new(IconName::Check).text_color(fill_fg).size(px(12.)))
                    }),
            )
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                let value = flag(&mut this.settings);
                *value = !*value;
                this.changed(cx);
            }))
    }

    fn radios(
        &mut self,
        id: &'static str,
        options: &'static [&'static str],
        choice: Choice,
        cx: &mut Context<Self>,
    ) -> Div {
        let selected = *choice(&mut self.settings);
        self.radio_rows(
            id,
            options,
            selected,
            move |settings, ix| *choice(settings) = ix,
            cx,
        )
    }

    /// Radio rows for `options` with `selected` checked; `pick` stores a click.
    fn radio_rows(
        &self,
        id: &'static str,
        options: &'static [&'static str],
        selected: usize,
        pick: impl Fn(&mut Settings, usize) + Copy + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (fill, border) = (theme.sidebar_accent, theme.input);
        v_flex()
            .gap_1()
            .children(options.iter().enumerate().map(|(ix, label)| {
                let on = ix == selected;
                h_flex()
                    .id(ElementId::NamedInteger(id.into(), ix as u64))
                    .items_center()
                    .gap_2()
                    .py_0p5()
                    .cursor_pointer()
                    .text_sm()
                    .child(
                        div()
                            .size(px(16.))
                            .flex_none()
                            .rounded_full()
                            .border_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .border_color(if on { fill } else { border })
                            .when(on, |dot| {
                                dot.child(div().size(px(8.)).rounded_full().bg(fill))
                            }),
                    )
                    .child(*label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        pick(&mut this.settings, ix);
                        this.changed(cx);
                    }))
            }))
    }

    /// A select: the current option in a bordered box; clicking expands the option list
    /// beneath it.
    fn select(
        &mut self,
        id: &'static str,
        options: &'static [&'static str],
        choice: Choice,
        cx: &mut Context<Self>,
    ) -> Div {
        let selected = *choice(&mut self.settings);
        let open = self.open_select == Some(id);
        let theme = cx.theme();
        let (border, muted, popover, hover, active) = (
            theme.input,
            theme.muted_foreground,
            theme.popover,
            theme.accent,
            theme.sidebar_accent,
        );
        let active_fg = theme.sidebar_accent_foreground;
        let trigger = h_flex()
            .id(id)
            .w(px(300.))
            .items_center()
            .justify_between()
            .px_3()
            .py_1p5()
            .rounded_md()
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .text_sm()
            .child(options[selected])
            .child(
                Icon::new(IconName::ChevronDown)
                    .text_color(muted)
                    .size(px(14.)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_select = if this.open_select == Some(id) {
                    None
                } else {
                    Some(id)
                };
                cx.notify();
            }));
        let list = open.then(|| {
            v_flex()
                .w(px(300.))
                .p_1()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(popover)
                .children(options.iter().enumerate().map(|(ix, label)| {
                    let on = ix == selected;
                    div()
                        .id(ElementId::NamedInteger(
                            SharedString::from(format!("{id}-opt")),
                            ix as u64,
                        ))
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_sm()
                        .when(on, |row| row.bg(active).text_color(active_fg))
                        .when(!on, |row| row.hover(|el| el.bg(hover)))
                        .child(*label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            *choice(&mut this.settings) = ix;
                            this.open_select = None;
                            this.changed(cx);
                        }))
                }))
        });
        v_flex().gap_1().child(trigger).children(list)
    }

    fn labeled(&self, label: &'static str, control: impl IntoElement) -> Div {
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(label),
            )
            .child(control)
    }

    fn notifications(&mut self, cx: &mut Context<Self>) -> Div {
        let desktop = self.checkbox(
            "notify-desktop",
            "Enable desktop notifications",
            |s| &mut s.desktop_notifications,
            cx,
        );
        let level = self.radios("notify-level", &NOTIFY_LEVELS, |s| &mut s.notify_level, cx);
        let mobile = self.checkbox(
            "notify-mobile",
            "Use different settings for my mobile devices",
            |s| &mut s.mobile_different,
            cx,
        );
        let mobile_level = if self.settings.mobile_different {
            let sel = self.select(
                "notify-mobile-level",
                &NOTIFY_LEVELS,
                |s| &mut s.mobile_notify_level,
                cx,
            );
            Some(self.labeled("On mobile, notify me about:", sel))
        } else {
            None
        };
        let mobile_handoff_select = self.select(
            "notify-mobile-handoff",
            &MOBILE_HANDOFF,
            |s| &mut s.mobile_handoff,
            cx,
        );
        let mobile_handoff = self.labeled(
            "When I'm not active on desktop, send notifications to my mobile devices:",
            mobile_handoff_select,
        );
        let status_pause = self.checkbox(
            "notify-status-pause",
            "Pause notifications automatically while a custom status is set",
            |s| &mut s.status_pauses_notifications,
            cx,
        );
        let threads = self.checkbox(
            "notify-threads",
            "Notify me about replies to threads I'm following",
            |s| &mut s.thread_replies,
            cx,
        );
        let ch_mentions = self.checkbox(
            "notify-ch-mentions",
            "Notify me about @channel, @here, and @everyone mentions",
            |s| &mut s.notify_channel_mentions,
            cx,
        );
        let huddle_invites = self.checkbox(
            "notify-huddle-invites",
            "Notify me when someone invites me to a huddle",
            |s| &mut s.notify_huddle_invites,
            cx,
        );
        let reminders = self.checkbox(
            "notify-reminders",
            "Notify me about Slackbot reminders and scheduled alerts",
            |s| &mut s.notify_reminders,
            cx,
        );
        let border = cx.theme().input;
        let keywords = div()
            .w(px(420.))
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(border)
            .text_sm()
            .child(
                Textarea::new(&self.keywords)
                    .appearance(false)
                    .bordered(false),
            );
        let schedule_on = self.checkbox(
            "schedule-on",
            "Only send notifications during these hours",
            |s| &mut s.schedule_enabled,
            cx,
        );
        let days = self.select(
            "schedule-days",
            &SCHEDULE_DAYS,
            |s| &mut s.schedule_days,
            cx,
        );
        let custom_days_row = (self.settings.schedule_days == 2).then(|| {
            const LABELS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
            let theme = cx.theme();
            let (accent, fg, muted, border) = (
                theme.primary,
                crate::theme::avatar_foreground(),
                theme.muted_foreground,
                theme.border,
            );
            h_flex()
                .flex_wrap()
                .gap_1p5()
                .children(LABELS.iter().enumerate().map(|(ix, label)| {
                    let active = self.settings.schedule_custom_days[ix];
                    div()
                        .id(("schedule-day-chip", ix))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .text_xs()
                        .cursor_pointer()
                        .when(active, |d| {
                            d.bg(accent)
                                .border_color(accent)
                                .text_color(fg)
                                .font_weight(FontWeight::SEMIBOLD)
                        })
                        .when(!active, |d| d.border_color(border).text_color(muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.settings.schedule_custom_days[ix] =
                                !this.settings.schedule_custom_days[ix];
                            this.changed(cx);
                        }))
                        .child(*label)
                }))
        });
        let start = self.select("schedule-start", &HOURS, |s| &mut s.schedule_start, cx);
        let end = self.select("schedule-end", &HOURS, |s| &mut s.schedule_end, cx);
        let sound = self.select("notify-sound", &SOUNDS, |s| &mut s.sound, cx);
        let dm_sound = self.select("notify-dm-sound", &SOUNDS, |s| &mut s.dm_sound, cx);
        let preview = self.checkbox(
            "notify-preview",
            "Include a preview of the message in each notification",
            |s| &mut s.show_preview,
            cx,
        );
        let mute = self.checkbox(
            "notify-mute",
            "Mute all messaging sounds from Slack",
            |s| &mut s.mute_sounds,
            cx,
        );
        let dock_badge = self.checkbox(
            "notify-dock-badge",
            "Show a badge on Slack's icon to indicate new activity",
            |s| &mut s.show_dock_badge,
            cx,
        );
        let badge_unreads = self.checkbox(
            "notify-badge-unreads",
            "Also show a dot badge for any unread message (not just DMs & mentions)",
            |s| &mut s.badge_on_unreads,
            cx,
        );
        let bounce_dock = self.checkbox(
            "notify-bounce-dock",
            "Bounce the dock icon for mentions & direct messages",
            |s| &mut s.bounce_dock,
            cx,
        );
        let vip_box = div()
            .w(px(420.))
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(border)
            .text_sm()
            .child(
                Textarea::new(&self.vip_input)
                    .appearance(false)
                    .bordered(false),
            );
        let urgent_override = self.checkbox(
            "notify-urgent-dm-override",
            "Allow urgent direct messages to break through Do Not Disturb & Focus mode",
            |s| &mut s.urgent_dm_override,
            cx,
        );
        let focus_status = self.checkbox(
            "notify-focus-auto-status",
            "Automatically set 'In focus mode' status during Focus mode",
            |s| &mut s.focus_auto_status,
            cx,
        );
        v_flex()
            .child(self.group(
                "Notify me about\u{2026}",
                None,
                v_flex()
                    .gap_2()
                    .child(desktop)
                    .child(level)
                    .child(mobile)
                    .children(mobile_level)
                    .child(mobile_handoff)
                    .child(status_pause)
                    .child(threads)
                    .child(ch_mentions)
                    .child(huddle_invites)
                    .child(reminders),
                cx,
            ))
            .child(self.group(
                "VIPs & Focus overrides",
                Some("Messages from VIP colleagues always notify you even during Do Not Disturb or Focus mode."),
                v_flex()
                    .gap_2()
                    .child(vip_box)
                    .child(urgent_override)
                    .child(focus_status),
                cx,
            ))
            .child(self.group(
                "My keywords",
                Some("You'll be notified whenever someone says one of your keywords."),
                keywords,
                cx,
            ))
            .child(self.group(
                "Notification schedule",
                Some("Your personal schedule overrides workspace default Do Not Disturb hours. Outside of these times, notifications will be paused."),
                v_flex()
                    .gap_3()
                    .child(schedule_on)
                    .child(self.labeled("Allow notifications", days))
                    .children(custom_days_row)
                    .child(h_flex().gap_4().items_start().child(self.labeled("From", start)).child(self.labeled("To", end))),
                cx,
            ))
            .child(self.group(
                "Sound & appearance",
                None,
                v_flex()
                    .gap_3()
                    .child(self.labeled("Notification sound (channels)", sound))
                    .child(self.labeled("Notification sound (DMs & mentions)", dm_sound))
                    .child(preview)
                    .child(mute)
                    .child(dock_badge)
                    .child(badge_unreads)
                    .child(bounce_dock),
                cx,
            ))
    }

    fn navigation(&mut self, cx: &mut Context<Self>) -> Div {
        let order = self.settings.nav_order;
        let rows: Vec<Div> = order
            .iter()
            .enumerate()
            .map(|(pos, &slot)| {
                let (id, label, flag): (&'static str, &'static str, Flag) = match slot {
                    0 => ("nav-home", "Home", |s| &mut s.show_home),
                    1 => ("nav-dms", "DMs", |s| &mut s.show_dms),
                    2 => ("nav-activity", "Activity", |s| &mut s.show_activity),
                    3 => ("nav-files", "Files", |s| &mut s.show_files),
                    _ => ("nav-later", "Later / To-dos", |s| &mut s.show_later),
                };
                let cb = self.checkbox(id, label, flag, cx);
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(cb)
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new(("nav-up", pos))
                                    .ghost()
                                    .with_size(Size::XSmall)
                                    .icon(IconName::ChevronUp)
                                    .disabled(pos == 0)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if pos > 0 {
                                            this.settings.nav_order.swap(pos - 1, pos);
                                            this.changed(cx);
                                        }
                                    })),
                            )
                            .child(
                                Button::new(("nav-down", pos))
                                    .ghost()
                                    .with_size(Size::XSmall)
                                    .icon(IconName::ChevronDown)
                                    .disabled(pos + 1 >= order.len())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if pos + 1 < this.settings.nav_order.len() {
                                            this.settings.nav_order.swap(pos, pos + 1);
                                            this.changed(cx);
                                        }
                                    })),
                            ),
                    )
            })
            .collect();
        let style = self.radios("nav-style", &TAB_STYLES, |s| &mut s.tab_style, cx);
        v_flex()
            .child(self.group(
                "Show & order navigation tabs",
                Some("Choose which tabs appear in the left rail and reorder them."),
                v_flex().gap_1().children(rows),
                cx,
            ))
            .child(self.group("Tab appearance", None, style, cx))
    }

    fn home(&mut self, cx: &mut Context<Self>) -> Div {
        let show = self.select("home-show", &SHOW, |s| &mut s.show, cx);
        let sort = self.select("home-sort", &SORTS, |s| &mut s.sort, cx);
        let unreads = self.checkbox(
            "home-unreads",
            "Show all unreads in one section",
            |s| &mut s.unreads_section,
            cx,
        );
        let private = self.checkbox(
            "home-private",
            "Separate private channels from public ones",
            |s| &mut s.separate_private,
            cx,
        );
        let photos = self.checkbox(
            "home-photos",
            "Show profile photos next to DMs",
            |s| &mut s.profile_photos,
            cx,
        );
        let compact_rows = self.checkbox(
            "home-compact",
            "Use compact sidebar rows",
            |s| &mut s.sidebar_compact,
            cx,
        );
        let show_muted = self.checkbox(
            "home-show-muted",
            "Show muted conversations in sidebar",
            |s| &mut s.show_muted_channels,
            cx,
        );
        v_flex()
            .child(
                self.group(
                    "Show and sort",
                    Some("Change the default for how conversations show and sort in your sidebar."),
                    v_flex()
                        .gap_3()
                        .child(self.labeled("Show", show))
                        .child(self.labeled("Sort", sort)),
                    cx,
                ),
            )
            .child(
                self.group(
                    "Organization",
                    None,
                    v_flex()
                        .gap_1()
                        .child(unreads)
                        .child(private)
                        .child(photos)
                        .child(compact_rows)
                        .child(show_muted),
                    cx,
                ),
            )
    }

    fn appearance(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (border, ring, is_dark) = (theme.border, theme.ring, theme.mode.is_dark());
        // Slack's Color Mode: three equal buttons in a row, icon beside the label, the chosen
        // one ringed in the accent.
        let mode = self.settings.mode;
        let modes = h_flex().gap_2().children(
            [
                (AppearanceMode::Light, "Light", IconName::Sun),
                (AppearanceMode::Dark, "Dark", IconName::Moon),
                (AppearanceMode::System, "System", IconName::Monitor),
            ]
            .into_iter()
            .enumerate()
            .map(|(ix, (choice, label, icon))| {
                let on = choice == mode;
                h_flex()
                    .id(("appearance-mode", ix))
                    .flex_1()
                    .h(px(38.))
                    .justify_center()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .border_1()
                    .border_color(if on { ring } else { border })
                    .when(on, |button| button.shadow_xs())
                    .child(Icon::new(icon).size(px(16.)))
                    .child(div().text_sm().child(label))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.mode = choice;
                        this.changed(cx);
                    }))
            }),
        );

        let palette = self.settings.palette;
        let named =
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .children(Palette::NAMED.iter().enumerate().map(|(ix, &choice)| {
                    let (side, accent) = choice.swatch(is_dark);
                    self.swatch_card(
                        ("palette", ix),
                        choice.name(),
                        side,
                        accent,
                        choice == palette,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.palette = choice;
                        this.changed(cx);
                    }))
                }));

        let (custom_sidebar, custom_accent) = match palette {
            Palette::Custom { sidebar, accent } => (Some(sidebar), Some(accent)),
            _ => (None, None),
        };
        let swatch_row = |kind: &'static str,
                          colors: &'static [u32],
                          current: Option<usize>,
                          cx: &mut Context<Self>| {
            h_flex()
                .gap_2()
                .children(colors.iter().enumerate().map(move |(ix, &hex)| {
                    let on = current == Some(ix);
                    div()
                        .id(ElementId::NamedInteger(kind.into(), ix as u64))
                        .size(px(28.))
                        .rounded_md()
                        .cursor_pointer()
                        .bg(theme::swatch_color(hex))
                        .border_2()
                        .border_color(if on { ring } else { border })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let (sidebar, accent) = match this.settings.palette {
                                Palette::Custom { sidebar, accent } => (sidebar, accent),
                                _ => (0, 0),
                            };
                            this.settings.palette = if kind == "custom-sidebar" {
                                Palette::Custom {
                                    sidebar: ix,
                                    accent,
                                }
                            } else {
                                Palette::Custom {
                                    sidebar,
                                    accent: ix,
                                }
                            };
                            this.changed(cx);
                        }))
                }))
        };
        let custom_hex_row = {
            let input_border = cx.theme().input;
            let sb_preview = theme::parse_hex_rgb(&self.custom_sidebar_hex.read(cx).value())
                .map(theme::swatch_color);
            let ac_preview = theme::parse_hex_rgb(&self.custom_accent_hex.read(cx).value())
                .map(theme::swatch_color);
            let hex_box = |state: &Entity<TextareaState>, preview: Option<gpui_kit::gpui::Hsla>| {
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_1()
                    .w(px(120.))
                    .rounded_md()
                    .border_1()
                    .border_color(input_border)
                    .text_xs()
                    .when_some(preview, |row, col| {
                        row.child(
                            div()
                                .flex_none()
                                .size(px(14.))
                                .rounded_xs()
                                .bg(col)
                                .border_1()
                                .border_color(border),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Textarea::new(state).appearance(false).bordered(false)),
                    )
            };
            h_flex()
                .items_center()
                .gap_2()
                .child(hex_box(&self.custom_sidebar_hex, sb_preview))
                .child(hex_box(&self.custom_accent_hex, ac_preview))
                .child(
                    Button::new("custom-hex-apply")
                        .outline()
                        .with_size(Size::Small)
                        .label("Apply hex")
                        .on_click(cx.listener(|this, _, _, cx| {
                            let sb =
                                theme::parse_hex_rgb(&this.custom_sidebar_hex.read(cx).value());
                            let ac = theme::parse_hex_rgb(&this.custom_accent_hex.read(cx).value());
                            if let (Some(sidebar), Some(accent)) = (sb, ac) {
                                this.settings.palette = Palette::CustomHex { sidebar, accent };
                                this.changed(cx);
                            }
                        })),
                )
        };
        let slack_theme_row = {
            let input_border = cx.theme().input;
            v_flex().gap_2().child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .max_w(px(360.))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(input_border)
                            .text_xs()
                            .child(
                                Textarea::new(&self.slack_theme_import)
                                    .appearance(false)
                                    .bordered(false),
                            ),
                    )
                    .child(
                        Button::new("slack-theme-import")
                            .outline()
                            .with_size(Size::Small)
                            .label("Import")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let raw = this.slack_theme_import.read(cx).value().to_string();
                                if let Some((sidebar, accent)) =
                                    theme::parse_slack_theme_string(&raw)
                                {
                                    this.settings.palette = Palette::CustomHex { sidebar, accent };
                                    this.custom_sidebar_hex.update(cx, |s, cx| {
                                        s.set_value(theme::format_hex_rgb(sidebar), window, cx)
                                    });
                                    this.custom_accent_hex.update(cx, |s, cx| {
                                        s.set_value(theme::format_hex_rgb(accent), window, cx)
                                    });
                                    this.changed(cx);
                                    crate::actions::notify(
                                        "Theme imported",
                                        "Applied Slack theme colors.",
                                        window,
                                        cx,
                                    );
                                } else {
                                    crate::actions::notify(
                                        "Invalid theme string",
                                        "Paste comma-separated #RRGGBB hex values from Slack.",
                                        window,
                                        cx,
                                    );
                                }
                            })),
                    )
                    .child(
                        Button::new("slack-theme-copy")
                            .outline()
                            .with_size(Size::Small)
                            .icon(IconName::Copy)
                            .label("Copy theme")
                            .tooltip("Copy Slack-compatible comma-separated hex theme string")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let exported = theme::format_slack_theme_string(
                                    this.settings.palette,
                                    is_dark,
                                );
                                cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(
                                    exported.clone(),
                                ));
                                crate::actions::notify("Slack theme copied", exported, window, cx);
                            })),
                    ),
            )
        };
        let simplified = self.checkbox(
            "appearance-simplified",
            "Simplified layout (hide workspace rail and extra chrome)",
            |s| &mut s.simplified_layout,
            cx,
        );
        let custom = v_flex()
            .gap_3()
            .child(self.labeled(
                "Sidebar",
                swatch_row("custom-sidebar", &CUSTOM_SIDEBARS, custom_sidebar, cx),
            ))
            .child(self.labeled(
                "Accent",
                swatch_row("custom-accent", &CUSTOM_ACCENTS, custom_accent, cx),
            ))
            .child(self.labeled("Custom hex colors (Sidebar / Accent)", custom_hex_row))
            .child(self.labeled(
                "Share or import Slack theme (comma-separated hex)",
                slack_theme_row,
            ))
            .child(simplified);

        let gradient = self.checkbox(
            "appearance-gradient",
            "Window gradient",
            |s| &mut s.window_gradient,
            cx,
        );
        let darker = self.checkbox(
            "appearance-darker-sidebars",
            "Darker sidebars",
            |s| &mut s.darker_sidebars,
            cx,
        );
        let font = self.select("appearance-font", &theme::FONT_LABELS, |s| &mut s.font, cx);
        let skin = self.radios("appearance-skin", &SKIN_TONES, |s| &mut s.skin_tone, cx);
        let jumbo = self.checkbox(
            "appearance-jumbo",
            "Display jumbomoji",
            |s| &mut s.jumbomoji,
            cx,
        );
        let emoticons = self.checkbox(
            "appearance-emoticons",
            "Convert my typed emoticons to emoji, so :D becomes \u{1F603}",
            |s| &mut s.convert_emoticons,
            cx,
        );

        v_flex()
            .child(self.group("Font", None, font, cx))
            .child(self.group("Color Mode", Some("Choose if Slack's appearance should be light or dark, or follow your computer's settings."), modes, cx))
            .child(self.group(
                "Slack themes",
                Some("Single-color themes that tint your sidebar, chrome, and highlights."),
                v_flex().gap_3().child(named).child(h_flex().gap_5().child(gradient).child(darker)),
                cx,
            ))
            .child(self.group("Custom theme", Some("Build your own: pick a sidebar and an accent color."), custom, cx))
            .child(self.group(
                "Emoji",
                None,
                v_flex().gap_2().child(self.labeled("Default skin tone", h_flex().child(skin))).child(jumbo).child(emoticons),
                cx,
            ))
    }

    /// A color-theme card: a mini sidebar + accent preview over the theme's name.
    fn swatch_card(
        &self,
        id: (&'static str, usize),
        name: &'static str,
        sidebar: gpui_kit::gpui::Hsla,
        accent: gpui_kit::gpui::Hsla,
        selected: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let (border, ring, background, muted) =
            (theme.border, theme.ring, theme.background, theme.muted);
        v_flex()
            .id(id)
            .w(px(110.))
            .gap_1()
            .p_1p5()
            .rounded_md()
            .cursor_pointer()
            .border_2()
            .border_color(if selected { ring } else { border })
            .child(
                h_flex()
                    .h(px(44.))
                    .rounded_sm()
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .w(px(36.))
                            .h_full()
                            .bg(sidebar)
                            .p_1()
                            .gap_1()
                            .child(div().h(px(5.)).w_full().rounded_sm().bg(accent)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .h_full()
                            .bg(background)
                            .p_1()
                            .gap_1()
                            .child(div().h(px(4.)).w(px(40.)).rounded_sm().bg(muted))
                            .child(div().h(px(4.)).w(px(28.)).rounded_sm().bg(muted)),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(if selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .child(name),
            )
    }

    fn messages(&mut self, cx: &mut Context<Self>) -> Div {
        let density = self.radios("msg-density", &DENSITY, |s| &mut s.density, cx);
        let names = self.radios("msg-names", &NAMES, |s| &mut s.names, cx);
        let clock24 = self.checkbox(
            "msg-clock-24h",
            "Display times with a 24-hour clock",
            |s| &mut s.clock_24h,
            cx,
        );
        let emoji_text = self.checkbox(
            "msg-emoji-text",
            "Display emoji as plain text (:smile:)",
            |s| &mut s.emoji_text_mode,
            cx,
        );
        let enter = self.radio_rows(
            "msg-enter",
            &ENTER_OPTIONS,
            usize::from(!self.settings.enter_sends),
            |settings, ix| settings.enter_sends = ix == 0,
            cx,
        );
        let markup = self.checkbox(
            "msg-markup",
            "Format messages with markup",
            |s| &mut s.format_markup,
            cx,
        );
        let inline = self.checkbox(
            "msg-inline",
            "Show images and files uploaded to Slack",
            |s| &mut s.inline_media,
            cx,
        );
        let previews = self.checkbox(
            "msg-previews",
            "Show images and files from linked websites",
            |s| &mut s.link_previews,
            cx,
        );
        let large = self.checkbox(
            "msg-large",
            "Even if they're larger than 2 MB",
            |s| &mut s.large_files,
            cx,
        );
        v_flex()
            .child(self.group("Message theme", None, density, cx))
            .child(
                self.group(
                    "Names & timestamps",
                    None,
                    v_flex()
                        .gap_2()
                        .child(names)
                        .child(clock24)
                        .child(emoji_text),
                    cx,
                ),
            )
            .child(self.group(
                "When writing a message, press Enter to\u{2026}",
                None,
                v_flex().gap_2().child(enter).child(markup),
                cx,
            ))
            .child(
                self.group(
                    "Inline media & links",
                    None,
                    v_flex()
                        .gap_1()
                        .child(inline)
                        .child(previews)
                        .child(div().pl_6().child(large)),
                    cx,
                ),
            )
    }

    fn language(&mut self, cx: &mut Context<Self>) -> Div {
        let language = self.select("lang", &LANGUAGES, |s| &mut s.language, cx);
        let auto = self.checkbox(
            "tz-auto",
            "Set time zone automatically",
            |s| &mut s.timezone_auto,
            cx,
        );
        let zone = self.select("tz", &ZONES, |s| &mut s.timezone, cx);
        let spell = self.checkbox(
            "spellcheck",
            "Enable spellcheck on your messages",
            |s| &mut s.spellcheck,
            cx,
        );
        v_flex()
            .child(self.group("Language", Some("Choose the language you want to use in Slack."), language, cx))
            .child(self.group("Time zone", Some("Slack uses your time zone to send summary and notification emails, and for times in your activity feeds and reminders."), v_flex().gap_2().child(auto).child(zone), cx))
            .child(self.group("Spellcheck", None, spell, cx))
    }

    fn accessibility(&mut self, cx: &mut Context<Self>) -> Div {
        let zoom = self.select("zoom", &ZOOM, |s| &mut s.zoom, cx);
        let animate = self.checkbox(
            "a11y-animate",
            "Allow animated images and emoji",
            |s| &mut s.animate_emoji,
            cx,
        );
        let underline = self.checkbox(
            "a11y-underline",
            "Underline links to websites",
            |s| &mut s.underline_links,
            cx,
        );
        let motion = self.checkbox("a11y-motion", "Reduce motion", |s| &mut s.reduce_motion, cx);
        let announce = self.checkbox(
            "a11y-announce",
            "Read incoming messages aloud with a screen reader",
            |s| &mut s.announce_incoming,
            cx,
        );
        v_flex()
            .child(self.group(
                "Zoom",
                Some("Use \u{2318}+ / \u{2318}- to zoom anywhere in Slack."),
                zoom,
                cx,
            ))
            .child(
                self.group(
                    "Animation & links",
                    None,
                    v_flex()
                        .gap_1()
                        .child(animate)
                        .child(underline)
                        .child(motion),
                    cx,
                ),
            )
            .child(self.group("Screen reader", None, announce, cx))
    }

    fn mark_as_read(&mut self, cx: &mut Context<Self>) -> Div {
        let mode = self.radios("mark-read", &MARK_READ, |s| &mut s.mark_read, cx);
        let confirm = self.checkbox(
            "mark-confirm",
            "Prompt to confirm when marking all messages as read (Shift+Esc)",
            |s| &mut s.confirm_mark_all,
            cx,
        );
        let no_bg = self.checkbox(
            "mark-no-bg",
            "Keep messages unread while window is in the background",
            |s| &mut s.no_bg_mark_read,
            cx,
        );
        v_flex()
            .child(self.group("When I view a channel", None, mode, cx))
            .child(self.group(
                "Mark everything read",
                None,
                v_flex().gap_1().child(confirm).child(no_bg),
                cx,
            ))
    }

    fn advanced(&mut self, cx: &mut Context<Self>) -> Div {
        let unsend = self.checkbox(
            "adv-unsend",
            "Ask before deleting a message",
            |s| &mut s.confirm_unsend,
            cx,
        );
        let external = self.checkbox(
            "adv-external",
            "Warn me about potentially malicious links",
            |s| &mut s.warn_external,
            cx,
        );
        let login = self.checkbox(
            "adv-login",
            "Launch slackless at login",
            |s| &mut s.launch_at_login,
            cx,
        );
        let menu_bar = self.checkbox(
            "adv-menu-bar",
            "Show slackless icon in the menu bar",
            |s| &mut s.show_menu_bar_icon,
            cx,
        );
        let close_tray = self.checkbox(
            "adv-close-tray",
            "Keep slackless running in the menu bar when closing the window",
            |s| &mut s.close_to_tray,
            cx,
        );
        let hardware = self.checkbox(
            "adv-hw",
            "Use hardware acceleration",
            |s| &mut s.hardware_acceleration,
            cx,
        );
        let debug = self.checkbox(
            "adv-debug",
            "Enable debug logging",
            |s| &mut s.debug_logging,
            cx,
        );
        let links = self.select("adv-links", &OPEN_LINKS, |s| &mut s.open_links, cx);
        let remote_search = self.checkbox(
            "adv-remote-search",
            "Also search Slack remotely by default (slower; local FTS5 stays instant otherwise)",
            |s| &mut s.remote_search_auto,
            cx,
        );
        let search_in_ch = self.checkbox(
            "adv-search-in-channel",
            "\u{2318}F searches the current conversation instead of all of Slack",
            |s| &mut s.search_in_channel,
            cx,
        );
        let excluded_border = cx.theme().input;
        let excluded_input = div()
            .w(px(420.))
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(excluded_border)
            .text_sm()
            .child(
                Textarea::new(&self.excluded_channels)
                    .appearance(false)
                    .bordered(false),
            );
        let compact_db = Button::new("adv-compact-db")
            .outline()
            .with_size(Size::Small)
            .label("Compact database (VACUUM)")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(PreferencesEvent::CompactStorage)));
        let prune_media = Button::new("adv-prune-media")
            .outline()
            .with_size(Size::Small)
            .label("Prune media cache")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(PreferencesEvent::PruneMediaCache)));
        let reset_cache = Button::new("adv-reset-cache")
            .outline()
            .with_size(Size::Small)
            .label("Clear local cache & reload")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(PreferencesEvent::ResetCache)));
        let storage_summary = {
            let muted = cx.theme().muted_foreground;
            let border = cx.theme().border;
            let card_bg = cx.theme().secondary.opacity(0.35);
            let body = match &self.storage_stats {
                Some(st) => v_flex()
                    .gap_1()
                    .child(
                        div().text_sm().child(format!(
                            "SQLite database: {} · Media cache: {}",
                            format_bytes(st.db_bytes),
                            format_bytes(st.media_cache_bytes),
                        )),
                    )
                    .child(div().text_xs().text_color(muted).child(format!(
                        "{} messages ({} revisions) · {} conversations · {} users · {} queued offline",
                        st.messages,
                        st.revisions,
                        st.conversations,
                        st.users,
                        st.offline_queued,
                    ))),
                None => v_flex().child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("Local SQLite metrics are shown when connected to a live workspace."),
                ),
            };
            v_flex()
                .gap_2()
                .p_3()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(card_bg)
                .child(body)
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(compact_db)
                        .child(prune_media)
                        .child(reset_cache),
                )
        };
        let dl_label = if self.settings.download_dir.is_empty() {
            "~/Downloads (default)".to_owned()
        } else {
            self.settings.download_dir.clone()
        };
        let has_custom_dl = !self.settings.download_dir.is_empty();
        let dl_muted = cx.theme().muted_foreground;
        let dl_border = cx.theme().input;
        let download_row = h_flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(dl_border)
                    .text_sm()
                    .text_color(dl_muted)
                    .child(dl_label),
            )
            .child(
                Button::new("adv-dl-choose")
                    .outline()
                    .with_size(Size::Small)
                    .label("Choose folder\u{2026}")
                    .on_click(cx.listener(|_, _, _, cx| {
                        let rx = cx.prompt_for_paths(PathPromptOptions {
                            files: false,
                            directories: true,
                            multiple: false,
                            prompt: Some("Choose download folder".into()),
                        });
                        cx.spawn(async move |this, cx| {
                            if let Ok(Ok(Some(paths))) = rx.await {
                                if let Some(path) = paths.into_iter().next() {
                                    let _ = this.update(cx, |prefs, cx| {
                                        prefs.settings.download_dir =
                                            path.to_string_lossy().into_owned();
                                        prefs.changed(cx);
                                    });
                                }
                            }
                        })
                        .detach();
                    })),
            )
            .when(has_custom_dl, |row| {
                row.child(
                    Button::new("adv-dl-reset")
                        .ghost()
                        .with_size(Size::Small)
                        .label("Reset")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.download_dir.clear();
                            this.changed(cx);
                        })),
                )
            });
        let ask_dl = self.checkbox(
            "adv-dl-ask",
            "Ask where to save each file before downloading",
            |s| &mut s.ask_download_location,
            cx,
        );
        let network_group = self.network_diagnostics_group(cx);
        v_flex()
            .child(self.group(
                "Search",
                Some("Local search is instant and works offline. Enable remote search when you want older history, unjoined channels, and server-indexed files automatically."),
                v_flex()
                    .gap_2()
                    .child(remote_search)
                    .child(search_in_ch)
                    .child(self.labeled("Exclude these channels from local search", excluded_input)),
                cx,
            ))
            .child(self.group(
                "Download location",
                Some("Where files downloaded from Slack are saved."),
                v_flex().gap_2().child(download_row).child(ask_dl),
                cx,
            ))
            .child(network_group)
            .child(self.group(
                "Other options",
                None,
                v_flex().gap_1().child(unsend).child(external),
                cx,
            ))
            .child(self.group("Open links in", None, links, cx))
            .child(self.group(
                "App",
                None,
                v_flex()
                    .gap_2()
                    .child(login)
                    .child(menu_bar)
                    .child(close_tray)
                    .child(hardware)
                    .child(debug),
                cx,
            ))
            .child(self.group(
                "Local storage & maintenance",
                Some("Durable local-first SQLite history & FTS5 search index for the active workspace."),
                storage_summary,
                cx,
            ))
    }

    fn network_diagnostics_group(&mut self, cx: &mut Context<Self>) -> Div {
        let dl_border = cx.theme().input;
        let proxy_field = div()
            .w(px(420.))
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(dl_border)
            .text_sm()
            .child(
                Textarea::new(&self.proxy_input)
                    .appearance(false)
                    .bordered(false),
            );
        let net_log_toggle = self.checkbox(
            "adv-net-log",
            "Capture HTTP net-log diagnostics (credentials redacted; last 500 requests)",
            |s| &mut s.net_log_enabled,
            cx,
        );
        let net_log_count = slackless_core::api::net_log_snapshot().len();
        let net_log_actions = h_flex()
            .items_center()
            .gap_2()
            .child(
                Button::new("adv-net-log-export")
                    .outline()
                    .with_size(Size::Small)
                    .icon(IconName::Download)
                    .label(format!("Export net log ({net_log_count})"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PreferencesEvent::ExportNetLog))),
            )
            .child(
                Button::new("adv-net-log-clear")
                    .ghost()
                    .with_size(Size::Small)
                    .label("Clear")
                    .on_click(cx.listener(|_, _, _, cx| {
                        slackless_core::api::clear_net_log();
                        cx.notify();
                    })),
            );
        self.group(
            "Network & diagnostics",
            Some("Optional HTTP/HTTPS/SOCKS proxy (leave empty to use system environment variables) and redacted HTTP net-log capture."),
            v_flex()
                .gap_2()
                .child(self.labeled("Proxy URL (applies to new connections)", proxy_field))
                .child(net_log_toggle)
                .child(net_log_actions),
            cx,
        )
    }
    fn audio_video(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (border, muted, success) = (theme.border, theme.muted_foreground, theme.success);
        let active_in = self.settings.huddle_input_device.clone();
        let active_out = self.settings.huddle_output_device.clone();
        let meter_pct = (self.mic_level * 3.5).clamp(0.0, 1.0);

        let mic_picker = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .child({
                        let is_def = active_in.is_none();
                        Button::new("prefs-mic-default")
                            .outline()
                            .with_size(Size::Small)
                            .label(if is_def {
                                "✓ System default"
                            } else {
                                "System default"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings.huddle_input_device = None;
                                this.refresh_audio_devices_and_probe(cx);
                                this.changed(cx);
                            }))
                    })
                    .children(
                        self.audio_devices
                            .inputs
                            .iter()
                            .enumerate()
                            .map(|(ix, dev)| {
                                let selected = active_in.as_deref() == Some(dev.name.as_str());
                                let name = dev.name.clone();
                                Button::new(("prefs-mic", ix))
                                    .outline()
                                    .with_size(Size::Small)
                                    .label(if selected {
                                        format!("✓ {}", dev.name)
                                    } else {
                                        dev.name.clone()
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.settings.huddle_input_device = Some(name.clone());
                                        this.refresh_audio_devices_and_probe(cx);
                                        this.changed(cx);
                                    }))
                            }),
                    ),
            )
            .when(
                self.audio_loading && self.audio_devices.inputs.is_empty(),
                |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Detecting audio devices…"),
                    )
                },
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().text_color(muted).child("Input level"))
                    .child(
                        div()
                            .w(px(320.))
                            .h(px(8.))
                            .rounded_full()
                            .bg(border.opacity(0.5))
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui_kit::gpui::relative(meter_pct))
                                    .rounded_full()
                                    .bg(success),
                            ),
                    ),
            );

        let speaker_picker =
            v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1p5()
                        .child({
                            let is_def = active_out.is_none();
                            Button::new("prefs-spk-default")
                                .outline()
                                .with_size(Size::Small)
                                .label(if is_def {
                                    "✓ System default"
                                } else {
                                    "System default"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.settings.huddle_output_device = None;
                                    this.changed(cx);
                                }))
                        })
                        .children(self.audio_devices.outputs.iter().enumerate().map(
                            |(ix, dev)| {
                                let selected = active_out.as_deref() == Some(dev.name.as_str());
                                let name = dev.name.clone();
                                Button::new(("prefs-spk", ix))
                                    .outline()
                                    .with_size(Size::Small)
                                    .label(if selected {
                                        format!("✓ {}", dev.name)
                                    } else {
                                        dev.name.clone()
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.settings.huddle_output_device = Some(name.clone());
                                        this.changed(cx);
                                    }))
                            },
                        )),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("prefs-test-speaker")
                                .outline()
                                .with_size(Size::Small)
                                .icon(IconName::Play)
                                .label("Test speaker")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.emit(PreferencesEvent::TestSpeaker(
                                        this.settings.huddle_output_device.clone(),
                                    ));
                                })),
                        )
                        .child(
                            Button::new("prefs-refresh-audio")
                                .ghost()
                                .with_size(Size::Small)
                                .icon(IconName::RefreshCw)
                                .label("Refresh devices")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.refresh_audio_devices_and_probe(cx);
                                    cx.notify();
                                })),
                        ),
                );

        let dsp = v_flex()
            .gap_1p5()
            .child(self.checkbox(
                "prefs-huddle-aec",
                "Echo cancellation (WebRTC AEC3)",
                |s| &mut s.huddle_aec,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-ns",
                "Background noise suppression",
                |s| &mut s.huddle_ns,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-agc",
                "Automatic microphone gain control",
                |s| &mut s.huddle_agc,
                cx,
            ));

        let behavior = v_flex()
            .gap_1p5()
            .child(self.checkbox(
                "prefs-huddle-join-muted",
                "Mute my microphone when joining a huddle",
                |s| &mut s.huddle_join_muted,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-leave-lock",
                "Automatically leave a huddle when my computer locks",
                |s| &mut s.huddle_leave_on_lock,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-auto-popout",
                "Automatically open the Huddle window when joining",
                |s| &mut s.huddle_auto_open_window,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-sounds",
                "Play join, leave, and incoming huddle ring chimes",
                |s| &mut s.huddle_sounds,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-reactions",
                "Show floating emoji reaction animations in huddles",
                |s| &mut s.huddle_reaction_animations,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-captions",
                "Automatically enable live captions when joining",
                |s| &mut s.huddle_auto_captions,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-music",
                "Play ambient music while waiting alone in a huddle",
                |s| &mut s.huddle_music_when_alone,
                cx,
            ));

        let camera_group = self.audio_video_camera(cx);

        v_flex()
            .child(camera_group)
            .child(self.group(
                "Microphone",
                Some("Choose which microphone slackless uses for native huddles."),
                mic_picker,
                cx,
            ))
            .child(self.group(
                "Speaker",
                Some("Choose where huddle audio plays."),
                speaker_picker,
                cx,
            ))
            .child(self.group(
                "Audio processing",
                Some("Pure-Rust real-time DSP applied to captured microphone audio."),
                dsp,
                cx,
            ))
            .child(self.group("When joining a huddle", None, behavior, cx))
    }

    fn audio_video_camera(&mut self, cx: &mut Context<Self>) -> Div {
        use gpui_kit::gpui::{ObjectFit, StyledImage as _, img};

        let theme = cx.theme();
        let (border, muted, card_bg) = (theme.border, theme.muted_foreground, theme.muted);
        let active_cam = self.settings.huddle_camera_device;
        let preview_on = self.camera_preview_on;

        let picker = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .child({
                        let is_def = active_cam.is_none();
                        Button::new("prefs-cam-default")
                            .outline()
                            .with_size(Size::Small)
                            .label(if is_def {
                                "✓ System default"
                            } else {
                                "System default"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings.huddle_camera_device = None;
                                if this.camera_preview_on {
                                    this.start_camera_preview(cx);
                                }
                                this.changed(cx);
                            }))
                    })
                    .children(self.cameras.iter().enumerate().map(|(ix, cam)| {
                        let selected = active_cam == Some(cam.index);
                        let cam_idx = cam.index;
                        Button::new(("prefs-cam", ix))
                            .outline()
                            .with_size(Size::Small)
                            .label(if selected {
                                format!("✓ {}", cam.name)
                            } else {
                                cam.name.clone()
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.settings.huddle_camera_device = Some(cam_idx);
                                if this.camera_preview_on {
                                    this.start_camera_preview(cx);
                                }
                                this.changed(cx);
                            }))
                    })),
            )
            .child(self.checkbox(
                "prefs-huddle-blur",
                "Blur my background by default when turning on camera",
                |s| &mut s.huddle_blur_background,
                cx,
            ))
            .child(
                h_flex().gap_2().child(
                    Button::new("prefs-cam-preview-toggle")
                        .outline()
                        .with_size(Size::Small)
                        .icon(if preview_on {
                            IconName::VideoOff
                        } else {
                            IconName::Video
                        })
                        .label(if preview_on {
                            "Stop camera preview"
                        } else {
                            "Preview camera"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.camera_preview_on {
                                this.stop_camera_preview();
                                cx.notify();
                            } else {
                                this.start_camera_preview(cx);
                            }
                        })),
                ),
            )
            .when(preview_on, |el| {
                let preview = div()
                    .w(px(280.))
                    .h(px(158.))
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(card_bg)
                    .border_1()
                    .border_color(border)
                    .flex()
                    .items_center()
                    .justify_center();
                el.child(if let Some(frame) = self.camera_frame.clone() {
                    preview.child(img(frame).w_full().h_full().object_fit(ObjectFit::Cover))
                } else {
                    preview.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Starting camera preview…"),
                    )
                })
            });

        self.group(
            "Camera",
            Some("Choose your webcam and optional portrait background blur for native huddles."),
            picker,
            cx,
        )
    }

    fn render_body(&mut self, cx: &mut Context<Self>) -> Div {
        match self.section {
            Section::Notifications => self.notifications(cx),
            Section::Navigation => self.navigation(cx),
            Section::Home => self.home(cx),
            Section::Appearance => self.appearance(cx),
            Section::Messages => self.messages(cx),
            Section::Language => self.language(cx),
            Section::AudioVideo => self.audio_video(cx),
            Section::Accessibility => self.accessibility(cx),
            Section::MarkAsRead => self.mark_as_read(cx),
            Section::Advanced => self.advanced(cx),
        }
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
            prefs_close.update(cx, |p, _| p.stop_mic_probe());
            true
        });
        cx.new(|_| PreferencesWindow { preferences })
    })
    .ok()
    .map(|(handle, _)| handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockkit::serde_json;

    #[test]
    fn format_bytes_scales_across_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(2 * 1024 * 1024), "2.0 MiB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.00 GiB");
    }

    #[test]
    fn appearance_mode_key_round_trips_and_resolves_system() {
        for mode in [
            AppearanceMode::Light,
            AppearanceMode::Dark,
            AppearanceMode::System,
        ] {
            assert_eq!(AppearanceMode::from_key(mode.key()), Some(mode));
        }
        assert_eq!(AppearanceMode::from_key("unknown"), None);
        assert!(AppearanceMode::System.resolve(WindowAppearance::Dark) == SlackTheme::Dark);
        assert!(
            AppearanceMode::System.resolve(WindowAppearance::VibrantLight) == SlackTheme::Light
        );
        assert!(AppearanceMode::Light.resolve(WindowAppearance::Dark) == SlackTheme::Light);
    }

    #[test]
    fn settings_json_is_backwards_compatible_with_partial_payloads() {
        let from_empty: Settings = serde_json::from_str("{}").expect("empty json deserializes");
        assert!(from_empty == Settings::default());

        let partial: Settings =
            serde_json::from_str(r#"{"clock_24h":true,"enter_sends":false,"zoom":4}"#)
                .expect("partial json deserializes");
        assert!(partial.clock_24h);
        assert!(!partial.enter_sends);
        assert_eq!(partial.zoom, 4);
        assert_eq!(partial.warn_external, Settings::default().warn_external);
        assert_eq!(partial.show_dock_badge, Settings::default().show_dock_badge);

        let round_trip: Settings =
            serde_json::from_str(&serde_json::to_string(&partial).unwrap()).unwrap();
        assert!(round_trip == partial);
    }

    #[test]
    fn spellcheck_language_timezone_and_schedule_helpers() {
        let mut s = Settings::default();
        assert_eq!(s.spellcheck_language(), Some("en_US"));
        s.language = 2;
        assert_eq!(s.spellcheck_language(), Some("sv"));
        s.language = 99;
        assert_eq!(s.spellcheck_language(), None);

        assert_eq!(s.timezone_offset_secs(), None);
        s.timezone_auto = false;
        s.timezone = 0;
        assert_eq!(s.timezone_offset_secs(), Some(3_600));
        s.timezone = 2;
        assert_eq!(s.timezone_offset_secs(), Some(-18_000));
        s.timezone = 99;
        assert_eq!(s.timezone_offset_secs(), Some(0));

        s.schedule_enabled = false;
        assert!(s.notifications_allowed_now());
        assert_eq!(s.schedule_summary(), "Off");

        s.notify_level = 0;
        assert_eq!(s.notify_level_wire(), "everything");
        s.notify_level = 1;
        assert_eq!(s.notify_level_wire(), "mentions_dms");
        s.notify_level = 2;
        assert_eq!(s.notify_level_wire(), "nothing");

        s.mobile_handoff = 3;
        assert_eq!(s.mobile_handoff_secs(), 300);
        s.schedule_days = 0;
        assert_eq!(s.schedule_days_wire(), "every_day");
        s.schedule_days = 1;
        assert_eq!(s.schedule_days_wire(), "weekdays");
        s.schedule_days = 2;
        assert_eq!(s.schedule_days_wire(), "custom");

        s.notify_level = 0;
        s.mobile_different = false;
        assert_eq!(s.mobile_notify_level_wire(), "everything");
        s.mobile_different = true;
        s.mobile_notify_level = 2;
        assert_eq!(s.mobile_notify_level_wire(), "nothing");

        s.schedule_enabled = true;
        s.schedule_days = 2;
        s.schedule_custom_days = [false; 7];
        assert!(!s.notifications_allowed_now());

        s.vip_users = "@ada, U999, Grace Hopper".to_owned();
        assert!(s.is_vip("U111", "ada", "Ada Lovelace"));
        assert!(s.is_vip("U999", "bob", "Bob"));
        assert!(s.is_vip("U222", "grace", "Grace Hopper"));
        assert!(!s.is_vip("U333", "carol", "Carol"));
        assert!(s.is_urgent_dm_breakthrough(true, "[URGENT] prod DB down"));
        assert!(s.is_urgent_dm_breakthrough(true, "\u{1F6A8} page on-call"));
        assert!(!s.is_urgent_dm_breakthrough(false, "[URGENT] channel chatter"));
        s.urgent_dm_override = false;
        assert!(!s.is_urgent_dm_breakthrough(true, "[URGENT] prod DB down"));
    }
}
