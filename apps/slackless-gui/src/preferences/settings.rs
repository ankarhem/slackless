//! Persisted preference schema (`Settings`), section metadata, option tables, notification
//! schedule rules, and screen-reader formatting helpers.

use gpui_kit::assets::IconName;
use gpui_kit::gpui::WindowAppearance;
use slackless_core::types::time;

use crate::theme::{self, Palette, SlackTheme};

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
    /// Current `-wal` write-ahead log size.
    pub wal_bytes: u64,
    /// Free (reclaimable by Compact) database pages, in bytes.
    pub freelist_bytes: u64,
}

/// Media cache size cap choices for Archive & Storage (`(megabytes, label)`; `0` = unlimited).
pub const MEDIA_CACHE_SIZE_OPTIONS: [(u32, &str); 6] = [
    (128, "128 MB"),
    (256, "256 MB"),
    (512, "512 MB"),
    (1024, "1 GB"),
    (2048, "2 GB"),
    (0, "Unlimited"),
];
/// Media cache max-age choices for Archive & Storage (`(days, label)`; `0` = keep forever).
pub const MEDIA_CACHE_AGE_OPTIONS: [(u32, &str); 5] = [
    (7, "7 days"),
    (14, "14 days"),
    (30, "30 days"),
    (90, "90 days"),
    (0, "Never"),
];

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
    pub(super) const ALL: [Section; 10] = [
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

    pub(super) fn label(self) -> &'static str {
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

    pub(super) fn icon(self) -> IconName {
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
pub(super) const SOUNDS: [&str; 6] = ["Knock Brush", "Ding", "Plink", "Hummus", "Boing", "None"];
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
pub(super) const TAB_STYLES: [&str; 2] = ["Icons & text", "Icons only"];
pub(super) const SORTS: [&str; 3] = ["Alphabetically", "By most recent activity", "By priority"];
pub(super) const SHOW: [&str; 4] = [
    "All conversations",
    "Unreads only",
    "Mentions only",
    "Custom by section",
];
pub(super) const SKIN_TONES: [&str; 6] = [
    "\u{270B}",
    "\u{270B}\u{1F3FB}",
    "\u{270B}\u{1F3FC}",
    "\u{270B}\u{1F3FD}",
    "\u{270B}\u{1F3FE}",
    "\u{270B}\u{1F3FF}",
];
pub(super) const DENSITY: [&str; 2] = ["Clean", "Compact"];
pub(super) const NAMES: [&str; 2] = ["Full & display names", "Just display names"];
pub(super) const LANGUAGES: [&str; 7] = [
    "English (US)",
    "English (UK)",
    "Svenska",
    "Deutsch",
    "Fran\u{e7}ais",
    "Espa\u{f1}ol",
    "\u{65E5}\u{672C}\u{8A9E}",
];
pub(super) const ZONES: [&str; 5] = [
    "(UTC+01:00) Stockholm",
    "(UTC+00:00) London",
    "(UTC-05:00) Eastern Time (US & Canada)",
    "(UTC-08:00) Pacific Time (US & Canada)",
    "(UTC+09:00) Tokyo",
];
pub(super) const ZOOM: [&str; 6] = ["80%", "90%", "100%", "110%", "125%", "150%"];
/// Screen-reader message verbosity choices (`0` = Standard, `1` = Concise, `2` = Verbose; `F-SHELL-105`).
pub const MESSAGE_VERBOSITY: [&str; 3] = ["Standard", "Concise", "Verbose"];
pub(super) const MARK_READ: [&str; 3] = [
    "Start me where I left off, and mark the channel read",
    "Start me at the newest message, and mark the channel read",
    "Start me at the newest message, but leave unseen messages unread",
];
/// Option 0 is `enter_sends == true`.
pub(super) const ENTER_OPTIONS: [&str; 2] = [
    "Send the message (Shift+Enter starts a new line)",
    "Start a new line (\u{2318}+Enter sends)",
];
pub(super) const OPEN_LINKS: [&str; 3] = ["Default browser", "Safari", "Firefox"];
/// Opt-in huddle transcription provider selector (Preferences -> Audio & video).
pub const TRANSCRIPTION_PROVIDERS: [&str; 3] = [
    "Slack built-in (AWS Transcribe)",
    "Parakeet TDT 0.6B v3 (Local)",
    "Whisper Large v3 Turbo (Local)",
];

/// Persisted non-secret settings for local / BYO-key AI summaries.
///
/// The optional Bearer API key is NEVER stored here — it lives exclusively in the OS Keychain
/// (`slackless_auth::SecretStore`) under [`slackless_auth::AI_SUMMARY_API_KEY_ACCOUNT`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AiSummarySettings {
    pub enabled: bool,
    pub provider: slackless_core::summarize::AiProvider,
    pub base_url: String,
    pub model: String,
    pub timeout_secs: u64,
}

impl Default for AiSummarySettings {
    fn default() -> Self {
        let provider = slackless_core::summarize::AiProvider::Ollama;
        Self {
            enabled: false,
            provider,
            base_url: provider.default_base_url().to_owned(),
            model: provider.default_model().to_owned(),
            timeout_secs: slackless_core::summarize::DEFAULT_TIMEOUT_SECS,
        }
    }
}

impl AiSummarySettings {
    /// Effective base URL (falling back to the selected preset's default when empty).
    pub fn effective_base_url(&self) -> &str {
        let trimmed = self.base_url.trim();
        if trimmed.is_empty() {
            self.provider.default_base_url()
        } else {
            trimmed
        }
    }

    /// Effective model name (falling back to the selected preset's default when empty).
    pub fn effective_model(&self) -> &str {
        let trimmed = self.model.trim();
        if trimmed.is_empty() {
            self.provider.default_model()
        } else {
            trimmed
        }
    }

    /// Whether AI summaries are enabled with a non-empty endpoint and model.
    pub fn is_ready_to_run(&self) -> bool {
        self.enabled && !self.effective_base_url().is_empty() && !self.effective_model().is_empty()
    }

    /// Build a runtime [`slackless_core::summarize::AiSummaryConfig`] with an optional API key
    /// loaded from Keychain.
    pub fn to_runtime_config(
        &self,
        api_key: Option<String>,
    ) -> slackless_core::summarize::AiSummaryConfig {
        slackless_core::summarize::AiSummaryConfig {
            provider: self.provider,
            base_url: self.effective_base_url().to_owned(),
            model: self.effective_model().to_owned(),
            api_key,
            timeout_secs: self.timeout_secs,
            ..slackless_core::summarize::AiSummaryConfig::default()
        }
    }
}

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
    /// Screen-reader message verbosity (`0` = Standard, `1` = Concise, `2` = Verbose; `F-SHELL-105`).
    pub message_verbosity: u8,
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
    /// Opt into beta / pre-release GitHub Releases builds (`F-SHELL-164`).
    pub beta_updates: bool,
    /// Display pronouns on your profile by default (`F-SHELL-010`).
    pub show_pronouns: bool,
    // Privacy & archive ("beyond Slack" local features)
    /// Show locally preserved deleted messages inline as muted ghost tombstones.
    pub show_deleted_messages: bool,
    /// Stealth mode: suppress outbound typing indicators, read receipts, and active presence.
    pub stealth_mode: bool,
    /// Strip tracking parameters (utm_*, fbclid, gclid, si, etc.) from opened and sent links.
    pub strip_tracking_params: bool,
    /// Warn before sending messages that appear to contain API keys, tokens, or private keys.
    pub warn_before_sending_secrets: bool,
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
    /// Save finalized huddle captions into local SQLite FTS5 (text-only; zero audio on disk).
    pub huddle_save_transcripts: bool,
    /// Opt-in huddle transcription provider index into [`TRANSCRIPTION_PROVIDERS`]
    /// (`0` = Slack built-in, `1` = Parakeet TDT 0.6B v3 local ONNX, `2` = Whisper Large v3 Turbo).
    pub huddle_transcription_provider: usize,
    /// Play gentle ambient music when waiting alone in a huddle (`F-COLLAB-058`).
    pub huddle_music_when_alone: bool,
    // Local / BYO-key AI summaries (API key stays in OS Keychain via `slackless-auth`).
    pub ai_summary: AiSummarySettings,
    // Archive & storage hygiene
    /// Per-directory media cache size cap in megabytes (`0` = unlimited).
    pub media_cache_max_mb: u32,
    /// Evict cached media files untouched for this many days (`0` = never).
    pub media_cache_max_days: u32,
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
            message_verbosity: 0,
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
            beta_updates: false,
            show_pronouns: true,
            show_deleted_messages: false,
            stealth_mode: false,
            strip_tracking_params: true,
            warn_before_sending_secrets: true,
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
            huddle_save_transcripts: true,
            huddle_transcription_provider: 0,
            huddle_music_when_alone: true,
            ai_summary: AiSummarySettings::default(),
            media_cache_max_mb: slackless_core::maintenance::DEFAULT_MEDIA_CACHE_MAX_MB,
            media_cache_max_days: slackless_core::maintenance::DEFAULT_MEDIA_CACHE_MAX_DAYS,
        }
    }
}

