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
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AppContext as _, Context, Div, ElementId, Entity, EventEmitter, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, WindowAppearance, div, px,
};

use crate::theme::{self, CUSTOM_ACCENTS, CUSTOM_SIDEBARS, Palette, SlackTheme};

pub enum PreferencesEvent {
    /// Some setting changed; read [`Preferences::settings`].
    Changed,
}

/// Slack's Color Mode: Light / Dark / System (follow the computer).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    Accessibility,
    MarkAsRead,
    Advanced,
}

impl Section {
    const ALL: [Section; 9] = [
        Section::Notifications,
        Section::Navigation,
        Section::Home,
        Section::Appearance,
        Section::Messages,
        Section::Language,
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
const SOUNDS: [&str; 6] = ["Knock Brush", "Ding", "Plink", "Hummus", "Boing", "None"];
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
#[derive(Clone)]
pub struct Settings {
    // Notifications
    pub notify_level: usize,
    pub mobile_different: bool,
    pub thread_replies: bool,
    pub keywords: String,
    pub sound: usize,
    pub show_preview: bool,
    pub mute_sounds: bool,
    pub schedule_enabled: bool,
    pub schedule_days: usize,
    pub schedule_start: usize,
    pub schedule_end: usize,
    // Navigation
    pub show_home: bool,
    pub show_dms: bool,
    pub show_activity: bool,
    pub show_files: bool,
    pub show_later: bool,
    pub tab_style: usize,
    // Home
    pub show: usize,
    pub sort: usize,
    pub unreads_section: bool,
    pub separate_private: bool,
    pub profile_photos: bool,
    // Appearance & Themes
    pub mode: AppearanceMode,
    pub palette: Palette,
    pub font: usize,
    pub skin_tone: usize,
    pub jumbomoji: bool,
    pub convert_emoticons: bool,
    // Messages & media
    pub density: usize,
    pub names: usize,
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
    // Mark as read
    pub mark_read: usize,
    pub confirm_mark_all: bool,
    // Advanced
    pub confirm_unsend: bool,
    pub warn_external: bool,
    pub launch_at_login: bool,
    pub hardware_acceleration: bool,
    pub debug_logging: bool,
    pub open_links: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            notify_level: 1,
            mobile_different: false,
            thread_replies: true,
            keywords: "deploy, incident".to_owned(),
            sound: 0,
            show_preview: true,
            mute_sounds: false,
            schedule_enabled: true,
            schedule_days: 1,
            schedule_start: 9,
            schedule_end: 18,
            show_home: true,
            show_dms: true,
            show_activity: true,
            show_files: false,
            show_later: true,
            tab_style: 0,
            show: 0,
            sort: 0,
            unreads_section: false,
            separate_private: false,
            profile_photos: true,
            mode: AppearanceMode::Dark,
            palette: Palette::Default,
            font: theme::DEFAULT_FONT,
            skin_tone: 0,
            jumbomoji: true,
            convert_emoticons: true,
            density: 0,
            names: 0,
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
            mark_read: 0,
            confirm_mark_all: true,
            confirm_unsend: true,
            warn_external: true,
            launch_at_login: false,
            hardware_acceleration: true,
            debug_logging: false,
            open_links: 0,
        }
    }
}

