//! Emoji: the shortcode catalog every view resolves through (reaction chips, mrkdwn and
//! rich-text `:shortcode:`s, statuses), the workspace's custom emoji (fixture images), and the
//! searchable emoji picker overlay the root mounts for the composer and add-reaction buttons.
//!
//! The picker is one long-lived entity laid out like Slack's: a row of category tabs (search /
//! frequently used first, custom last), the search field, one scrolling grid of titled sections
//! (Frequently Used, Getting Work Done, then each category) that a tab scrolls to, and a footer
//! with the hover preview and the skin-tone choice. A pick emits [`EmojiPickerEvent::Pick`]; the
//! root decides whether it inserts into a composer or adds a reaction. Frequently Used starts
//! from the user's `emoji_use` pref ([`frequent_from_prefs`]), else a default set, and moves
//! each pick to the front (in memory).
//!
//! The grid's rows are built once per open, search or skin-tone change and drawn lazily: a frame
//! lays out only the rows in view.
//!
//! Skin tones follow Slack's wire format: a toned pick is named `wave::skin-tone-3`, and the
//! glyph carries the matching Fitzpatrick modifier (U+1F3FB..U+1F3FF).
//!
//! Design tokens: colors via `cx.theme()`; the grid metrics are the only local constants.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, AppContext as _, BoxShadow, Context, Entity, EventEmitter, Focusable as _,
    FontWeight, Hsla, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Pixels,
    Render, ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Subscription, Task, UniformListScrollHandle, Window, div, img, point, px,
    rgb, uniform_list,
};
use slackless_core::api::GifItem;

use crate::blockkit::{Json, serde_json};
use crate::fixtures::Fixtures;
use gpui_kit::gpui::ImageSource;

/// One catalog emoji: Slack shortcode, default glyph, and whether it takes a skin tone.
pub struct Emoji {
    pub name: &'static str,
    pub glyph: &'static str,
    pub skin: bool,
}

const fn e(name: &'static str, glyph: &'static str) -> Emoji {
    Emoji {
        name,
        glyph,
        skin: false,
    }
}

