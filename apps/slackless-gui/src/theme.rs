//! Slack-style theming (docs/research/ui-reference.md "Theming"): two named palettes —
//! Slack Dark (startup, matches the reference screenshot) and Slack Light — installed on
//! top of the gpui-kit theme system (gpui-kit-guide section 11), optionally tinted by one of
//! Slack's named color themes or a custom sidebar/accent pair ([`Palette`]).
//!
//! Every view reads colors exclusively through `cx.theme()` tokens; no view carries
//! hard-coded colors. The deliberate exceptions live here too: the avatar identity chips keep
//! Slack's fixed brand hues in every theme ([`avatar_color`]), because they identify people,
//! not surfaces, and the theme picker previews each palette's own colors ([`Palette::swatch`],
//! [`swatch_color`]).

use std::borrow::Cow;

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};
use gpui_kit::gpui::{
    App, Background, Global, Hsla, Pixels, SharedString, hsla, linear_color_stop, linear_gradient,
    px, rgb,
};

/// Slack's Font choices slackless can show (Preferences > Appearance > Font): Lato, Slack's
/// default, embedded as a Latin subset (`fonts/OFL.txt`), and the families macOS ships.
/// Slack's own web fonts (Atkinson Hyperlegible Next, Noto Sans, OpenDyslexic, Roboto Mono)
/// are not bundled, keeping the binary small.
pub const FONT_LABELS: [&str; 5] = [
    "Arial",
    "Comic Sans",
    "Georgia",
    "Lato (Default)",
    "San Francisco Pro (System)",
];

/// The font family behind each of [`FONT_LABELS`].
pub const FONT_FAMILIES: [&str; 5] = ["Arial", "Comic Sans MS", "Georgia", "Lato", ".SystemUIFont"];

/// Lato, Slack's default font.
pub const DEFAULT_FONT: usize = 3;

/// Inline code's face: Monaco (Slack's code font) with its em grown so its glyphs come out at
/// 6/7 of the body size, 12px in the 14px body: the size Slack's and our pre blocks use.
pub const INLINE_CODE_FAMILY: &str = "Slackless Inline Code";

const MONACO: &str = "/System/Library/Fonts/Monaco.ttf";

/// Whether [`INLINE_CODE_FAMILY`] loaded (it is built from the system's Monaco).
struct InlineCode(bool);

impl Global for InlineCode {}

/// The family inline code renders in: the smaller face when it loaded, else the theme's mono.
pub fn inline_code_family(cx: &App) -> SharedString {
    if cx.try_global::<InlineCode>().is_some_and(|loaded| loaded.0) {
        INLINE_CODE_FAMILY.into()
    } else {
        cx.theme().mono_font_family.clone()
    }
}

/// The Font in effect, re-applied whenever a theme change reloads the kit's own.
struct UiFont(usize);

impl Global for UiFont {}

/// Focus mode is on: the theme is drained to grayscale, badges included.
struct Focus(bool);

impl Global for Focus {}

/// Active window chrome modifiers installed alongside the color theme.
#[derive(Clone, Copy)]
struct ThemeChrome {
    palette: Palette,
    darker_sidebars: bool,
    window_gradient: bool,
}

impl Default for ThemeChrome {
    fn default() -> Self {
        Self {
            palette: Palette::Default,
            darker_sidebars: false,
            window_gradient: true,
        }
    }
}

impl Global for ThemeChrome {}

fn focused(cx: &App) -> bool {
    cx.try_global::<Focus>().is_some_and(|focus| focus.0)
}

fn chrome_state(cx: &App) -> ThemeChrome {
    cx.try_global::<ThemeChrome>().copied().unwrap_or_default()
}

/// Whether the sidebar and window chrome currently have a dark surface (either Dark mode,
/// any tinted named/custom theme, or Darker sidebars in Light mode).
pub fn sidebar_is_dark(cx: &App) -> bool {
    let chrome = chrome_state(cx);
    cx.theme().mode.is_dark() || chrome.palette != Palette::Default || chrome.darker_sidebars
}

/// The palettes slackless ships.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SlackTheme {
    Dark,
    Light,
}

impl SlackTheme {
    fn mode(self) -> ThemeMode {
        match self {
            SlackTheme::Dark => ThemeMode::Dark,
            SlackTheme::Light => ThemeMode::Light,
        }
    }
}

/// Embed Lato and install the startup theme: Slack Dark, per the reference screenshot.
pub fn init(cx: &mut App) {
    let lato = [
        include_bytes!("../fonts/Lato-Regular.ttf").as_slice(),
        include_bytes!("../fonts/Lato-Italic.ttf"),
        include_bytes!("../fonts/Lato-Bold.ttf"),
        include_bytes!("../fonts/Lato-BoldItalic.ttf"),
    ];
    if let Err(e) = cx.text_system().add_fonts(lato.map(Cow::Borrowed).to_vec()) {
        tracing::warn!("could not load Lato ({e})");
    }
    let inline_code = std::fs::read(MONACO)
        .ok()
        .and_then(|monaco| crate::code_font::scaled(&monaco, INLINE_CODE_FAMILY, 6. / 7.));
    let loaded = inline_code.is_some_and(|font| {
        cx.text_system()
            .add_fonts(vec![Cow::Owned(font)])
            .inspect_err(|e| tracing::warn!("could not load the inline code face ({e})"))
            .is_ok()
    });
    cx.set_global(InlineCode(loaded));
    activate(SlackTheme::Dark, cx);
}

/// The Font in effect, an index into [`FONT_LABELS`].
pub fn font(cx: &App) -> usize {
    cx.try_global::<UiFont>()
        .map_or(DEFAULT_FONT, |font| font.0)
}

/// Resolve a font index safely into [`FONT_FAMILIES`], falling back to [`DEFAULT_FONT`].
pub fn font_family_for(index: usize) -> &'static str {
    FONT_FAMILIES
        .get(index)
        .copied()
        .unwrap_or(FONT_FAMILIES[DEFAULT_FONT])
}