impl Settings {
    /// Whether the notification schedule lets notifications through now (always without
    /// one): inside its hours, on its days ("Custom" days count as every day here).
    pub fn notifications_allowed_now(&self) -> bool {
        if !self.schedule_enabled {
            return true;
        }
        let utc = time::now_epoch_secs();
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

    /// Automatic media cache retention policy (size cap + max age).
    pub fn media_cache_policy(&self) -> slackless_core::maintenance::MediaCachePolicy {
        slackless_core::maintenance::MediaCachePolicy::from_mb_days(
            self.media_cache_max_mb,
            self.media_cache_max_days,
        )
    }

    /// Active GitHub Releases update channel (`F-SHELL-164`).
    pub fn update_channel(&self) -> slackless_core::domain::updater::UpdateChannel {
        if self.beta_updates {
            slackless_core::domain::updater::UpdateChannel::Beta
        } else {
            slackless_core::domain::updater::UpdateChannel::Stable
        }
    }

    /// Active huddle transcription provider (`SlackBuiltIn` by default; local models opt-in).
    pub fn transcription_provider(&self) -> slackless_core::huddle::TranscriptionProvider {
        slackless_core::huddle::TranscriptionProvider::from_index(
            self.huddle_transcription_provider,
        )
    }

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

    /// `Weekdays, 9:00 AM – 6:00 PM`, or `Off` without a schedule. Out-of-range indices
    /// (e.g. from corrupted persisted JSON) fall back cleanly to defaults.
    pub fn schedule_summary(&self) -> String {
        if !self.schedule_enabled {
            return "Off".to_owned();
        }
        let days = SCHEDULE_DAYS
            .get(self.schedule_days)
            .copied()
            .unwrap_or(SCHEDULE_DAYS[1]);
        let start = HOURS.get(self.schedule_start).copied().unwrap_or(HOURS[9]);
        let end = HOURS.get(self.schedule_end).copied().unwrap_or(HOURS[18]);
        format!("{days}, {start} \u{2013} {end}")
    }
}

/// Format a message for screen-reader announcements according to `verbosity`
/// (`0` = Standard, `1` = Concise, `2` = Verbose; `F-SHELL-105`, `F-SHELL-106`, `F-SHELL-279`).
pub fn format_accessible_message(
    author: &str,
    timestamp: &str,
    text: &str,
    reaction_count: usize,
    reply_count: usize,
    edited: bool,
    verbosity: u8,
) -> String {
    let body = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let body = if body.is_empty() {
        "(attachment or empty message)".to_owned()
    } else {
        body
    };
    let replies = match reply_count {
        0 => String::new(),
        1 => ", 1 reply".to_owned(),
        n => format!(", {n} replies"),
    };
    match verbosity {
        // Concise: author + body only
        1 => format!("{author}: {body}"),
        // Verbose: author, timestamp, body, edited flag, reactions, replies
        2 => {
            let edited_part = if edited { " (edited)" } else { "" };
            let reactions = match reaction_count {
                0 => String::new(),
                1 => ", 1 reaction".to_owned(),
                n => format!(", {n} reactions"),
            };
            format!("{author} at {timestamp}: {body}{edited_part}{reactions}{replies}")
        }
        // Standard (default): author, timestamp, body, and reply count when threaded
        _ => format!("{author}, {timestamp}: {body}{replies}"),
    }
}

/// Format a live cross-workspace unread & mention summary for screen readers (`F-SHELL-278`).
/// Each tuple is `(workspace_name, mention_count, has_unreads)`.
pub fn format_cross_workspace_summary(workspaces: &[(String, u32, bool)]) -> String {
    if workspaces.is_empty() {
        return "No workspaces signed in.".to_owned();
    }
    let total_mentions: u32 = workspaces.iter().map(|(_, m, _)| *m).sum();
    let unread_ws = workspaces.iter().filter(|(_, m, u)| *m > 0 || *u).count();
    if total_mentions == 0 && unread_ws == 0 {
        return format!(
            "All caught up across {} {}.",
            workspaces.len(),
            if workspaces.len() == 1 {
                "workspace"
            } else {
                "workspaces"
            }
        );
    }
    let parts: Vec<String> = workspaces
        .iter()
        .map(|(name, mentions, unread)| match (*mentions, *unread) {
            (0, false) => format!("{name}: all read"),
            (0, true) => format!("{name}: unread messages"),
            (1, _) => format!("{name}: 1 mention"),
            (n, _) => format!("{name}: {n} mentions"),
        })
        .collect();
    format!(
        "{} {}: {}",
        workspaces.len(),
        if workspaces.len() == 1 {
            "workspace"
        } else {
            "workspaces"
        },
        parts.join("; ")
    )
}