/// A skin-tone capable emoji.
const fn s(name: &'static str, glyph: &'static str) -> Emoji {
    Emoji {
        name,
        glyph,
        skin: true,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    Frequent,
    People,
    Nature,
    Food,
    Travel,
    Activity,
    Objects,
    Symbols,
    Flags,
    Custom,
}

/// The picker's tabs in Slack's order with outline icons: the search glyph Slack shows for
/// Frequently Used first, then the kit's nearest match for each of Slack's own (a heart for its
/// peace sign, a volleyball for its football, a hash for its logo).
const TABS: [(Category, IconName); 10] = [
    (Category::Frequent, IconName::Search),
    (Category::People, IconName::FaceGrinning),
    (Category::Nature, IconName::Leaf),
    (Category::Food, IconName::Hamburger),
    (Category::Travel, IconName::Plane),
    (Category::Activity, IconName::Volleyball),
    (Category::Objects, IconName::Lightbulb),
    (Category::Symbols, IconName::Heart),
    (Category::Flags, IconName::Flag),
    (Category::Custom, IconName::Hash),
];

const PEOPLE: &[Emoji] = &[
    e("grinning", "😀"),
    e("smiley", "😃"),
    e("smile", "😄"),
    e("grin", "😁"),
    e("laughing", "😆"),
    e("sweat_smile", "😅"),
    e("joy", "😂"),
    e("rolling_on_the_floor_laughing", "🤣"),
    e("slightly_smiling_face", "🙂"),
    e("upside_down_face", "🙃"),
    e("wink", "😉"),
    e("blush", "😊"),
    e("innocent", "😇"),
    e("heart_eyes", "😍"),
    e("star-struck", "🤩"),
    e("kissing_heart", "😘"),
    e("yum", "😋"),
    e("stuck_out_tongue", "😛"),
    e("thinking_face", "🤔"),
    e("face_with_raised_eyebrow", "🤨"),
    e("neutral_face", "😐"),
    e("expressionless", "😑"),
    e("no_mouth", "😶"),
    e("smirk", "😏"),
    e("unamused", "😒"),
    e("face_with_rolling_eyes", "🙄"),
    e("grimacing", "😬"),
    e("relieved", "😌"),
    e("pensive", "😔"),
    e("sleepy", "😪"),
    e("sleeping", "😴"),
    e("mask", "😷"),
    e("face_with_thermometer", "🤒"),
    e("nerd_face", "🤓"),
    e("sunglasses", "😎"),
    e("confused", "😕"),
    e("worried", "😟"),
    e("open_mouth", "😮"),
    e("astonished", "😲"),
    e("flushed", "😳"),
    e("cry", "😢"),
    e("sob", "😭"),
    e("scream", "😱"),
    e("rage", "😡"),
    e("skull", "💀"),
    e("clown_face", "🤡"),
    e("ghost", "👻"),
    e("robot_face", "🤖"),
    s("wave", "👋"),
    s("raised_hand", "✋"),
    s("ok_hand", "👌"),
    s("v", "✌️"),
    s("crossed_fingers", "🤞"),
    s("point_up", "☝️"),
    s("thumbsup", "👍"),
    s("thumbsdown", "👎"),
    s("clap", "👏"),
    s("raised_hands", "🙌"),
    s("pray", "🙏"),
    s("muscle", "💪"),
    e("handshake", "🤝"),
    e("eyes", "👀"),
    e("brain", "🧠"),
    s("face_palm", "🤦"),
    s("shrug", "🤷"),
];

const NATURE: &[Emoji] = &[
    e("dog", "🐶"),
    e("cat", "🐱"),
    e("fox_face", "🦊"),
    e("bear", "🐻"),
    e("panda_face", "🐼"),
    e("unicorn_face", "🦄"),
    e("bee", "🐝"),
    e("bug", "🐛"),
    e("butterfly", "🦋"),
    e("turtle", "🐢"),
    e("octopus", "🐙"),
    e("whale", "🐳"),
    e("sunflower", "🌻"),
    e("rose", "🌹"),
    e("seedling", "🌱"),
    e("evergreen_tree", "🌲"),
    e("palm_tree", "🌴"),
    e("cactus", "🌵"),
    e("fallen_leaf", "🍂"),
    e("sunny", "☀️"),
    e("cloud", "☁️"),
    e("rainbow", "🌈"),
    e("snowflake", "❄️"),
    e("zap", "⚡"),
    e("fire", "🔥"),
    e("droplet", "💧"),
    e("ocean", "🌊"),
    e("star", "⭐"),
    e("sparkles", "✨"),
    e("crescent_moon", "🌙"),
];

const FOOD: &[Emoji] = &[
    e("coffee", "☕"),
    e("tea", "🍵"),
    e("beer", "🍺"),
    e("wine_glass", "🍷"),
    e("pizza", "🍕"),
    e("hamburger", "🍔"),
    e("fries", "🍟"),
    e("taco", "🌮"),
    e("sushi", "🍣"),
    e("ramen", "🍜"),
    e("sandwich", "🥪"),
    e("cake", "🍰"),
    e("birthday", "🎂"),
    e("doughnut", "🍩"),
    e("cookie", "🍪"),
    e("apple", "🍎"),
    e("banana", "🍌"),
    e("avocado", "🥑"),
    e("popcorn", "🍿"),
    e("ice_cream", "🍨"),
];

const ACTIVITY: &[Emoji] = &[
    e("tada", "🎉"),
    e("confetti_ball", "🎊"),
    e("balloon", "🎈"),
    e("gift", "🎁"),
    e("trophy", "🏆"),
    e("medal", "🏅"),
    e("soccer", "⚽"),
    e("basketball", "🏀"),
    e("tennis", "🎾"),
    e("video_game", "🎮"),
    e("dart", "🎯"),
    e("game_die", "🎲"),
    e("art", "🎨"),
    e("guitar", "🎸"),
    e("headphones", "🎧"),
    e("microphone", "🎤"),
    s("runner", "🏃"),
    s("bicyclist", "🚴"),
];

const TRAVEL: &[Emoji] = &[
    e("rocket", "🚀"),
    e("airplane", "✈️"),
    e("car", "🚗"),
    e("bus", "🚌"),
    e("train", "🚆"),
    e("bike", "🚲"),
    e("ship", "🚢"),
    e("house", "🏠"),
    e("house_with_garden", "🏡"),
    e("office", "🏢"),
    e("earth_africa", "🌍"),
    e("mountain", "⛰️"),
    e("beach_with_umbrella", "🏖️"),
    e("world_map", "🗺️"),
    e("tent", "⛺"),
    e("construction", "🚧"),
];

const OBJECTS: &[Emoji] = &[
    e("computer", "💻"),
    e("keyboard", "⌨️"),
    e("iphone", "📱"),
    e("bulb", "💡"),
    e("calendar", "📆"),
    e("spiral_calendar_pad", "🗓️"),
    e("memo", "📝"),
    e("pencil2", "✏️"),
    e("books", "📚"),
    e("package", "📦"),
    e("lock", "🔒"),
    e("key", "🔑"),
    e("hammer_and_wrench", "🛠️"),
    e("gear", "⚙️"),
    e("mag", "🔍"),
    e("link", "🔗"),
    e("paperclip", "📎"),
    e("pushpin", "📌"),
    e("chart_with_upwards_trend", "📈"),
    e("bell", "🔔"),
    e("hourglass", "⌛"),
    e("alarm_clock", "⏰"),
    e("money_with_wings", "💸"),
    e("speech_balloon", "💬"),
    e("email", "📧"),
];

const SYMBOLS: &[Emoji] = &[
    e("heart", "❤️"),
    e("orange_heart", "🧡"),
    e("yellow_heart", "💛"),
    e("green_heart", "💚"),
    e("blue_heart", "💙"),
    e("purple_heart", "💜"),
    e("broken_heart", "💔"),
    e("100", "💯"),
    e("white_check_mark", "✅"),
    e("heavy_check_mark", "✔️"),
    e("x", "❌"),
    e("warning", "⚠️"),
    e("no_entry", "⛔"),
    e("question", "❓"),
    e("exclamation", "❗"),
    e("bangbang", "‼️"),
    e("recycle", "♻️"),
    e("zzz", "💤"),
    e("red_circle", "🔴"),
    e("large_green_circle", "🟢"),
    e("large_blue_circle", "🔵"),
    e("arrow_up", "⬆️"),
    e("arrow_down", "⬇️"),
    e("repeat", "🔁"),
    e("new", "🆕"),
    e("sos", "🆘"),
];

/// Flags in Slack's order and naming (`flag-se`, the ten oldest by bare code).
const FLAGS: &[Emoji] = &[
    e("checkered_flag", "🏁"),
    e("triangular_flag_on_post", "🚩"),
    e("crossed_flags", "🎌"),
    e("waving_black_flag", "🏴"),
    e("waving_white_flag", "🏳️"),
    e("rainbow-flag", "🏳️‍🌈"),
    e("transgender_flag", "🏳️‍⚧️"),
    e("pirate_flag", "🏴‍☠️"),
    e("flag-ar", "🇦🇷"),
    e("flag-at", "🇦🇹"),
    e("flag-au", "🇦🇺"),
    e("flag-be", "🇧🇪"),
    e("flag-br", "🇧🇷"),
    e("flag-ca", "🇨🇦"),
    e("flag-ch", "🇨🇭"),
    e("cn", "🇨🇳"),
    e("de", "🇩🇪"),
    e("flag-dk", "🇩🇰"),
    e("flag-ee", "🇪🇪"),
    e("es", "🇪🇸"),
    e("flag-eu", "🇪🇺"),
    e("flag-fi", "🇫🇮"),
    e("fr", "🇫🇷"),
    e("gb", "🇬🇧"),
    e("flag-ie", "🇮🇪"),
    e("flag-in", "🇮🇳"),
    e("flag-is", "🇮🇸"),
    e("it", "🇮🇹"),
    e("jp", "🇯🇵"),
    e("kr", "🇰🇷"),
    e("flag-mx", "🇲🇽"),
    e("flag-nl", "🇳🇱"),
    e("flag-no", "🇳🇴"),
    e("flag-nz", "🇳🇿"),
    e("flag-pl", "🇵🇱"),
    e("flag-pt", "🇵🇹"),
    e("flag-se", "🇸🇪"),
    e("flag-ua", "🇺🇦"),
    e("us", "🇺🇸"),
    e("flag-za", "🇿🇦"),
];

/// The picker's categories in Slack's order, under Slack's section titles.
const CATALOG: [(Category, &str, &[Emoji]); 8] = [
    (Category::People, "Smileys & People", PEOPLE),
    (Category::Nature, "Animals & Nature", NATURE),
    (Category::Food, "Food & Drink", FOOD),
    (Category::Travel, "Travel & Places", TRAVEL),
    (Category::Activity, "Activities", ACTIVITY),
    (Category::Objects, "Objects", OBJECTS),
    (Category::Symbols, "Symbols", SYMBOLS),
    (Category::Flags, "Flags", FLAGS),
];

/// Slack's "Getting Work Done" section, under Frequently Used and without a tab of its own.
const WORK: [&str; 26] = [
    "white_check_mark",
    "eyes",
    "raised_hands",
    "pray",
    "heavy_plus_sign",
    "clap",
    "bulb",
    "dart",
    "wave",
    "thumbsup",
    "tada",
    "one",
    "two",
    "three",
    "mega",
    "white_circle",
    "large_blue_circle",
    "red_circle",
    "cool",
    "ok",
    "sos",
    "rotating_light",
    "rocket",
    "fire",
    "heart",
    "100",
];

/// Alternate shortcodes Slack accepts for catalog emoji.
const ALIASES: &[(&str, &str)] = &[
    ("+1", "thumbsup"),
    ("-1", "thumbsdown"),
    ("simple_smile", "slightly_smiling_face"),
    ("hand", "raised_hand"),
    ("facepalm", "face_palm"),
];

/// Frequently Used when the user's prefs carry no `emoji_use` (the sample workspace).
const FREQUENT_SEED: &[&str] = &[
    "thumbsup",
    "white_check_mark",
    "eyes",
    "raised_hands",
    "tada",
    "joy",
    "heart",
    "pray",
    "fire",
    "rocket",
    "100",
    "shipit",
];
/// Slack's cap on Frequently Used: four rows.
const FREQUENT_MAX: usize = 36;

/// Grid metrics: `COLUMNS` cells of `CELL` x `ROW` (a section title takes a row too, so the
/// grid renders lazily as a uniform list), the scrolling height and the emoji size. The width
/// adds the grid's `px_4` padding on both sides.
const COLUMNS: usize = 9;
const CELL: f32 = 36.;
const ROW: f32 = 32.;
const GRID_HEIGHT: f32 = 300.;
const GLYPH: f32 = 22.;
const PICKER_WIDTH: f32 = COLUMNS as f32 * CELL + 32.;

fn lookup(name: &str) -> Option<&'static Emoji> {
    let name = ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name, |(_, target)| *target);
    CATALOG
        .iter()
        .flat_map(|(_, _, list)| list.iter())
        .find(|emoji| emoji.name == name)
}