/// Switch the UI font to [`FONT_LABELS`]`[font]` (out-of-range falls back to [`DEFAULT_FONT`]);
/// a no-op when it is already in effect.
pub fn set_font(font: usize, cx: &mut App) {
    let font = if font < FONT_FAMILIES.len() {
        font
    } else {
        DEFAULT_FONT
    };
    if self::font(cx) == font {
        return;
    }
    cx.set_global(UiFont(font));
    Theme::update(cx, |theme| theme.font_family = font_family_for(font).into());
}

/// Switch the whole app to `theme` with the default color theme and repaint every window.
pub fn activate(theme: SlackTheme, cx: &mut App) {
    activate_with(theme, Palette::Default, false, true, false, cx);
}

/// Switch the whole app to `theme` tinted by the color theme `palette`, honoring
/// `darker_sidebars`, `window_gradient`, and focus mode's grayscale when `focus` is set.
///
/// `Theme::change` loads the registered mode palette; the Slack palette (and the color theme
/// over it) is applied in a second pass, because a mode change reloads that mode's theme over
/// same-closure edits.
pub fn activate_with(
    theme: SlackTheme,
    palette: Palette,
    darker_sidebars: bool,
    window_gradient: bool,
    focus: bool,
    cx: &mut App,
) {
    let mode = theme.mode();
    let family = font_family_for(font(cx));
    cx.set_global(Focus(focus));
    cx.set_global(ThemeChrome {
        palette,
        darker_sidebars,
        window_gradient,
    });
    Theme::change(mode, None, cx);
    Theme::update(cx, |theme| {
        apply(mode.is_dark(), theme);
        tint(palette, mode.is_dark(), darker_sidebars, theme);
        if focus {
            grayscale(theme);
        }
        theme.font_family = family.into();
    });
}

/// Slack's focus mode turns the theme grayscale "so badges no longer appear in red (or your
/// theme color)": the chrome, sidebar and selection lose their hue.
fn grayscale(theme: &mut Theme) {
    theme.title_bar = theme.title_bar.grayscale();
    theme.sidebar = theme.sidebar.grayscale();
    theme.sidebar_border = theme.sidebar_border.grayscale();
    theme.sidebar_accent = theme.sidebar_accent.grayscale();
    theme.sidebar_primary = theme.sidebar_primary.grayscale();
    theme.sidebar_primary_foreground = theme.sidebar_primary_foreground.grayscale();
    theme.ring = theme.ring.grayscale();
}

/// Slack's named color themes (Preferences > Appearance & Themes > Color themes), plus a
/// custom theme built from a sidebar and an accent swatch. `Default` keeps the plain Slack
/// Light / Dark palette. Hues approximate Slack's.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Palette {
    Default,
    Aubergine,
    Clementine,
    Banana,
    Jade,
    Lagoon,
    Barbra,
    MoodIndigo,
    /// Indexes into [`CUSTOM_SIDEBARS`] and [`CUSTOM_ACCENTS`].
    Custom {
        sidebar: usize,
        accent: usize,
    },
    /// Arbitrary `#RRGGBB` sidebar & accent colors (`F-SHELL-077`).
    CustomHex {
        sidebar: u32,
        accent: u32,
    },
}

impl Palette {
    /// The named themes in picker order (the custom theme is offered separately).
    pub const NAMED: [Palette; 8] = [
        Palette::Default,
        Palette::Aubergine,
        Palette::Clementine,
        Palette::Banana,
        Palette::Jade,
        Palette::Lagoon,
        Palette::Barbra,
        Palette::MoodIndigo,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Palette::Default => "Slack",
            Palette::Aubergine => "Aubergine",
            Palette::Clementine => "Clementine",
            Palette::Banana => "Banana",
            Palette::Jade => "Jade",
            Palette::Lagoon => "Lagoon",
            Palette::Barbra => "Barbra",
            Palette::MoodIndigo => "Mood Indigo",
            Palette::Custom { .. } | Palette::CustomHex { .. } => "Custom",
        }
    }

    /// `(sidebar, accent)` hex colors; `None` for the untinted default.
    fn colors(self) -> Option<(u32, u32)> {
        match self {
            Palette::Default => None,
            Palette::Aubergine => Some((0x3F_0E_40, 0x11_64_A3)),
            Palette::Clementine => Some((0x7A_2E_12, 0xC7_51_14)),
            Palette::Banana => Some((0x5C_4A_12, 0xD9_A5_19)),
            Palette::Jade => Some((0x0B_3B_2E, 0x1B_7A_56)),
            Palette::Lagoon => Some((0x0F_3D_5C, 0x1D_78_B4)),
            Palette::Barbra => Some((0x5A_18_40, 0xC9_18_5B)),
            Palette::MoodIndigo => Some((0x1F_2A_4D, 0x4E_5F_E6)),
            Palette::Custom { sidebar, accent } => Some((
                CUSTOM_SIDEBARS[sidebar % CUSTOM_SIDEBARS.len()],
                CUSTOM_ACCENTS[accent % CUSTOM_ACCENTS.len()],
            )),
            Palette::CustomHex { sidebar, accent } => Some((sidebar, accent)),
        }
    }

    /// Slack's `sidebar_theme` pref value: the theme names Slack's own picker stores, without
    /// the `_theme` suffix. Themes slackless does not ship a palette for keep the default look.
    fn from_slack(name: &str) -> Self {
        match name.strip_suffix("_theme").unwrap_or(name) {
            "aubergine" => Palette::Aubergine,
            "clementine" => Palette::Clementine,
            "banana" => Palette::Banana,
            "jade" => Palette::Jade,
            "lagoon" | "ocean" => Palette::Lagoon,
            "barbra" => Palette::Barbra,
            "mood_indigo" => Palette::MoodIndigo,
            _ => Palette::Default,
        }
    }

    /// Slack's `sidebar_theme` wire name for `users.prefs.set`.
    pub fn slack_pref_name(self) -> Option<&'static str> {
        match self {
            Palette::Default => Some("default"),
            Palette::Aubergine => Some("aubergine"),
            Palette::Clementine => Some("clementine"),
            Palette::Banana => Some("banana"),
            Palette::Jade => Some("jade"),
            Palette::Lagoon => Some("lagoon"),
            Palette::Barbra => Some("barbra"),
            Palette::MoodIndigo => Some("mood_indigo"),
            Palette::Custom { .. } | Palette::CustomHex { .. } => None,
        }
    }

    /// Return the active `(sidebar, accent)` 24-bit RGB values if tinted.
    pub fn rgb_pair(self) -> Option<(u32, u32)> {
        self.colors()
    }

    /// The swatch shown for this theme in the picker: its sidebar and accent colors, or
    /// Slack's untinted Light/Dark default colors for [`Palette::Default`].
    pub fn swatch(self, dark: bool) -> (Hsla, Hsla) {
        match self.colors() {
            Some((sidebar, accent)) => (color(sidebar), color(accent)),
            None if dark => (color(0x13_18_1D), color(0x05_66_96)),
            None => (color(0xEB_F0_F5), color(0x11_64_A3)),
        }
    }
}

