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

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode};
use gpui_kit::gpui::{App, Hsla, rgb};

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

    /// The other palette.
    pub fn toggle(self) -> Self {
        match self {
            SlackTheme::Dark => SlackTheme::Light,
            SlackTheme::Light => SlackTheme::Dark,
        }
    }
}

/// Install the startup theme: Slack Dark, per the reference screenshot.
pub fn init(cx: &mut App) {
    activate(SlackTheme::Dark, cx);
}

/// Switch the whole app to `theme` with the default color theme and repaint every window.
pub fn activate(theme: SlackTheme, cx: &mut App) {
    activate_with(theme, Palette::Default, cx);
}

/// Switch the whole app to `theme` tinted by the color theme `palette`.
///
/// `Theme::change` loads the registered mode palette; the Slack palette (and the color theme
/// over it) is applied in a second pass, because a mode change reloads that mode's theme over
/// same-closure edits.
pub fn activate_with(theme: SlackTheme, palette: Palette, cx: &mut App) {
    let mode = theme.mode();
    Theme::change(mode, None, cx);
    Theme::update(cx, |theme| {
        apply(mode.is_dark(), theme);
        tint(palette, mode.is_dark(), theme);
    });
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

/// The appearance Slack stores for the signed-in user: `messages_theme` chooses light or dark
/// and `sidebar_theme` the color theme, so slackless opens looking like Slack itself.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Appearance {
    pub theme: SlackTheme,
    pub palette: Palette,
}

/// Read Slack's theme prefs into an [`Appearance`]; `None` when the message theme is absent.
pub fn appearance_from_prefs(prefs: &crate::blockkit::serde_json::Value) -> Option<Appearance> {
    let messages = prefs
        .get("messages_theme")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let theme = if messages.starts_with("light") {
        SlackTheme::Light
    } else if messages.starts_with("dark") {
        SlackTheme::Dark
    } else {
        return None;
    };
    let sidebar = prefs
        .get("sidebar_theme")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    Some(Appearance {
        theme,
        palette: Palette::from_slack(sidebar),
    })
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
        // Slack dims the window behind a modal with a light black wash.
        theme.overlay = color(0x00_00_00).opacity(0.2);
        theme.success = color(0x2B_AC_76); // presence green
        theme.success_foreground = color(0xFF_FF_FF);
        theme.warning = color(0xEC_B2_2E);
        theme.warning_foreground = color(0x1A_1D_21);
        theme.sidebar = color(0x1A_1D_21);
        theme.sidebar_border = color(0x30_36_3D);
        theme.sidebar_foreground = color(0xD1_D2_D3);
        theme.sidebar_primary = color(0xD1_D2_D3); // unread pill: near-white bg
        theme.sidebar_primary_foreground = color(0x1A_1D_21);
        theme.sidebar_accent = color(0x11_64_A3); // selected row blue
        theme.sidebar_accent_foreground = color(0xFF_FF_FF);
        theme.title_bar = color(0x1A_1D_21);
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
        theme.title_bar = color(0xDC_EF_F8);
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
    fn appearance_follows_the_workspace_theme_prefs() {
        let light = json!({
            "messages_theme": "light_with_avatars",
            "sidebar_theme": "hoth_theme",
        });
        let appearance = appearance_from_prefs(&light).expect("Slack's light prefs");
        assert!(appearance.theme == SlackTheme::Light);
        assert!(appearance.palette == Palette::Default);

        let dark = json!({
            "messages_theme": "dark_with_avatars",
            "sidebar_theme": "aubergine_theme",
        });
        let appearance = appearance_from_prefs(&dark).expect("Slack's dark prefs");
        assert!(appearance.theme == SlackTheme::Dark);
        assert!(appearance.palette == Palette::Aubergine);

        assert!(appearance_from_prefs(&json!({})).is_none());
    }
}