/// `wave` from `wave::skin-tone-3`.
fn base_name(shortcode: &str) -> &str {
    shortcode.split("::").next().unwrap_or(shortcode)
}

/// The Slack skin tone (2..=6) a shortcode carries; 1 is the default yellow.
fn tone_of(shortcode: &str) -> u8 {
    shortcode
        .split("::")
        .nth(1)
        .and_then(|suffix| suffix.strip_prefix("skin-tone-"))
        .and_then(|tone| tone.parse().ok())
        .filter(|tone| (2..=6).contains(tone))
        .unwrap_or(1)
}

/// `emoji`'s glyph with skin `tone` applied (the modifier follows the base code point; a
/// trailing variation selector is dropped, as the modifier implies emoji presentation).
fn toned(emoji: &Emoji, tone: u8) -> String {
    if !emoji.skin || tone < 2 {
        return emoji.glyph.to_owned();
    }
    let mut chars = emoji.glyph.chars();
    let mut glyph = String::new();
    glyph.extend(chars.next());
    glyph.extend(char::from_u32(0x1F3FB + u32::from(tone - 2)));
    glyph.extend(chars.filter(|c| *c != '\u{FE0F}'));
    glyph
}

/// `name` with Slack's skin-tone suffix when `tone` is not the default.
fn toned_name(name: &str, tone: u8) -> String {
    if tone < 2 {
        name.to_owned()
    } else {
        format!("{name}::skin-tone-{tone}")
    }
}

/// The default glyph for a bare shortcode (`palm_tree`, no colons; a skin-tone suffix is
/// ignored), if it is a standard emoji.
pub(crate) fn glyph(shortcode: &str) -> Option<&'static str> {
    let name = base_name(shortcode);
    lookup(name)
        .map(|emoji| emoji.glyph)
        .or_else(|| unicode_emoji(name).map(emojis::Emoji::as_str))
}

/// The glyph a shortcode displays as, with its `::skin-tone-N` suffix applied.
pub(crate) fn display_glyph(shortcode: &str) -> Option<String> {
    let name = base_name(shortcode);
    let tone = tone_of(shortcode);
    if let Some(emoji) = lookup(name) {
        return Some(toned(emoji, tone));
    }
    let emoji = unicode_emoji(name)?;
    let emoji = skin_tone(tone)
        .and_then(|skin| emoji.with_skin_tone(skin))
        .unwrap_or(emoji);
    Some(emoji.as_str().to_owned())
}

/// The `emojis` crate's skin tone for Slack tone `tone` (2..=6); `None` for the default.
fn skin_tone(tone: u8) -> Option<emojis::SkinTone> {
    match tone {
        2 => Some(emojis::SkinTone::Light),
        3 => Some(emojis::SkinTone::MediumLight),
        4 => Some(emojis::SkinTone::Medium),
        5 => Some(emojis::SkinTone::MediumDark),
        6 => Some(emojis::SkinTone::Dark),
        _ => None,
    }
}

/// Every standard emoji beyond the picker's [`CATALOG`], by the `emojis` crate's GitHub
/// shortcodes, which match Slack's for nearly all of them. Slack hyphenates multi-person
/// sequences (`man-woman-girl-boy`) where GitHub underscores them, families under `family_`.
fn unicode_emoji(name: &str) -> Option<&'static emojis::Emoji> {
    let underscored = name.replace('-', "_");
    emojis::get_by_shortcode(name)
        .or_else(|| emojis::get_by_shortcode(&underscored))
        .or_else(|| emojis::get_by_shortcode(&format!("family_{underscored}")))
}

/// One emoji at `size`: the unicode glyph, the workspace custom image, or a literal `:name:`.
pub(crate) fn emoji_element(shortcode: &str, fixtures: &Fixtures, size: Pixels) -> AnyElement {
    EmojiArt::of(shortcode, fixtures).element(size)
}

/// What a shortcode draws as, resolved once so it can be drawn later without the fixtures
/// (a tooltip builder): a glyph, a workspace emoji image, or its `:shortcode:` text.
#[derive(Clone)]
pub(crate) enum EmojiArt {
    Glyph(String),
    Image(ImageSource),
    Text(String),
}

impl EmojiArt {
    pub(crate) fn of(shortcode: &str, fixtures: &Fixtures) -> Self {
        if let Some(glyph) = display_glyph(shortcode) {
            Self::Glyph(glyph)
        } else if let Some(image) = fixtures.custom_emoji(base_name(shortcode)) {
            Self::Image(image.source())
        } else {
            Self::Text(format!(":{shortcode}:"))
        }
    }

    /// The emoji at `size`: a glyph's font size, an image's side.
    pub(crate) fn element(&self, size: Pixels) -> AnyElement {
        match self {
            Self::Glyph(glyph) => div()
                .text_size(size)
                .child(glyph.clone())
                .into_any_element(),
            // A Picture: animated custom emoji play, as in Slack.
            Self::Image(source) => div()
                .flex_none()
                .size(size)
                .child(crate::media::Picture::new(source.clone()).radius(px(2.)))
                .into_any_element(),
            Self::Text(text) => div().child(text.clone()).into_any_element(),
        }
    }
}

/// A `:shortcode:` (optionally followed by `:skin-tone-N:`) at the start of `rest`: its byte
/// length and the shortcode in Slack's `name::skin-tone-N` form. Only the name syntax is
/// checked; callers resolve it to a glyph or a custom emoji.
pub(crate) fn parse_shortcode(rest: &str) -> Option<(usize, String)> {
    let tail = rest.strip_prefix(':')?;
    let end = tail.find(':')?;
    let name = &tail[..end];
    let valid = !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '+' | '-'));
    if !valid {
        return None;
    }
    let len = end + 2;
    let after = &rest[len..];
    if let Some(tone) = after
        .strip_prefix(":skin-tone-")
        .and_then(|tone| tone.get(..2))
        .filter(|tone| tone.ends_with(':'))
    {
        return Some((
            len + ":skin-tone-".len() + 2,
            format!("{name}::skin-tone-{}", &tone[..1]),
        ));
    }
    Some((len, name.to_owned()))
}

