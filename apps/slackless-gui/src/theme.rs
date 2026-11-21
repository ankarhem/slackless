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
    App, Background, Global, Hsla, Pixels, linear_color_stop, linear_gradient, px, rgb,
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

/// The Font in effect, re-applied whenever a theme change reloads the kit's own.
struct UiFont(usize);

impl Global for UiFont {}

/// Focus mode is on: the theme is drained to grayscale, badges included.
struct Focus(bool);

impl Global for Focus {}

fn focused(cx: &App) -> bool {
    cx.try_global::<Focus>().is_some_and(|focus| focus.0)
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
        eprintln!("slackless: could not load Lato ({e})");
    }
    activate(SlackTheme::Dark, cx);
}

/// The Font in effect, an index into [`FONT_LABELS`].
pub fn font(cx: &App) -> usize {
    cx.try_global::<UiFont>()
        .map_or(DEFAULT_FONT, |font| font.0)
}

/// Switch the UI font to [`FONT_LABELS`]`[font]` (a valid index); a no-op when it is in effect.
pub fn set_font(font: usize, cx: &mut App) {
    if self::font(cx) == font {
        return;
    }
    cx.set_global(UiFont(font));
    Theme::update(cx, |theme| theme.font_family = FONT_FAMILIES[font].into());
}

/// Switch the whole app to `theme` with the default color theme and repaint every window.
pub fn activate(theme: SlackTheme, cx: &mut App) {
    activate_with(theme, Palette::Default, false, cx);
}

/// Switch the whole app to `theme` tinted by the color theme `palette`, in focus mode's
/// grayscale when `focus` is set.
///
/// `Theme::change` loads the registered mode palette; the Slack palette (and the color theme
/// over it) is applied in a second pass, because a mode change reloads that mode's theme over
/// same-closure edits.
pub fn activate_with(theme: SlackTheme, palette: Palette, focus: bool, cx: &mut App) {
    let mode = theme.mode();
    let family = FONT_FAMILIES[font(cx)];
    cx.set_global(Focus(focus));
    Theme::change(mode, None, cx);
    Theme::update(cx, |theme| {
        apply(mode.is_dark(), theme);
        tint(palette, mode.is_dark(), theme);
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
            Palette::Custom { .. } => "Custom",
        }
    }

    /// `(sidebar, accent)` hex colors; `None` for the untinted default.
    fn colors(self) -> Option<(u32, u32)> {
        match self {
            Palette::Default => None,
            Palette::Aubergine => Some((0x3F_0E_40, 0x11_64_A3)),
            Palette::Clementine => Some((0x7A_2E_12, 0xE8_71_2D)),
            Palette::Banana => Some((0x5C_4A_12, 0xD9_A5_19)),
            Palette::Jade => Some((0x0B_3B_2E, 0x2B_AC_76)),
            Palette::Lagoon => Some((0x0F_3D_5C, 0x2D_9E_E0)),
            Palette::Barbra => Some((0x5A_18_40, 0xE0_1E_5A)),
            Palette::MoodIndigo => Some((0x1F_2A_4D, 0x5B_6C_FF)),
            Palette::Custom { sidebar, accent } => Some((
                CUSTOM_SIDEBARS[sidebar % CUSTOM_SIDEBARS.len()],
                CUSTOM_ACCENTS[accent % CUSTOM_ACCENTS.len()],
            )),
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

    /// The swatch shown for this theme in the picker: its sidebar color, or the current
    /// sidebar token for the default theme.
    pub fn swatch(self, current_sidebar: Hsla) -> (Hsla, Hsla) {
        match self.colors() {
            Some((sidebar, accent)) => (color(sidebar), color(accent)),
            None => (current_sidebar, color(0x11_64_A3)),
        }
    }
}

/// The color theme Slack stores for the signed-in user (`sidebar_theme`), so slackless opens
/// tinted like Slack itself; `None` when the pref is absent. Light, dark or system is not a
/// workspace pref: Slack keeps its Color Mode on the client, and so does slackless.
pub fn palette_from_prefs(prefs: &crate::blockkit::serde_json::Value) -> Option<Palette> {
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

/// A custom-builder swatch as a theme color.
pub fn swatch_color(hex: u32) -> Hsla {
    color(hex)
}

/// Lay a color theme over the base palette: the sidebar takes the theme's color (with light
/// text on it in both modes) and selection / focus use its accent.
fn tint(palette: Palette, dark: bool, theme: &mut Theme) {
    let Some((sidebar, accent)) = palette.colors() else {
        return;
    };
    let sidebar = color(sidebar);
    // Dark mode keeps the sidebar subdued so it does not outshine the message column.
    theme.sidebar = if dark {
        sidebar.blend(color(0x1A_1D_21).opacity(0.35))
    } else {
        sidebar
    };
    theme.sidebar_foreground = color(0xF2_F2_F2);
    theme.sidebar_border = sidebar.blend(color(0x00_00_00).opacity(0.25));
    theme.sidebar_accent = color(accent);
    theme.sidebar_accent_foreground = color(0xFF_FF_FF);
    theme.sidebar_primary = color(0xFF_FF_FF);
    theme.sidebar_primary_foreground = sidebar;
    theme.ring = color(accent);
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

/// Slack's tab rail fill (`theme-surf-inv-sec`): on dark themes the near-black Slack 4.53
/// measures (#0E171B, a dark wash over its chrome); on light ones a near-opaque white panel
/// (measured at 90% over the chrome's blue, top and bottom).
pub fn tab_rail_wash(cx: &App) -> Hsla {
    if cx.theme().mode.is_dark() {
        color(0x0E_17_1B)
    } else {
        color(0xFF_FF_FF).opacity(0.9)
    }
}

/// The rail's fills over that wash as the sidebar foreground's opacity: the active tile and
/// the round control buttons measure 30% on dark themes and 13% on light ones.
pub fn rail_fill(cx: &App) -> f32 {
    if cx.theme().mode.is_dark() { 0.3 } else { 0.13 }
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
/// (#fff2df on white).
pub fn connect_bar(cx: &App) -> Hsla {
    cx.theme().background.blend(color(0xFF_A5_00).opacity(0.13))
}

/// Slack's tint on the channel message whose thread is open beside it: a warm wash in
/// light (#fffae0 on white) that barely registers in dark (#171a1e on #1a1d21, measured).
pub fn thread_root(cx: &App) -> Hsla {
    if cx.theme().mode == ThemeMode::Dark {
        color(0x17_1A_1E)
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
    let (side, glow) = if theme.mode.is_dark() {
        (900., color(0x33_3B_40))
    } else {
        (1400., color(0xF5_FA_FB))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockkit::serde_json::json;

    #[test]
    fn palette_follows_the_workspace_sidebar_theme() {
        let hoth = json!({ "sidebar_theme": "hoth_theme" });
        assert_eq!(palette_from_prefs(&hoth), Some(Palette::Default));
        let aubergine = json!({ "sidebar_theme": "aubergine_theme" });
        assert_eq!(palette_from_prefs(&aubergine), Some(Palette::Aubergine));
        assert_eq!(palette_from_prefs(&json!({})), None);
    }
}