impl Settings {
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
            let subscriptions = vec![cx.subscribe_in(
                &keywords,
                window,
                |this: &mut Preferences, keywords, event: &InputEvent, _, cx| {
                    if let InputEvent::Change = event {
                        this.settings.keywords = keywords.read(cx).value().to_string();
                        cx.emit(PreferencesEvent::Changed);
                    }
                },
            )];
            Preferences {
                section: Section::Notifications,
                settings,
                open_select: None,
                keywords,
                _subscriptions: subscriptions,
            }
        })
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Jump to `section` (the DND menu opens Notifications, the theme toggle Appearance).
    pub fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        self.open_select = None;
        cx.notify();
    }

    /// Mirror a theme switch made outside the window (the title-bar toggle).
    pub fn set_appearance_mode(&mut self, mode: AppearanceMode, cx: &mut Context<Self>) {
        if self.settings.mode != mode {
            self.settings.mode = mode;
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
        let level = self.radios("notify-level", &NOTIFY_LEVELS, |s| &mut s.notify_level, cx);
        let mobile = self.checkbox(
            "notify-mobile",
            "Use different settings for my mobile devices",
            |s| &mut s.mobile_different,
            cx,
        );
        let threads = self.checkbox(
            "notify-threads",
            "Notify me about replies to threads I'm following",
            |s| &mut s.thread_replies,
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
        let start = self.select("schedule-start", &HOURS, |s| &mut s.schedule_start, cx);
        let end = self.select("schedule-end", &HOURS, |s| &mut s.schedule_end, cx);
        let sound = self.select("notify-sound", &SOUNDS, |s| &mut s.sound, cx);
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
        v_flex()
            .child(self.group("Notify me about\u{2026}", None, v_flex().gap_2().child(level).child(mobile).child(threads), cx))
            .child(self.group(
                "My keywords",
                Some("You'll be notified whenever someone says one of your keywords."),
                keywords,
                cx,
            ))
            .child(self.group(
                "Notification schedule",
                Some("You'll only receive notifications in the hours you choose. Outside of those times, notifications will be paused."),
                v_flex()
                    .gap_3()
                    .child(schedule_on)
                    .child(self.labeled("Allow notifications", days))
                    .child(h_flex().gap_4().items_start().child(self.labeled("From", start)).child(self.labeled("To", end))),
                cx,
            ))
            .child(self.group(
                "Sound & appearance",
                None,
                v_flex().gap_3().child(self.labeled("Notification sound (messages)", sound)).child(preview).child(mute),
                cx,
            ))
    }

    fn navigation(&mut self, cx: &mut Context<Self>) -> Div {
        let home = self.checkbox("nav-home", "Home", |s| &mut s.show_home, cx);
        let dms = self.checkbox("nav-dms", "DMs", |s| &mut s.show_dms, cx);
        let activity = self.checkbox("nav-activity", "Activity", |s| &mut s.show_activity, cx);
        let files = self.checkbox("nav-files", "Files", |s| &mut s.show_files, cx);
        let later = self.checkbox("nav-later", "Later", |s| &mut s.show_later, cx);
        let style = self.radios("nav-style", &TAB_STYLES, |s| &mut s.tab_style, cx);
        v_flex()
            .child(
                self.group(
                    "Show these tabs",
                    Some("Choose which tabs appear in the navigation bar on the left."),
                    v_flex()
                        .gap_1()
                        .child(home)
                        .child(dms)
                        .child(activity)
                        .child(files)
                        .child(later),
                    cx,
                ),
            )
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
            .child(self.group(
                "Organization",
                None,
                v_flex().gap_1().child(unreads).child(private).child(photos),
                cx,
            ))
    }

    fn appearance(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (border, ring, sidebar) = (theme.border, theme.ring, theme.sidebar);
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
                    let (side, accent) = choice.swatch(sidebar);
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
        let custom = v_flex()
            .gap_3()
            .child(self.labeled(
                "Sidebar",
                swatch_row("custom-sidebar", &CUSTOM_SIDEBARS, custom_sidebar, cx),
            ))
            .child(self.labeled(
                "Accent",
                swatch_row("custom-accent", &CUSTOM_ACCENTS, custom_accent, cx),
            ));

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
            .child(self.group("Slack themes", Some("Single-color themes that tint your sidebar and highlights."), named, cx))
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
            .child(self.group("Names", None, names, cx))
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
        v_flex()
            .child(self.group("When I view a channel", None, mode, cx))
            .child(self.group("Mark everything read", None, confirm, cx))
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
        v_flex()
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
                v_flex().gap_1().child(login).child(hardware).child(debug),
                cx,
            ))
    }
}

impl Render for Preferences {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.section {
            Section::Notifications => self.notifications(cx),
            Section::Navigation => self.navigation(cx),
            Section::Home => self.home(cx),
            Section::Appearance => self.appearance(cx),
            Section::Messages => self.messages(cx),
            Section::Language => self.language(cx),
            Section::Accessibility => self.accessibility(cx),
            Section::MarkAsRead => self.mark_as_read(cx),
            Section::Advanced => self.advanced(cx),
        };
        let nav = self.nav(cx);
        let title = self.section.label();
        h_flex()
            .h(px(560.))
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