/// `text` with known `:shortcode:`s replaced by their glyphs (plain_text with `emoji: true`).
pub(crate) fn replace_shortcodes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(':') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        match parse_shortcode(rest).and_then(|(len, code)| Some((len, display_glyph(&code)?))) {
            Some((len, glyph)) => {
                out.push_str(&glyph);
                rest = &rest[len..];
            }
            None => {
                out.push(':');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Slack's Frequently Used from the user's prefs: the `emoji_use` counts (a JSON object, sent
/// JSON-encoded in a string) summed per emoji over skin tones and aliases, most used first with
/// ties by name, at most [`FREQUENT_MAX`]. Names that are neither a standard emoji nor one of
/// the workspace's custom emoji (`is_custom`), say a deleted one, are dropped as Slack drops
/// them. Empty when the pref is absent.
pub(crate) fn frequent_from_prefs(prefs: &Json, is_custom: impl Fn(&str) -> bool) -> Vec<String> {
    let decoded;
    let uses = match prefs.get("emoji_use") {
        Some(Json::String(text)) => {
            decoded = serde_json::from_str::<Json>(text).unwrap_or_default();
            &decoded
        }
        Some(uses) => uses,
        None => return Vec::new(),
    };
    let mut counts: HashMap<&str, u64> = HashMap::new();
    for (name, count) in uses.as_object().into_iter().flatten() {
        let base = base_name(name);
        let name = lookup(base).map_or(base, |emoji| emoji.name);
        let Some(count) = count.as_u64() else {
            continue;
        };
        if glyph(name).is_some() || is_custom(name) {
            *counts.entry(name).or_default() += count;
        }
    }
    let mut ranked: Vec<(&str, u64)> = counts.into_iter().collect();
    ranked.sort_by(|(a, x), (b, y)| y.cmp(x).then_with(|| a.cmp(b)));
    ranked
        .into_iter()
        .take(FREQUENT_MAX)
        .map(|(name, _)| name.to_owned())
        .collect()
}

/// The user's skin tone from Slack's `preferred_skin_tone` pref (`""` or `"2"`..`"6"`): 1, the
/// default yellow, unless a Fitzpatrick tone is set.
pub(crate) fn skin_tone_from_prefs(prefs: &Json) -> u8 {
    prefs
        .get("preferred_skin_tone")
        .and_then(|tone| match tone {
            Json::String(text) => text.parse().ok(),
            tone => tone.as_u64().and_then(|tone| u8::try_from(tone).ok()),
        })
        .filter(|tone| (2..=6).contains(tone))
        .unwrap_or(1)
}

/// Top-level tab of the emoji / GIF picker (Slack's `Emoji` | `GIFs` switcher).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickerTab {
    Emoji,
    Gifs,
}

/// Curated Giphy mood/reaction presets shown only in offline/fixture mode (when signed into a
/// live workspace, the GIFs tab loads Slack's real `gif.featured` and `gif.search` endpoints).
const GIF_PRESETS: [(&str, &str, u32); 12] = [
    ("Celebrate", "🎉", 0x5B2C6F),
    ("Applause", "👏", 0x1B4F72),
    ("Thumbs up", "👍", 0x145A32),
    ("Laugh", "😂", 0x7D6608),
    ("Yes", "🙌", 0x0E6251),
    ("Thank you", "🙏", 0x641E16),
    ("Mind blown", "🤯", 0x4A235A),
    ("Shrug", "🤷", 0x2E4053),
    ("Popcorn", "🍿", 0x7E5109),
    ("Dancing", "💃", 0x78281F),
    ("High five", "✋", 0x1F618D),
    ("Facepalm", "🤦", 0x424949),
];

/// A picked emoji: the shortcode as Slack names it (tone suffix included) and its glyph;
/// `glyph` is `None` for workspace custom emoji, which only exist as images.
#[derive(Clone)]
pub struct EmojiChoice {
    pub name: String,
    pub glyph: Option<String>,
}

pub enum EmojiPickerEvent {
    Pick(EmojiChoice),
    /// A GIF tile from `gif.featured` / `gif.search` was clicked: send its URL.
    PickGif(String),
    /// Request featured GIFs (`""`) or search GIFs (`query`) from Slack's `gif.*` API.
    SearchGifs(String),
    /// Run `/giphy <query>` in fixture/fallback mode.
    RunGiphy(String),
    ToneChanged(u8),
}

/// One row of the picker's grid: a section title, or up to [`COLUMNS`] emoji.
enum Row {
    Title(&'static str),
    Cells(Vec<EmojiChoice>),
}

pub struct EmojiPicker {
    fixtures: Arc<Fixtures>,
    search: Entity<TextareaState>,
    /// Active top tab (`Emoji` or `GIFs`).
    tab: PickerTab,
    /// Whether the top `Emoji` | `GIFs` tab switcher is shown (true for composers, false for
    /// message reactions).
    show_gifs: bool,
    /// Lowercased search text for Emoji (spaces normalized to `_`).
    query: String,
    /// Raw trimmed search text for GIFs.
    gif_query: String,
    /// Live GIF results returned by Slack's `gif.featured` / `gif.search` endpoints.
    gif_results: Vec<GifItem>,
    /// Last query whose results are currently in `gif_results` (`None` before first load).
    gif_loaded_query: Option<String>,
    /// Preview URLs that should have their decoded frames & GPU textures evicted on the next
    /// render (when a new search replaces old results or hover moves off an animated GIF).
    stale_gif_urls: Vec<String>,
    /// Index of the GIF tile currently under the pointer (the only tile animating at once).
    hovered_gif: Option<usize>,
    /// Animated `preview_url` currently decoded for `hovered_gif`, evicted as soon as hover moves.
    active_anim_url: Option<String>,
    gif_grid: UniformListScrollHandle,
    gif_loading: bool,
    gif_debounce: Option<Task<()>>,
    /// Slack skin tone, 1 (default) to 6.
    tone: u8,
    tones_open: bool,
    /// Base shortcodes, most used first: the user's `emoji_use` pref or [`FREQUENT_SEED`], each
    /// pick moved to the front.
    frequent: Vec<String>,
    /// The grid as [`EmojiPicker::rebuild`] last laid it out.
    rows: Vec<Row>,
    /// The first row of each section, with the tab it belongs to.
    sections: Vec<(Category, usize)>,
    grid: UniformListScrollHandle,
    /// The tab last clicked, highlighted until the grid is scrolled by hand: the section it
    /// jumped to may sit too low in the grid to reach the top.
    jumped: Option<Category>,
    /// Cell under the pointer, previewed in the footer.
    hovered: Option<EmojiChoice>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<EmojiPickerEvent> for EmojiPicker {}

/// Evict a single GIF URL's decoded frames and Metal textures from GPUI's window & asset cache.
fn evict_gif_asset(url: &str, window: &mut Window, cx: &mut gpui_kit::gpui::App) {
    let source = ImageSource::from(url.to_owned());
    if let ImageSource::Resource(ref resource) = source {
        if let Some(Ok(data)) = window.get_asset::<gpui_kit::gpui::ImgResourceLoader>(resource, cx)
        {
            let _ = window.drop_image(data);
        }
    }
    source.remove_asset(cx);
}

impl EmojiPicker {
    pub fn new(
        fixtures: Arc<Fixtures>,
        window: &mut Window,
        cx: &mut gpui_kit::gpui::App,
    ) -> Entity<EmojiPicker> {
        cx.new(|cx| {
            let search = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Search all emoji")
                    .auto_grow(1, 1)
            });
            let subscriptions = vec![cx.subscribe_in(
                &search,
                window,
                |this: &mut EmojiPicker, search, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => {
                        let raw = search.read(cx).value().trim().to_owned();
                        match this.tab {
                            PickerTab::Emoji => {
                                let query = raw.to_lowercase().replace(' ', "_");
                                if query != this.query {
                                    this.query = query;
                                    this.rebuild();
                                    cx.notify();
                                }
                            }
                            PickerTab::Gifs => {
                                if raw != this.gif_query {
                                    this.gif_query = raw;
                                    this.request_gifs(true, cx);
                                    cx.notify();
                                }
                            }
                        }
                    }
                    InputEvent::PressEnter { .. } => match this.tab {
                        PickerTab::Emoji => {
                            if let Some(first) = this.first_cell().cloned() {
                                this.pick(first, cx);
                            }
                        }
                        PickerTab::Gifs => {
                            if let Some(first) = this.gif_results.first().cloned() {
                                cx.emit(EmojiPickerEvent::PickGif(first.url));
                            } else {
                                let query = this.gif_query.trim().to_owned();
                                if !query.is_empty() {
                                    cx.emit(EmojiPickerEvent::RunGiphy(query));
                                } else if let Some((preset, _, _)) = GIF_PRESETS.first() {
                                    cx.emit(EmojiPickerEvent::RunGiphy((*preset).to_owned()));
                                }
                            }
                        }
                    },
                    _ => {}
                },
            )];
            let frequent = if fixtures.frequent_emoji.is_empty() {
                FREQUENT_SEED
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect()
            } else {
                fixtures.frequent_emoji.clone()
            };
            let mut picker = EmojiPicker {
                tone: fixtures.skin_tone,
                fixtures,
                search,
                tab: PickerTab::Emoji,
                show_gifs: false,
                query: String::new(),
                gif_query: String::new(),
                gif_results: Vec::new(),
                gif_loaded_query: None,
                stale_gif_urls: Vec::new(),
                hovered_gif: None,
                active_anim_url: None,
                gif_grid: UniformListScrollHandle::new(),
                gif_loading: false,
                gif_debounce: None,
                tones_open: false,
                frequent,
                rows: Vec::new(),
                sections: Vec::new(),
                grid: UniformListScrollHandle::new(),
                jumped: None,
                hovered: None,
                _subscriptions: subscriptions,
            };
            picker.rebuild();
            picker
        })
    }

    /// Reset to the top of the grid with an empty, focused search on `tab`.
    pub fn open_tab(
        &mut self,
        tab: PickerTab,
        show_gifs: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_gifs = show_gifs;
        self.tab = if show_gifs { tab } else { PickerTab::Emoji };
        self.query.clear();
        self.gif_query.clear();
        self.tones_open = false;
        self.rebuild();
        let placeholder = match self.tab {
            PickerTab::Emoji => "Search all emoji",
            PickerTab::Gifs => "Search all GIFs on Giphy",
        };
        self.search.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx);
            state.set_value("", window, cx);
        });
        self.search.read(cx).focus_handle(cx).focus(window, cx);
        if self.tab == PickerTab::Gifs {
            self.request_gifs(false, cx);
        }
        cx.notify();
    }

    /// Receive GIF results from Slack's `gif.featured` / `gif.search` backend.
    pub fn set_gif_results(&mut self, query: &str, gifs: Vec<GifItem>, cx: &mut Context<Self>) {
        if query.trim() != self.gif_query.trim() {
            return;
        }
        self.hovered_gif = None;
        if let Some(anim) = self.active_anim_url.take() {
            self.stale_gif_urls.push(anim);
        }
        for old in self.gif_results.drain(..) {
            if !gifs.iter().any(|g| g.still_url == old.still_url) {
                self.stale_gif_urls.push(old.still_url);
            }
            if !gifs.iter().any(|g| g.preview_url == old.preview_url) {
                self.stale_gif_urls.push(old.preview_url);
            }
        }
        self.gif_loading = false;
        self.gif_loaded_query = Some(self.gif_query.trim().to_owned());
        self.gif_results = gifs;
        self.gif_grid.scroll_to_item_strict(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Evict all decoded GIF frames and Metal GPU textures when the picker closes or leaves
    /// the GIFs tab so memory returns to baseline and zero animation timers stay alive.
    pub fn release_gifs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.gif_debounce = None;
        self.gif_loading = false;
        self.gif_loaded_query = None;
        self.hovered_gif = None;
        if let Some(anim) = self.active_anim_url.take() {
            evict_gif_asset(&anim, window, cx);
        }
        for url in self.stale_gif_urls.drain(..) {
            evict_gif_asset(&url, window, cx);
        }
        for gif in self.gif_results.drain(..) {
            evict_gif_asset(&gif.still_url, window, cx);
            if gif.preview_url != gif.still_url {
                evict_gif_asset(&gif.preview_url, window, cx);
            }
        }
    }

    fn switch_tab(&mut self, tab: PickerTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        if self.tab == PickerTab::Gifs {
            self.release_gifs(window, cx);
        }
        self.open_tab(tab, self.show_gifs, window, cx);
    }

    fn request_gifs(&mut self, debounce: bool, cx: &mut Context<Self>) {
        if self.fixtures.sample {
            self.gif_loading = false;
            self.gif_debounce = None;
            return;
        }
        let query = self.gif_query.trim().to_owned();
        if self.gif_loaded_query.as_deref() == Some(query.as_str()) && !self.gif_results.is_empty()
        {
            self.gif_loading = false;
            self.gif_debounce = None;
            return;
        }
        self.gif_loading = true;
        if !debounce {
            self.gif_debounce = None;
            cx.emit(EmojiPickerEvent::SearchGifs(query));
            return;
        }
        self.gif_debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let _ = this.update(cx, |picker, cx| {
                if picker.gif_query.trim() == query {
                    cx.emit(EmojiPickerEvent::SearchGifs(query));
                }
            });
        }));
    }

    /// Restore locally persisted frequent emoji and/or skin tone at launch.
    pub fn restore(
        &mut self,
        frequent: Option<Vec<String>>,
        tone: Option<u8>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        if let Some(list) = frequent.filter(|list| !list.is_empty()) {
            self.frequent = list;
            changed = true;
        }
        if let Some(tone) = tone.filter(|t| (1..=6).contains(t)) {
            self.tone = tone;
            changed = true;
        }
        if changed {
            self.rebuild();
            cx.notify();
        }
    }

    pub fn frequent(&self) -> &[String] {
        &self.frequent
    }

    fn pick(&mut self, choice: EmojiChoice, cx: &mut Context<Self>) {
        let base = base_name(&choice.name).to_owned();
        self.frequent.retain(|name| *name != base);
        self.frequent.insert(0, base);
        self.frequent.truncate(FREQUENT_MAX);
        cx.emit(EmojiPickerEvent::Pick(choice));
    }

    /// Lay the grid out again, scrolled to the top: the search matches, or every section in
    /// Slack's order.
    fn rebuild(&mut self) {
        self.rows.clear();
        self.sections.clear();
        self.hovered = None;
        self.jumped = None;
        self.grid.scroll_to_item_strict(0, ScrollStrategy::Top);
        if !self.query.is_empty() {
            let found = self.search_results();
            self.push_section(Category::Frequent, "Search results", found);
            return;
        }
        let frequent = self
            .frequent
            .iter()
            .filter_map(|name| self.choice(name))
            .collect();
        self.push_section(Category::Frequent, "Frequently Used", frequent);
        let work = WORK.iter().filter_map(|name| self.choice(name)).collect();
        self.push_section(Category::Frequent, "Getting Work Done", work);
        for (category, title, list) in CATALOG {
            let cells = list
                .iter()
                .map(|emoji| self.catalog_choice(emoji))
                .collect();
            self.push_section(category, title, cells);
        }
        let custom = self.custom_choices().collect();
        self.push_section(Category::Custom, "Custom", custom);
    }

    /// Append a titled section, [`COLUMNS`] cells to a row; an empty one is left out.
    fn push_section(&mut self, category: Category, title: &'static str, cells: Vec<EmojiChoice>) {
        if cells.is_empty() {
            return;
        }
        self.sections.push((category, self.rows.len()));
        self.rows.push(Row::Title(title));
        let mut cells = cells.into_iter();
        loop {
            let row: Vec<EmojiChoice> = cells.by_ref().take(COLUMNS).collect();
            if row.is_empty() {
                break;
            }
            self.rows.push(Row::Cells(row));
        }
    }

    /// Catalog emoji, then custom emoji, whose name contains the search text.
    fn search_results(&self) -> Vec<EmojiChoice> {
        let query = self.query.as_str();
        CATALOG
            .iter()
            .flat_map(|(_, _, list)| list.iter())
            .filter(|emoji| emoji.name.contains(query))
            .map(|emoji| self.catalog_choice(emoji))
            .chain(
                self.custom_choices()
                    .filter(|choice| choice.name.contains(query)),
            )
            .collect()
    }

    fn catalog_choice(&self, emoji: &Emoji) -> EmojiChoice {
        let tone = if emoji.skin { self.tone } else { 1 };
        EmojiChoice {
            name: toned_name(emoji.name, tone),
            glyph: Some(toned(emoji, tone)),
        }
    }

    /// The cell for shortcode `name`: a catalog or standard emoji in the picker's skin tone, or
    /// a workspace custom emoji; `None` when it is neither (say, a deleted custom emoji).
    fn choice(&self, name: &str) -> Option<EmojiChoice> {
        if let Some(emoji) = lookup(name) {
            return Some(self.catalog_choice(emoji));
        }
        if let Some(emoji) = unicode_emoji(name) {
            let toned = skin_tone(self.tone).and_then(|skin| emoji.with_skin_tone(skin));
            return Some(EmojiChoice {
                name: match toned {
                    Some(_) => toned_name(name, self.tone),
                    None => name.to_owned(),
                },
                glyph: Some(toned.unwrap_or(emoji).as_str().to_owned()),
            });
        }
        self.fixtures.custom_emoji(name).map(|_| EmojiChoice {
            name: name.to_owned(),
            glyph: None,
        })
    }

    fn custom_choices(&self) -> impl Iterator<Item = EmojiChoice> + '_ {
        self.fixtures.custom_emoji.keys().map(|name| EmojiChoice {
            name: name.clone(),
            glyph: None,
        })
    }

    fn first_cell(&self) -> Option<&EmojiChoice> {
        self.rows.iter().find_map(|row| match row {
            Row::Cells(cells) => cells.first(),
            Row::Title(_) => None,
        })
    }

    fn cell_at(&self, row: usize, col: usize) -> Option<&EmojiChoice> {
        match self.rows.get(row)? {
            Row::Cells(cells) => cells.get(col),
            Row::Title(_) => None,
        }
    }

    /// Scroll the grid to `category`'s section, clearing a search first.
    fn jump(&mut self, category: Category, window: &mut Window, cx: &mut Context<Self>) {
        if !self.query.is_empty() {
            self.query.clear();
            self.rebuild();
            self.search
                .update(cx, |state, cx| state.set_value("", window, cx));
        }
        if let Some(&(_, row)) = self
            .sections
            .iter()
            .find(|(section, _)| *section == category)
        {
            self.grid.scroll_to_item_strict(row, ScrollStrategy::Top);
            self.jumped = Some(category);
        }
        cx.notify();
    }

    /// The tab to highlight: search while searching, the tab last clicked, else the one whose
    /// section is at the top of the grid.
    fn active_tab(&self) -> Category {
        if !self.query.is_empty() {
            return Category::Frequent;
        }
        if let Some(category) = self.jumped {
            return category;
        }
        let top = {
            let grid = self.grid.0.borrow();
            grid.deferred_scroll_to_item.map_or_else(
                || (-grid.base_handle.offset().y / px(ROW)) as usize,
                |deferred| deferred.item_index,
            )
        };
        self.sections
            .iter()
            .take_while(|(_, row)| *row <= top)
            .last()
            .map_or(Category::Frequent, |(category, _)| *category)
    }

    /// The grid rows in `range`, for the lazily drawn list.
    fn render_rows(&self, range: Range<usize>, cx: &Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (foreground, hover) = (theme.foreground, theme.secondary_hover);
        range
            .filter_map(|ix| match self.rows.get(ix)? {
                Row::Title(title) => Some(
                    div()
                        .h(px(ROW))
                        .flex()
                        .items_center()
                        .px_1()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(foreground)
                        .child(*title)
                        .into_any_element(),
                ),
                Row::Cells(cells) => Some(
                    h_flex()
                        .id(("emoji-row", ix))
                        .h(px(ROW))
                        .children(
                            cells
                                .iter()
                                .enumerate()
                                .map(|(col, choice)| self.cell(ix, col, choice, hover, cx)),
                        )
                        .into_any_element(),
                ),
            })
            .collect()
    }

    fn cell(
        &self,
        row: usize,
        col: usize,
        choice: &EmojiChoice,
        hover: Hsla,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let face = match &choice.glyph {
            Some(glyph) => div()
                .text_size(px(GLYPH))
                .child(glyph.clone())
                .into_any_element(),
            None => emoji_element(&choice.name, &self.fixtures, px(GLYPH)),
        };
        div()
            .id(("emoji-cell", col))
            .w(px(CELL))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .hover(|cell| cell.bg(hover))
            .child(face)
            .on_hover(cx.listener(move |this, inside: &bool, _, cx| {
                if *inside {
                    this.hovered = this.cell_at(row, col).cloned();
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(choice) = this.cell_at(row, col).cloned() {
                    this.pick(choice, cx);
                }
            }))
    }
    /// Virtualized 3-column GIF rows: each tile renders its lightweight single-frame
    /// `still_url` (`tinygifpreview`, ~20 KB / 160 KB decoded, 0% idle CPU), and only the
    /// hovered tile overlays its animated `preview_url` (`tinygif`), evicting the previous
    /// animated texture as soon as hover moves so memory stays bounded.
    fn render_gif_rows(&self, range: Range<usize>, cx: &Context<Self>) -> Vec<AnyElement> {
        const GIF_COLS: usize = 3;
        let secondary = cx.theme().secondary;
        let tile = px(100.);
        range
            .filter_map(|row_ix| {
                let start = row_ix * GIF_COLS;
                let slice = self.gif_results.get(start..)?;
                let items = &slice[..slice.len().min(GIF_COLS)];
                Some(
                    h_flex()
                        .id(("gif-row", row_ix))
                        .h(px(108.))
                        .pb_2()
                        .gap_2()
                        .children(items.iter().enumerate().map(|(col, gif)| {
                            let idx = start + col;
                            let url = gif.url.clone();
                            let title = SharedString::from(gif.title.clone());
                            let is_hovered = self.hovered_gif == Some(idx);
                            let anim_url = gif.preview_url.clone();
                            div()
                                .id(("gif-item", idx))
                                .relative()
                                .w(tile)
                                .h(tile)
                                .rounded_md()
                                .overflow_hidden()
                                .bg(secondary)
                                .cursor_pointer()
                                .hover(|el| el.opacity(0.92))
                                .when(!gif.title.is_empty(), |el| {
                                    el.tooltip(move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(title.clone())
                                            .build(window, cx)
                                    })
                                })
                                .child(
                                    img(gif.still_url.clone())
                                        .size_full()
                                        .object_fit(ObjectFit::Cover)
                                        .rounded(px(6.)),
                                )
                                .when(is_hovered && gif.preview_url != gif.still_url, |el| {
                                    el.child(
                                        div().absolute().inset_0().child(
                                            crate::media::Picture::new(gif.preview_url.clone())
                                                .fit(ObjectFit::Cover)
                                                .radius(px(6.)),
                                        ),
                                    )
                                })
                                .on_hover(cx.listener(move |this, inside: &bool, _, cx| {
                                    if *inside {
                                        if this.hovered_gif != Some(idx) {
                                            if let Some(prev) =
                                                this.active_anim_url.replace(anim_url.clone())
                                            {
                                                if prev != anim_url {
                                                    this.stale_gif_urls.push(prev);
                                                }
                                            }
                                            this.hovered_gif = Some(idx);
                                            cx.notify();
                                        }
                                    } else if this.hovered_gif == Some(idx) {
                                        this.hovered_gif = None;
                                        if let Some(prev) = this.active_anim_url.take() {
                                            this.stale_gif_urls.push(prev);
                                        }
                                        cx.notify();
                                    }
                                }))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(EmojiPickerEvent::PickGif(url.clone()));
                                }))
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// The 3-column GIFs tab content, matching Slack's `"Search all GIFs on Giphy"` grid.
    fn render_gifs_body(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (border, secondary, hover, foreground, muted) = (
            theme.border,
            theme.secondary,
            theme.secondary_hover,
            theme.foreground,
            theme.muted_foreground,
        );
        let tile_w = px(100.);
        let tile_h = px(100.);
        let body_h = px(GRID_HEIGHT + 86.);

        if self.gif_loading {
            return div()
                .id("gif-grid-loading")
                .h(body_h)
                .px_4()
                .pt_3()
                .pb_3()
                .flex()
                .flex_wrap()
                .content_start()
                .gap_2()
                .children((0usize..9).map(|ix| {
                    div()
                        .id(("gif-skel", ix))
                        .w(tile_w)
                        .h(tile_h)
                        .rounded_md()
                        .bg(secondary)
                }))
                .into_any_element();
        }

        if !self.gif_results.is_empty() {
            let row_count = self.gif_results.len().div_ceil(3);
            return uniform_list(
                "gif-grid",
                row_count,
                cx.processor(|this, range: Range<usize>, _, cx| this.render_gif_rows(range, cx)),
            )
            .h(body_h)
            .px_4()
            .pt_3()
            .pb_2()
            .track_scroll(&self.gif_grid)
            .into_any_element();
        }

        if !self.fixtures.sample && self.gif_loaded_query.is_some() {
            return div()
                .id("gif-grid-empty")
                .h(body_h)
                .p_4()
                .text_sm()
                .text_color(muted)
                .child(format!("No GIFs match \u{201C}{}\u{201D}", self.gif_query))
                .into_any_element();
        }

        let needle = self.gif_query.trim().to_lowercase();
        let presets: Vec<_> = GIF_PRESETS
            .iter()
            .copied()
            .filter(|(label, _, _)| needle.is_empty() || label.to_lowercase().contains(&needle))
            .collect();
        let query_banner = (!needle.is_empty()).then(|| {
            let query = self.gif_query.trim().to_owned();
            let display = SharedString::from(format!("Send /giphy \u{201C}{query}\u{201D}"));
            h_flex()
                .id("gif-run-query")
                .w_full()
                .px_3()
                .py_2()
                .mb_2()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(secondary)
                .cursor_pointer()
                .hover(|el| el.bg(hover))
                .items_center()
                .justify_between()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::Sticker)
                                .size(px(15.))
                                .text_color(foreground),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(foreground)
                                .child(display),
                        ),
                )
                .child(div().text_xs().text_color(muted).child("Return"))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(EmojiPickerEvent::RunGiphy(query.clone()));
                }))
        });

        v_flex()
            .id("gif-grid-presets")
            .h(body_h)
            .px_4()
            .pt_3()
            .pb_3()
            .overflow_y_scroll()
            .children(query_banner)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .children(
                        presets
                            .into_iter()
                            .enumerate()
                            .map(|(ix, (label, glyph, tint))| {
                                let bg = Hsla::from(rgb(tint)).opacity(if theme.mode.is_dark() {
                                    0.85
                                } else {
                                    0.9
                                });
                                let fg = Hsla::from(rgb(0xFF_FF_FF));
                                v_flex()
                                    .id(("gif-preset", ix))
                                    .w(tile_w)
                                    .h(px(74.))
                                    .rounded_md()
                                    .bg(bg)
                                    .cursor_pointer()
                                    .hover(|el| el.opacity(0.88))
                                    .items_center()
                                    .justify_center()
                                    .gap_1()
                                    .child(div().text_size(px(22.)).child(glyph))
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(fg)
                                            .child(label),
                                    )
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(EmojiPickerEvent::RunGiphy(label.to_owned()));
                                    }))
                            }),
                    ),
            )
            .into_any_element()
    }
}