/// The color theme Slack stores for the signed-in user (`sidebar_theme`), so slackless opens
/// tinted like Slack itself; `None` when the pref is absent. Light, dark or system is not a
/// workspace pref: Slack keeps its Color Mode on the client, and so does slackless.
pub fn palette_from_prefs(prefs: &serde_json::Value) -> Option<Palette> {
    prefs
        .get("sidebar_theme")
        .and_then(|value| value.as_str())
        .map(Palette::from_slack)
}

/// Sidebar swatches offered by the custom theme builder.
pub const CUSTOM_SIDEBARS: [u32; 6] = [
    0x3F_0E_40, 0x19_17_1D, 0x0B_3B_2E, 0x0F_3D_5C, 0x5A_18_40, 0x3A_3A_3A,
];
/// Accent swatches offered by the custom theme builder.
pub const CUSTOM_ACCENTS: [u32; 6] = [
    0x11_64_A3, 0x2B_AC_76, 0xE0_1E_5A, 0xEC_B2_2E, 0x7C_5C_FF, 0xE8_71_2D,
];

/// Parse a `#RRGGBB` or `RRGGBB` hex color string into a 24-bit RGB integer (`F-SHELL-077`).
pub fn parse_hex_rgb(input: &str) -> Option<u32> {
    let trimmed = input.trim().trim_start_matches('#');
    if trimmed.len() != 6 || !trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(trimmed, 16).ok()
}

/// Format a 24-bit RGB integer as `#RRGGBB`.
pub fn format_hex_rgb(rgb: u32) -> String {
    format!("#{:06X}", rgb & 0x00FF_FFFF)
}