impl Render for EmojiPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.stale_gif_urls.is_empty() {
            let stale = std::mem::take(&mut self.stale_gif_urls);
            window.defer(cx, move |window, cx| {
                for url in stale {
                    evict_gif_asset(&url, window, cx);
                }
            });
        }
        let theme = cx.theme();
        let border = theme.border;
        let input_border = theme.input;
        let ring = theme.ring;
        let background = theme.background;
        let footer_bg = theme.muted;
        let muted = theme.muted_foreground;
        let foreground = theme.foreground;
        let hover = theme.secondary_hover;
        let active_bg = theme.secondary_active;

        // Top `Emoji` | `GIFs` switcher matching official Slack's picker header.
        let top_tabs = self.show_gifs.then(|| {
            let emoji_active = self.tab == PickerTab::Emoji;
            let gifs_active = self.tab == PickerTab::Gifs;
            let transparent = Hsla::transparent_black();
            let emoji_accent = Hsla::from(rgb(0xEC_B2_2E));
            h_flex()
                .w_full()
                .h(px(44.))
                .border_b_1()
                .border_color(border)
                .child(
                    h_flex()
                        .id("picker-tab-emoji")
                        .flex_1()
                        .h_full()
                        .items_center()
                        .justify_center()
                        .gap_1p5()
                        .cursor_pointer()
                        .border_b_2()
                        .border_color(if emoji_active {
                            foreground
                        } else {
                            transparent
                        })
                        .when(!emoji_active, |el| el.hover(|el| el.bg(hover.opacity(0.4))))
                        .child(
                            Icon::new(IconName::FaceSlightlySmiling)
                                .size(px(16.))
                                .text_color(if emoji_active { emoji_accent } else { muted }),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(if emoji_active {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(if emoji_active { foreground } else { muted })
                                .child("Emoji"),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.switch_tab(PickerTab::Emoji, window, cx);
                        })),
                )
                .child(
                    h_flex()
                        .id("picker-tab-gifs")
                        .flex_1()
                        .h_full()
                        .items_center()
                        .justify_center()
                        .gap_1p5()
                        .cursor_pointer()
                        .border_b_2()
                        .border_color(if gifs_active { foreground } else { transparent })
                        .when(!gifs_active, |el| el.hover(|el| el.bg(hover.opacity(0.4))))
                        .child(
                            Icon::new(IconName::Sticker)
                                .size(px(16.))
                                .text_color(if gifs_active { foreground } else { muted }),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(if gifs_active {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(if gifs_active { foreground } else { muted })
                                .child("GIFs"),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.switch_tab(PickerTab::Gifs, window, cx);
                        })),
                )
        });

        // The search field in Slack's focus ring: the accent border inside a soft halo.
        let focused = self.search.read(cx).focus_handle(cx).is_focused(window);
        let search = h_flex()
            .mx_4()
            .mt_3()
            .h(px(36.))
            .items_center()
            .rounded_lg()
            .border_1()
            .border_color(if focused { ring } else { input_border })
            .when(focused, |field| {
                field.shadow(vec![BoxShadow {
                    color: ring.opacity(0.3),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(3.),
                    inset: false,
                }])
            })
            .bg(background)
            .child(
                div().flex_1().min_w_0().text_sm().child(
                    Textarea::new(&self.search)
                        .appearance(false)
                        .bordered(false),
                ),
            );

        if self.tab == PickerTab::Gifs {
            return v_flex()
                .id("emoji-picker")
                .w(px(PICKER_WIDTH))
                .rounded_lg()
                .border_1()
                .border_color(border)
                .bg(background)
                .shadow_lg()
                .text_color(foreground)
                .children(top_tabs)
                .child(search)
                .child(self.render_gifs_body(cx));
        }

        // Slack's category row: outline icons, the section in view (or the one clicked) boxed.
        let active = self.active_tab();
        let has_custom = !self.fixtures.custom_emoji.is_empty();
        let tabs = h_flex().justify_between().px_3().pt_3().children(
            TABS.iter()
                .filter(|(category, _)| *category != Category::Custom || has_custom)
                .enumerate()
                .map(|(ix, &(category, icon))| {
                    let current = category == active;
                    div()
                        .id(("emoji-tab", ix))
                        .w(px(32.))
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .when(current, |tab| tab.bg(active_bg))
                        .when(!current, |tab| tab.hover(|tab| tab.bg(hover)))
                        .child(Icon::new(icon).size(px(18.)).text_color(if current {
                            foreground
                        } else {
                            muted
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.jump(category, window, cx);
                        }))
                }),
        );

        let grid = if self.rows.is_empty() {
            div()
                .mt_1p5()
                .h(px(GRID_HEIGHT))
                .p_4()
                .text_sm()
                .text_color(muted)
                .child(format!("No emoji match \u{201C}{}\u{201D}", self.query))
                .into_any_element()
        } else {
            uniform_list(
                "emoji-grid",
                self.rows.len(),
                cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
            )
            .mt_1p5()
            .h(px(GRID_HEIGHT))
            .px_4()
            .track_scroll(&self.grid)
            .on_scroll(cx.listener(|this, _, _, cx| {
                // Scrolling by hand hands the highlight back to the section in view.
                this.jumped = None;
                cx.notify();
            }))
            .into_any_element()
        };

        // Footer: the hovered emoji on the left, Slack's skin-tone button (or its six choices)
        // on the right.
        let preview = h_flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_2()
            .when_some(self.hovered.as_ref(), |preview, choice| {
                preview
                    .child(match &choice.glyph {
                        Some(glyph) => div()
                            .text_size(px(28.))
                            .child(glyph.clone())
                            .into_any_element(),
                        None => emoji_element(&choice.name, &self.fixtures, px(28.)),
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(foreground)
                            .child(format!(":{}:", choice.name)),
                    )
            });
        let hand = lookup("raised_hand");
        let skin = if self.tones_open {
            h_flex()
                .gap_0p5()
                .children((1..=6u8).map(|tone| {
                    div()
                        .id(("emoji-tone", usize::from(tone)))
                        .size(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .text_size(px(18.))
                        .when(tone == self.tone, |choice| {
                            choice.border_1().border_color(ring)
                        })
                        .hover(|choice| choice.bg(hover))
                        .children(hand.map(|hand| toned(hand, tone)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.tone = tone;
                            this.tones_open = false;
                            this.rebuild();
                            cx.emit(EmojiPickerEvent::ToneChanged(tone));
                            cx.notify();
                        }))
                }))
                .into_any_element()
        } else {
            h_flex()
                .id("emoji-skin-tone")
                .flex_none()
                .h(px(32.))
                .px_2p5()
                .gap_1p5()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .hover(|button| button.bg(hover))
                .child(
                    div()
                        .text_size(px(18.))
                        .children(hand.map(|hand| toned(hand, self.tone))),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(muted)
                        .child("Skin Tone"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.tones_open = true;
                    cx.notify();
                }))
                .into_any_element()
        };
        let footer = h_flex()
            .h(px(56.))
            .px_4()
            .gap_2()
            .items_center()
            .rounded_b_lg()
            .bg(footer_bg)
            .child(preview)
            .child(skin);

        v_flex()
            .id("emoji-picker")
            .w(px(PICKER_WIDTH))
            .rounded_lg()
            .border_1()
            .border_color(border)
            .bg(background)
            .shadow_lg()
            .text_color(foreground)
            .children(top_tabs)
            .child(tabs)
            .child(search)
            .child(grid)
            .child(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockkit::serde_json::json;

    #[test]
    fn frequent_emoji_follow_the_emoji_use_counts() {
        // Slack sends the pref JSON-encoded; skin tones and aliases count toward one emoji, and
        // a name that resolves to nothing (a deleted custom emoji) drops out.
        let uses = json!({
            "joy": 3,
            "+1": 2,
            "thumbsup::skin-tone-3": 2,
            "tada": 3,
            "shipit": 1,
            "partying_face": 1,
            "deleted_custom": 9,
        });
        let prefs = json!({ "emoji_use": uses.to_string() });
        assert_eq!(
            frequent_from_prefs(&prefs, |name| name == "shipit"),
            ["thumbsup", "joy", "tada", "partying_face", "shipit"]
        );
        let many: serde_json::Map<String, Json> = PEOPLE
            .iter()
            .map(|emoji| (emoji.name.to_owned(), json!(1)))
            .collect();
        let prefs = json!({ "emoji_use": many });
        assert_eq!(frequent_from_prefs(&prefs, |_| false).len(), FREQUENT_MAX);
        assert!(frequent_from_prefs(&json!({}), |_| true).is_empty());
    }

    #[test]
    fn skin_tone_follows_the_preferred_tone() {
        assert_eq!(
            skin_tone_from_prefs(&json!({ "preferred_skin_tone": "" })),
            1
        );
        assert_eq!(
            skin_tone_from_prefs(&json!({ "preferred_skin_tone": "4" })),
            4
        );
        assert_eq!(
            skin_tone_from_prefs(&json!({ "preferred_skin_tone": 6 })),
            6
        );
        assert_eq!(
            skin_tone_from_prefs(&json!({ "preferred_skin_tone": "9" })),
            1
        );
    }

    #[test]
    fn getting_work_done_resolves_every_emoji() {
        let missing: Vec<_> = WORK.iter().filter(|name| glyph(name).is_none()).collect();
        assert!(missing.is_empty(), "{missing:?}");
    }
}