/// Parse a Slack shareable / legacy comma-separated hex theme string (`F-SHELL-078`, `F-SHELL-079`)
/// into `(sidebar, accent)` RGB values. Accepts Slack's classic 8- or 10-color format
/// (`column_bg,menu_bg,active_item,active_item_text,hover_item,text_color,active_presence,mention_badge[,top_nav_bg,top_nav_text]`)
/// as well as a shorthand 2-color `sidebar,accent` string.
pub fn parse_slack_theme_string(input: &str) -> Option<(u32, u32)> {
    let colors: Vec<u32> = input
        .split([',', ' ', '\n', '\t'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(parse_hex_rgb)
        .collect::<Option<Vec<_>>>()?;
    match colors.as_slice() {
        [sidebar, accent] => Some((*sidebar, *accent)),
        [sidebar, _, accent, ..] => Some((*sidebar, *accent)),
        _ => None,
    }
}

/// Export `palette` as Slack's shareable 8-color comma-separated hex theme string (`F-SHELL-078`).
pub fn format_slack_theme_string(palette: Palette, dark: bool) -> String {
    let (sidebar, accent) = palette.rgb_pair().unwrap_or(if dark {
        (0x13_18_1D, 0x05_66_96)
    } else {
        (0x3F_0E_40, 0x11_64_A3)
    });
    let sb = format_hex_rgb(sidebar);
    let ac = format_hex_rgb(accent);
    format!("{sb},{sb},{ac},#FFFFFF,{sb},#FFFFFF,#2BAC76,#CD2553")
}

/// A custom-builder swatch as a theme color.
pub fn swatch_color(hex: u32) -> Hsla {
    color(hex)
}

/// Pick a legible foreground (`#1D1C1D` vs `#FFFFFF`) for a hex accent fill using relative
/// sRGB luminance so bright accents like Banana stay high-contrast everywhere.
fn contrasting_foreground(hex: u32) -> Hsla {
    let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
    let b = (hex & 0xFF) as f32 / 255.0;
    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    if luma > 0.58 {
        color(0x1D_1C_1D)
    } else {
        color(0xFF_FF_FF)
    }
}

/// Lay a color theme (and optional `darker_sidebars`) over the base palette: the top chrome,
/// rail, and sidebar all share the theme's hue with crisp contrast in both Light and Dark mode.
fn tint(palette: Palette, dark: bool, darker_sidebars: bool, theme: &mut Theme) {
    let Some((sidebar_hex, accent_hex)) = palette.colors() else {
        if darker_sidebars {
            if dark {
                theme.sidebar = color(0x0E_11_15);
                theme.title_bar = color(0x05_0F_18);
                theme.sidebar_border = color(0x1E_22_28);
            } else {
                theme.sidebar = color(0x1E_23_29);
                theme.title_bar = color(0x14_18_1E);
                theme.title_bar_border = color(0x0D_10_14);
                theme.sidebar_border = color(0x2F_36_3F);
                theme.sidebar_foreground = color(0xF2_F2_F2);
                theme.sidebar_primary = color(0xFF_FF_FF);
                theme.sidebar_primary_foreground = color(0x1E_23_29);
            }
        }
        return;
    };
    let base = color(sidebar_hex);
    let accent = color(accent_hex);
    let accent_fg = contrasting_foreground(accent_hex);
    if dark {
        // Dark mode blends the theme hue into dark charcoal so it stays cohesive with the
        // message canvas without washing out contrast, matching official Slack Dark themes.
        let mix = if darker_sidebars { 0.68 } else { 0.48 };
        theme.sidebar = base.blend(color(0x13_15_19).opacity(mix));
        theme.title_bar = base.blend(color(0x0B_0D_10).opacity((mix + 0.14).min(0.84)));
        theme.title_bar_border = theme.title_bar.blend(color(0x00_00_00).opacity(0.35));
        theme.sidebar_foreground = color(0xEB_EC_EE);
        theme.sidebar_border = theme.sidebar.blend(color(0xFF_FF_FF).opacity(0.12));
    } else {
        // Light mode shows the full signature Slack sidebar color with a slightly deeper chrome
        // band behind the title bar and workspace rail.
        theme.sidebar = if darker_sidebars {
            base.blend(color(0x0B_0D_10).opacity(0.32))
        } else {
            base
        };
        theme.title_bar = theme.sidebar.blend(color(0x00_00_00).opacity(0.22));
        theme.title_bar_border = theme.title_bar.blend(color(0x00_00_00).opacity(0.25));
        theme.sidebar_foreground = color(0xF2_F2_F2);
        theme.sidebar_border = theme.sidebar.blend(color(0xFF_FF_FF).opacity(0.15));
    }
    theme.sidebar_accent = accent;
    theme.sidebar_accent_foreground = accent_fg;
    theme.sidebar_primary = color(0xFF_FF_FF);
    theme.sidebar_primary_foreground = theme.sidebar;
    theme.ring = accent;
}

fn color(hex: u32) -> Hsla {
    Hsla::from(rgb(hex))
}

/// Slack's inline-code text color: orange on dark themes, red on light ones.
pub fn code_foreground(cx: &App) -> Hsla {
    if cx.theme().mode.is_dark() {
        color(0xE8_91_2D)
    } else {
        color(0xE0_1E_5A)
    }
}

/// Recessed background for triple-backtick code blocks inside the composer: clearly distinct
/// from [`composer_surfaces`] in both dark and light mode.
pub fn code_block_background(cx: &App) -> Hsla {
    if cx.theme().mode.is_dark() {
        color(0x1A_1D_21)
    } else {
        color(0xF4_F4_F5)
    }
}

/// Slack's blue `"New"` badge pill shown in menus (e.g. `+` -> `File, canvas, or list`) as
/// `(background, foreground)`.
pub fn new_badge(cx: &App) -> (Hsla, Hsla) {
    if cx.theme().mode.is_dark() {
        (color(0x10_23_4B), color(0x9D_C3_FF))
    } else {
        (color(0xE3_EE_FF), color(0x12_64_A3))
    }
}

/// Slack's destructive text ("Delete message..."): the danger red on light themes, a light
/// rose on dark ones.
pub fn danger_text(cx: &App) -> Hsla {
    let theme = cx.theme();
    if theme.mode.is_dark() {
        color(0xE6_9B_A1)
    } else {
        theme.danger
    }
}

/// Slack's mention badge (mention counts, the "Unread mentions" chip) as (background,
/// foreground): the danger red with white text on light themes, a light rose with dark text
/// on dark ones (measured on Slack 4.53).
pub fn mention_badge(cx: &App) -> (Hsla, Hsla) {
    let theme = cx.theme();
    let (bg, fg) = if theme.mode.is_dark() {
        (color(0xE6_9B_A1), color(0x1A_1D_21))
    } else {
        (theme.danger, theme.danger_foreground)
    };
    if focused(cx) {
        (bg.grayscale(), fg)
    } else {
        (bg, fg)
    }
}

/// Slack's tab rail fill (`theme-surf-inv-sec`): for the default theme, Slack 4.53's measured
/// near-black (#0E171B) in dark mode or 90% white in light mode; for tinted themes and Darker
/// sidebars, a slightly deeper shade of the sidebar so the rail always matches the theme hue.
pub fn tab_rail_wash(cx: &App) -> Hsla {
    let chrome = chrome_state(cx);
    if chrome.palette == Palette::Default && !chrome.darker_sidebars {
        if cx.theme().mode.is_dark() {
            color(0x0E_17_1B)
        } else {
            color(0xFF_FF_FF).opacity(0.9)
        }
    } else {
        cx.theme().sidebar.blend(color(0x00_00_00).opacity(0.20))
    }
}

/// The rail's fills over that wash as the sidebar foreground's opacity: darker/tinted rails
/// take a stronger translucent tile fill than the light white rail.
pub fn rail_fill(cx: &App) -> f32 {
    if sidebar_is_dark(cx) { 0.25 } else { 0.13 }
}

/// Where a light-mode Slack panel's shadow (`box-shadow: 0 0 20px rgba(0,0,0,.1)`) falls on the
/// panel to its left: a strip darkening toward its right edge. GPUI's resizable panels keep
/// each panel's own shadow inside it, so the neighbor paints the falloff itself.
pub fn panel_shadow_edge() -> Background {
    let shade = color(0x00_00_00).opacity(0.05);
    linear_gradient(
        90.,
        linear_color_stop(shade.opacity(0.), 0.),
        linear_color_stop(shade, 1.),
    )
}

/// The composer box's surface and its formatting band (Slack's `.c-wysiwyg_container` and
/// `__formatting`): one raised surface on dark themes, white under a light gray band on light.
pub fn composer_surfaces(cx: &App) -> (Hsla, Hsla) {
    if cx.theme().mode.is_dark() {
        (color(0x22_24_29), color(0x22_24_29))
    } else {
        (color(0xFF_FF_FF), color(0xF8_F8_F8))
    }
}

/// Slack's Slack Connect context bar over the composer: an amber wash over the message pane
/// (#fff2df on white); on dark themes the deep amber Slack also tints an open thread's root
/// with (#2b1f04, measured on Slack 4.53).
pub fn connect_bar(cx: &App) -> Hsla {
    if cx.theme().mode.is_dark() {
        color(0x2B_1F_04)
    } else {
        cx.theme().background.blend(color(0xFF_A5_00).opacity(0.13))
    }
}

/// Slack's tint on the channel message whose thread is open beside it: a warm wash in
/// light (#fffae0 on white), the Slack Connect bar's amber in dark.
pub fn thread_root(cx: &App) -> Hsla {
    if cx.theme().mode.is_dark() {
        connect_bar(cx)
    } else {
        cx.theme().background.blend(color(0xFF_DE_30).opacity(0.15))
    }
}

/// Slack's tint on a message row being edited in place: warm olive in dark (`#433F2A`,
/// measured on Slack 4.53), warm yellow wash in light.
pub fn editing_row(cx: &App) -> Hsla {
    if cx.theme().mode.is_dark() {
        color(0x43_3F_2A)
    } else {
        cx.theme().background.blend(color(0xFF_DE_30).opacity(0.15))
    }
}

/// Slack's reaction pill that includes you (`.c-reaction--reacted`) as (background, count
/// color, ring): filled blue under light text on dark themes; a blue wash with blue text and a
/// blue ring on light ones.
pub fn reacted_pill(cx: &App) -> (Hsla, Hsla, Option<Hsla>) {
    let theme = cx.theme();
    if theme.mode.is_dark() {
        (color(0x0C_77_A7), theme.foreground, None)
    } else {
        (theme.link.opacity(0.1), theme.link, Some(color(0x1D_9B_D1)))
    }
}

/// The glow Slack's window chrome carries in its top-left corner, over the flat chrome tint
/// (`title_bar`), as measured on Slack 4.53: the side of the square it is painted on and
/// the square's fill. The fill fades the glow to transparent along the diagonal, reaching it
/// where `x + y` equals the side; fading the alpha with the color gives Slack's quick
/// falloff near the corner. A square keeps GPUI's aspect-scaled gradient math exact.
pub fn chrome_glow(cx: &App) -> (Pixels, Background) {
    let theme = cx.theme();
    let chrome = chrome_state(cx);
    if !chrome.window_gradient {
        let clear = theme.title_bar.opacity(0.);
        return (
            px(0.),
            linear_gradient(
                135.,
                linear_color_stop(clear, 0.),
                linear_color_stop(clear, 1.),
            ),
        );
    }
    let (side, glow) = if chrome.palette == Palette::Default && !chrome.darker_sidebars {
        if theme.mode.is_dark() {
            (900., color(0x33_3B_40))
        } else {
            (1400., color(0xF5_FA_FB))
        }
    } else {
        // Tinted themes derive their corner highlight from the chrome itself so the glow
        // stays harmoniously matched to Aubergine, Jade, Lagoon, Clementine, etc.
        let lift = if theme.mode.is_dark() { 0.16 } else { 0.22 };
        (
            1000.,
            theme.title_bar.blend(color(0xFF_FF_FF).opacity(lift)),
        )
    };
    let glow = if focused(cx) { glow.grayscale() } else { glow };
    // On a square, a 135deg gradient puts the corner at t = 0.5 - 1/sqrt(2) and the
    // anti-diagonal through (side, 0) at t = 0.5.
    let fill = linear_gradient(
        135.,
        linear_color_stop(glow, 0.5 - std::f32::consts::FRAC_1_SQRT_2),
        linear_color_stop(theme.title_bar.opacity(0.), 0.5),
    );
    (px(side), fill)
}

/// Fill the global theme's color tokens with the named Slack palette.
///
/// Editing the fields the app and views actually consume is enough: untouched kit tokens
/// keep the mode defaults and stay consistent with the two palettes' lightness.
fn apply(dark: bool, theme: &mut Theme) {
    if dark {
        // Slack Dark (approximates the reference screenshot).
        theme.background = color(0x1A_1D_21);
        theme.foreground = color(0xD1_D2_D3);
        theme.muted = color(0x25_28_2D);
        theme.muted_foreground = color(0xAB_AB_AD);
        theme.border = color(0x30_36_3D);
        theme.input = color(0x3B_41_4B);
        theme.accent = color(0x24_28_2E); // subtle hover wash
        theme.accent_foreground = color(0xFF_FF_FF);
        theme.secondary = color(0x2A_2E_34);
        theme.secondary_foreground = color(0xD1_D2_D3);
        theme.secondary_hover = color(0x33_37_3E);
        theme.secondary_active = color(0x3B_3F_46);
        theme.link = color(0x1D_9B_D1);
        theme.link_hover = color(0x36_A8_E0);
        theme.link_active = color(0x1D_9B_D1);
        theme.selection = color(0x1D_9B_D1).opacity(0.35);
        theme.primary = color(0x00_7A_5A); // Slack green (send button)
        theme.primary_foreground = color(0xFF_FF_FF);
        theme.primary_hover = color(0x14_85_67);
        theme.primary_active = color(0x00_61_4B);
        theme.danger = color(0xE0_1E_5A); // New divider + mention red
        theme.danger_foreground = color(0xFF_FF_FF);
        theme.danger_hover = color(0xE8_4A_79);
        theme.danger_active = color(0xB9_1A_4C);
        // Destructive buttons ("Delete message") are Slack's solid red, not the kit's tint.
        theme.button_danger = theme.danger;
        theme.button_danger_foreground = theme.danger_foreground;
        theme.button_danger_hover = theme.danger_hover;
        theme.button_danger_active = theme.danger_active;
        // Primary buttons ("Save", "Send") are Slack's green, not the kit's foreground fill.
        theme.button_primary = theme.primary;
        theme.button_primary_foreground = theme.primary_foreground;
        theme.button_primary_hover = theme.primary_hover;
        theme.button_primary_active = theme.primary_active;
        // Slack dims the window behind a modal with a light black wash.
        theme.overlay = color(0x00_00_00).opacity(0.2);
        theme.success = color(0x2B_AC_76); // presence green
        theme.success_foreground = color(0xFF_FF_FF);
        theme.warning = color(0xEC_B2_2E);
        theme.warning_foreground = color(0x1A_1D_21);
        // The sidebar card sits a shade darker than the message card, on navy chrome.
        theme.sidebar = color(0x13_18_1D);
        theme.sidebar_border = color(0x24_28_2C);
        theme.sidebar_foreground = color(0xD1_D2_D3);
        theme.sidebar_primary = color(0xD1_D2_D3); // unread pill: near-white bg
        theme.sidebar_primary_foreground = color(0x1A_1D_21);
        theme.sidebar_accent = color(0x05_66_96); // selected row blue
        theme.sidebar_accent_foreground = color(0xFF_FF_FF);
        theme.title_bar = color(0x07_1D_2C);
        theme.title_bar_border = color(0x101216);
        theme.popover = color(0x25_28_2D);
        theme.popover_foreground = color(0xD1_D2_D3);
        theme.ring = color(0x1D_9B_D1);
        theme.caret = color(0xD1_D2_D3);
        theme.scrollbar = color(0x1A_1D_21);
        theme.scrollbar_thumb = color(0x43_47_4D);
        theme.scrollbar_thumb_hover = color(0x56_58_5C);
        theme.tab = color(0x1A_1D_21);
        theme.tab_active = color(0x1A_1D_21);
        theme.tab_bar = color(0x1A_1D_21);
        theme.tab_foreground = color(0xAB_AB_AD);
        theme.tab_active_foreground = color(0xFF_FF_FF);
    } else {
        // Slack Light.
        theme.background = color(0xFF_FF_FF);
        theme.foreground = color(0x1D_1C_1D);
        theme.muted = color(0xF6_F6_F6);
        theme.muted_foreground = color(0x61_60_61);
        theme.border = color(0xE1_E3_E5);
        theme.input = color(0xB9_BC_C0);
        theme.accent = color(0xF2_F3_F5);
        theme.accent_foreground = color(0x1D_1C_1D);
        theme.secondary = color(0xF2_F3_F5);
        theme.secondary_foreground = color(0x1D_1C_1D);
        theme.secondary_hover = color(0xE8_EA_EC);
        theme.secondary_active = color(0xE0_E2_E4);
        theme.link = color(0x12_64_A3);
        theme.link_hover = color(0x0E_51_84);
        theme.link_active = color(0x12_64_A3);
        theme.selection = color(0x12_64_A3).opacity(0.25);
        theme.primary = color(0x00_7A_5A);
        theme.primary_foreground = color(0xFF_FF_FF);
        theme.primary_hover = color(0x14_85_67);
        theme.primary_active = color(0x00_61_4B);
        theme.danger = color(0xE0_1E_5A);
        theme.danger_foreground = color(0xFF_FF_FF);
        theme.danger_hover = color(0xE8_4A_79);
        theme.danger_active = color(0xB9_1A_4C);
        theme.button_danger = theme.danger;
        theme.button_danger_foreground = theme.danger_foreground;
        theme.button_danger_hover = theme.danger_hover;
        theme.button_danger_active = theme.danger_active;
        // Primary buttons ("Save", "Send") are Slack's green, not the kit's foreground fill.
        theme.button_primary = theme.primary;
        theme.button_primary_foreground = theme.primary_foreground;
        theme.button_primary_hover = theme.primary_hover;
        theme.button_primary_active = theme.primary_active;
        theme.overlay = color(0x00_00_00).opacity(0.2);
        theme.success = color(0x2B_AC_76);
        theme.success_foreground = color(0xFF_FF_FF);
        theme.warning = color(0xEC_B2_2E);
        theme.warning_foreground = color(0x1D_1C_1D);
        theme.sidebar = color(0xFF_FF_FF);
        theme.sidebar_border = color(0xE1_E3_E5);
        theme.sidebar_foreground = color(0x1D_1C_1D);
        theme.sidebar_primary = color(0x11_64_A3); // unread pill: blue bg
        theme.sidebar_primary_foreground = color(0xFF_FF_FF);
        theme.sidebar_accent = color(0x11_64_A3);
        theme.sidebar_accent_foreground = color(0xFF_FF_FF);
        theme.title_bar = color(0xB1_EB_FC);
        theme.title_bar_border = color(0xE1_E3_E5);
        theme.popover = color(0xFF_FF_FF);
        theme.popover_foreground = color(0x1D_1C_1D);
        theme.ring = color(0x12_64_A3);
        theme.caret = color(0x1D_1C_1D);
        theme.scrollbar = color(0xFF_FF_FF);
        theme.scrollbar_thumb = color(0xC1_C3_C6);
        theme.scrollbar_thumb_hover = color(0xA5_A7_AA);
        theme.tab = color(0xFF_FF_FF);
        theme.tab_active = color(0xFF_FF_FF);
        theme.tab_bar = color(0xFF_FF_FF);
        theme.tab_foreground = color(0x61_60_61);
        theme.tab_active_foreground = color(0x1D_1C_1D);
    }
}

/// Slack's fixed avatar chip hues; identity colors, not theme colors. Entries are dark
/// enough to carry white initials in both themes.
const AVATAR_COLORS: [u32; 8] = [
    0x36_C5_F0, 0x2E_B6_7D, 0xE0_1E_5A, 0x7C_5C_FF, 0x00_7A_5A, 0xEC_B2_2E, 0x4A_6F_A5, 0xC2_3E_36,
];

/// The avatar chip color for an author key (stable hash, same author always same hue).
pub fn avatar_color(key: &str) -> Hsla {
    let index = key.bytes().fold(0usize, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(usize::from(byte))
    }) % AVATAR_COLORS.len();
    color(AVATAR_COLORS[index])
}

/// Initials color on avatar chips: white in every theme.
pub fn avatar_foreground() -> Hsla {
    color(0xFF_FF_FF)
}

/// Fixed dark-glass chrome palette for the full-screen image viewer overlay (Slack renders its
/// lightbox dark in both Light and Dark modes).
#[derive(Clone, Copy)]
pub struct ViewerChrome {
    pub panel: Hsla,
    pub glass: Hsla,
    pub button: Hsla,
    pub button_hover: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_dim: Hsla,
}

/// Semantic dark-chrome tokens for [`crate::image_viewer::ImageViewer`].
pub fn viewer_chrome() -> ViewerChrome {
    ViewerChrome {
        panel: color(0x5B_5B_5B),
        glass: color(0x33_35_38).opacity(0.92),
        button: color(0x3A_3D_42).opacity(0.85),
        button_hover: color(0x4A_4D_52).opacity(0.9),
        border: color(0xFF_FF_FF).opacity(0.16),
        text: color(0xD9_DA_DB).opacity(0.95),
        text_muted: color(0xD9_DA_DB).opacity(0.65),
        text_dim: color(0xD9_DA_DB).opacity(0.45),
    }
}

/// Translucent dark scrim used behind play buttons on video thumbnails and link unfurls.
pub fn media_overlay_scrim() -> Hsla {
    hsla(0., 0., 0., 0.62)
}

/// Slightly denser dark scrim for video duration badges and Block Kit video tiles.
pub fn media_overlay_scrim_strong() -> Hsla {
    hsla(0., 0., 0., 0.68)
}

/// Hairline white border over dark media scrims.
pub fn media_overlay_border() -> Hsla {
    hsla(0., 0., 1., 0.25)
}

/// High-contrast white icon/label foreground over dark media scrims.
pub fn media_overlay_foreground() -> Hsla {
    hsla(0., 0., 1., 0.95)
}

/// Theme-aware syntax-highlighting foreground color for a [`crate::syntax::TokenKind`].
pub fn syntax_token_color(kind: crate::syntax::TokenKind, dark: bool, fg: Hsla) -> Hsla {
    use crate::syntax::TokenKind;
    match (kind, dark) {
        (TokenKind::Plain, _) => fg,
        (TokenKind::Keyword, true) => color(0xFF_7B_72),
        (TokenKind::String, true) => color(0xA5_D6_FF),
        (TokenKind::Number, true) => color(0x79_C0_FF),
        (TokenKind::Comment, true) => color(0x8B_94_9E),
        (TokenKind::Tag, true) => color(0x7E_E7_87),
        (TokenKind::Attr, true) => color(0x79_C0_FF),
        (TokenKind::Type, true) => color(0xFF_A6_57),
        (TokenKind::DiffAdd, true) => color(0x7E_E7_87),
        (TokenKind::DiffDel, true) => color(0xFF_A1_98),
        (TokenKind::DiffHunk, true) => color(0xD2_A8_FF),
        (TokenKind::Keyword, false) => color(0xCF_22_2E),
        (TokenKind::String, false) => color(0x0A_30_69),
        (TokenKind::Number, false) => color(0x05_50_AE),
        (TokenKind::Comment, false) => color(0x6E_77_81),
        (TokenKind::Tag, false) => color(0x11_63_29),
        (TokenKind::Attr, false) => color(0x05_50_AE),
        (TokenKind::Type, false) => color(0x95_38_00),
        (TokenKind::DiffAdd, false) => color(0x11_63_29),
        (TokenKind::DiffDel, false) => color(0xCF_22_2E),
        (TokenKind::DiffHunk, false) => color(0x82_50_DF),
    }
}

/// Live huddle card header gradient, tuned for Dark and Light themes.
pub fn huddle_banner_gradient(dark: bool) -> Background {
    if dark {
        linear_gradient(
            135.,
            linear_color_stop(Hsla::from(rgb(0x1E_3A_34)), 0.),
            linear_color_stop(Hsla::from(rgb(0x28_43_59)), 1.),
        )
    } else {
        linear_gradient(
            135.,
            linear_color_stop(Hsla::from(rgb(0x9B_C7_C9)), 0.),
            linear_color_stop(Hsla::from(rgb(0xD5_E2_B8)), 1.),
        )
    }
}

/// Dark neutral backdrop behind video frames, screen thumbnails, and media scrim pills.
pub fn media_backdrop_bg() -> Hsla {
    Hsla::from(rgb(0x14_17_1C))
}

/// High-contrast white ink for badges/controls sitting directly atop dark media scrims.
pub fn media_overlay_fg() -> Hsla {
    Hsla::from(rgb(0xFF_FF_FF))
}

/// Warm Slack brand gold used for the emoji picker tab icon accent.
pub fn emoji_tab_accent() -> Hsla {
    Hsla::from(rgb(0xEC_B2_2E))
}

/// Semantic tag color for Slack Lists select/multi-select option chips.
pub fn list_choice_color(name: &str, theme: &Theme) -> Hsla {
    match name {
        "green" => theme.success,
        "red" => theme.danger,
        "blue" | "indigo" => theme.link,
        "yellow" | "orange" => rgb(0xD9_77_06).into(),
        "purple" => rgb(0x7C_3A_ED).into(),
        "pink" => rgb(0xDB_27_77).into(),
        "cyan" => rgb(0x08_91_B2).into(),
        "brown" => rgb(0x92_40_0E).into(),
        _ => theme.muted_foreground,
    }
}

/// Stage surface, participant card, and overlay tokens for the Huddle pop-out window.
#[derive(Clone, Copy)]
pub struct HuddleStageTokens {
    pub light_ctrl_active_bg: Hsla,
    pub light_ctrl_active_fg: Hsla,
    pub stage_bg: Background,
    pub stage_solid: Hsla,
    pub tile_bg: Hsla,
    pub tile_border: Hsla,
    pub tile_pill_bg: Hsla,
    pub tile_pill_fg: Hsla,
    pub scrim_bg: Hsla,
    pub scrim_fg: Hsla,
    pub scrim_muted: Hsla,
}

/// Resolve the Huddle pop-out stage palette for an optional room background preset and active theme.
pub fn huddle_stage_tokens(bg_id: Option<&str>, theme: &Theme) -> HuddleStageTokens {
    let dark_mode = theme.mode.is_dark();
    let (stage_a, stage_b, stage_solid, tile_bg, stage_is_dark) = match bg_id {
        Some("meadow") => (
            color(0xBC_E3_CC),
            color(0xDC_F2_E4),
            color(0xCB_EA_D7),
            color(0xFF_FF_FF),
            false,
        ),
        Some("sky") => (
            color(0xC5_E5_FA),
            color(0xE8_F5_FE),
            color(0xD6_EE_FC),
            color(0xFF_FF_FF),
            false,
        ),
        Some("midnight") => (
            color(0x0E_16_29),
            color(0x1B_29_4A),
            color(0x10_17_2A),
            color(0x1B_24_40),
            true,
        ),
        Some("sunset") => (
            color(0x2B_18_18),
            color(0x47_23_29),
            color(0x2B_18_18),
            color(0x3B_22_22),
            true,
        ),
        Some("forest") => (
            color(0x10_24_1C),
            color(0x1C_3D_30),
            color(0x11_22_1B),
            color(0x1B_33_28),
            true,
        ),
        Some("ocean") => (
            color(0x0D_22_30),
            color(0x18_3A_52),
            color(0x0F_21_2E),
            color(0x18_32_44),
            true,
        ),
        Some("plum") => (
            color(0x24_14_2B),
            color(0x3D_1D_47),
            color(0x24_14_2B),
            color(0x35_1F_3E),
            true,
        ),
        _ if dark_mode => {
            let top = theme.background.blend(theme.title_bar.opacity(0.35));
            let bot = theme.background;
            (top, bot, bot, theme.muted, true)
        }
        _ => (
            color(0xC2_E6_CE),
            color(0xE1_F4_E8),
            color(0xD2_EE_DC),
            color(0xFF_FF_FF),
            false,
        ),
    };

    let stage_bg = linear_gradient(
        145.,
        linear_color_stop(stage_a, 0.),
        linear_color_stop(stage_b, 1.),
    );
    let (tile_border, tile_pill_bg, tile_pill_fg) = if stage_is_dark {
        (
            color(0xFF_FF_FF).opacity(0.13),
            color(0x0E_10_14).opacity(0.80),
            color(0xFF_FF_FF),
        )
    } else {
        (
            color(0xFF_FF_FF).opacity(0.93),
            color(0xFF_FF_FF).opacity(0.90),
            color(0x1D_1C_1D),
        )
    };

    HuddleStageTokens {
        light_ctrl_active_bg: color(0x2B_2D_31),
        light_ctrl_active_fg: color(0xFF_FF_FF),
        stage_bg,
        stage_solid,
        tile_bg,
        tile_border,
        tile_pill_bg,
        tile_pill_fg,
        scrim_bg: color(0x14_17_1C).opacity(0.89),
        scrim_fg: color(0xFF_FF_FF),
        scrim_muted: color(0xD1_D2_D3),
    }
}

/// Fixed dark-glass palette for the always-on-top floating huddle control bar, binding semantic
/// danger/accent hues to the active theme while keeping crisp contrast over any desktop wallpaper.
#[derive(Clone, Copy)]
pub struct FloatingHuddlePalette {
    pub bar_bg: Hsla,
    pub bar_border: Hsla,
    pub active_bg: Hsla,
    pub active_hover: Hsla,
    pub active_fg: Hsla,
    pub inactive_bg: Hsla,
    pub inactive_hover: Hsla,
    pub white: Hsla,
    pub danger_bg: Hsla,
    pub danger_hover: Hsla,
    pub pen_active_bg: Hsla,
    pub pen_active_hover: Hsla,
    pub subtle_fg: Hsla,
}

/// Semantic tokens for [`crate::popout::floating_controls::HuddleFloatingPanel`].
pub fn floating_huddle_palette(theme: &Theme) -> FloatingHuddlePalette {
    let white = color(0xFF_FF_FF);
    FloatingHuddlePalette {
        bar_bg: color(0x15_1A_21),
        bar_border: white.opacity(0.14),
        active_bg: color(0xE6_E8_EB),
        active_hover: color(0xF5_F6_F7),
        active_fg: color(0x1A_1D_21),
        inactive_bg: color(0x4B_4F_55),
        inactive_hover: color(0x5D_62_6A),
        white,
        danger_bg: theme.danger,
        danger_hover: theme.danger_hover,
        pen_active_bg: color(0x1D_9B_D1),
        pen_active_hover: color(0x2A_AA_E0),
        subtle_fg: white.opacity(0.53),
    }
}

/// Chromatic hue color for Huddle screen-share drawing pens, strokes, and live cursor halos.
pub fn huddle_draw_hue_color(hue: i32, saturation: f32, lightness: f32, alpha: f32) -> Hsla {
    let h = (hue.rem_euclid(360)) as f32 / 360.0;
    hsla(h, saturation, lightness, alpha)
}

/// White outer halo ring around remote Huddle drawing cursors.
pub fn huddle_draw_cursor_ring() -> Hsla {
    hsla(0.0, 0.0, 1.0, 0.95)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn palette_follows_the_workspace_sidebar_theme() {
        let hoth = json!({ "sidebar_theme": "hoth_theme" });
        assert_eq!(palette_from_prefs(&hoth), Some(Palette::Default));
        let aubergine = json!({ "sidebar_theme": "aubergine_theme" });
        assert_eq!(palette_from_prefs(&aubergine), Some(Palette::Aubergine));
        assert_eq!(palette_from_prefs(&json!({})), None);
    }

    #[test]
    fn parses_and_formats_custom_hex_theme_colors() {
        assert_eq!(parse_hex_rgb("#3F0E40"), Some(0x3F_0E_40));
        assert_eq!(parse_hex_rgb("1164a3"), Some(0x11_64_A3));
        assert_eq!(parse_hex_rgb("#GGG"), None);
        assert_eq!(format_hex_rgb(0x11_64_A3), "#1164A3");
        let p = Palette::CustomHex {
            sidebar: 0x11_22_33,
            accent: 0x44_55_66,
        };
        assert_eq!(p.rgb_pair(), Some((0x11_22_33, 0x44_55_66)));

        // Slack's classic 8-color and 10-color theme strings (`F-SHELL-078`, `F-SHELL-079`).
        let legacy = "#3F0E40,#350D36,#1164A3,#FFFFFF,#350D36,#FFFFFF,#2BAC76,#CD2553";
        assert_eq!(
            parse_slack_theme_string(legacy),
            Some((0x3F_0E_40, 0x11_64_A3))
        );
        assert_eq!(
            parse_slack_theme_string("#123456, #abcdef"),
            Some((0x12_34_56, 0xAB_CD_EF))
        );
        assert_eq!(parse_slack_theme_string("#123456"), None);
        let exported = format_slack_theme_string(p, true);
        assert_eq!(
            parse_slack_theme_string(&exported),
            Some((0x11_22_33, 0x44_55_66))
        );
    }
}
