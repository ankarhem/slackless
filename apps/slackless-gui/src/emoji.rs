//! Emoji: the shortcode catalog every view resolves through (reaction chips, mrkdwn and
//! rich-text `:shortcode:`s, statuses), the workspace's custom emoji (fixture images), and the
//! searchable emoji picker overlay the root mounts for the composer and add-reaction buttons.
//!
//! The picker is one long-lived entity: a search field, a skin-tone selector, category tabs
//! (frequently used first, custom last) over a fixed-size scrolling grid, and a hover preview
//! footer. A pick emits [`EmojiPickerEvent::Pick`]; the root decides whether it inserts into a
//! composer or adds a reaction. Frequently-used emoji are tracked in memory (most recent first).
//!
//! Skin tones follow Slack's wire format: a toned pick is named `wave::skin-tone-3`, and the
//! glyph carries the matching Fitzpatrick modifier (U+1F3FB..U+1F3FF).
//!
//! Design tokens: colors via `cx.theme()`; the grid metrics are the only local constants.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, Focusable as _, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, img, px,
};

use crate::fixtures::Fixtures;

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
    Activity,
    Travel,
    Objects,
    Symbols,
    Custom,
}

/// Picker tabs in order: category, section label, tab glyph.
const TABS: [(Category, &str, &str); 9] = [
    (Category::Frequent, "Frequently used", "\u{1F552}"),
    (Category::People, "Smileys & people", "\u{1F600}"),
    (Category::Nature, "Animals & nature", "\u{1F43B}"),
    (Category::Food, "Food & drink", "\u{1F354}"),
    (Category::Activity, "Activities", "\u{26BD}"),
    (Category::Travel, "Travel & places", "\u{1F697}"),
    (Category::Objects, "Objects", "\u{1F4A1}"),
    (Category::Symbols, "Symbols", "\u{2764}\u{FE0F}"),
    (Category::Custom, "Custom", "\u{2728}"),
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

const CATALOG: [(Category, &[Emoji]); 7] = [
    (Category::People, PEOPLE),
    (Category::Nature, NATURE),
    (Category::Food, FOOD),
    (Category::Activity, ACTIVITY),
    (Category::Travel, TRAVEL),
    (Category::Objects, OBJECTS),
    (Category::Symbols, SYMBOLS),
];

/// Alternate shortcodes Slack accepts for catalog emoji.
const ALIASES: &[(&str, &str)] = &[
    ("+1", "thumbsup"),
    ("-1", "thumbsdown"),
    ("simple_smile", "slightly_smiling_face"),
    ("hand", "raised_hand"),
    ("facepalm", "face_palm"),
];

/// The frequently-used row before the user has picked anything.
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
const FREQUENT_MAX: usize = 18;

/// Grid metrics: square cell, columns, scroll height; the width adds the `p_2` padding.
const CELL: f32 = 34.;
const COLUMNS: usize = 9;
const GRID_HEIGHT: f32 = 238.;
const PICKER_WIDTH: f32 = COLUMNS as f32 * CELL + 16.;

fn lookup(name: &str) -> Option<&'static Emoji> {
    let name = ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name, |(_, target)| *target);
    CATALOG
        .iter()
        .flat_map(|(_, list)| list.iter())
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
    let skin = match tone {
        2 => Some(emojis::SkinTone::Light),
        3 => Some(emojis::SkinTone::MediumLight),
        4 => Some(emojis::SkinTone::Medium),
        5 => Some(emojis::SkinTone::MediumDark),
        6 => Some(emojis::SkinTone::Dark),
        _ => None,
    };
    let emoji = skin
        .and_then(|skin| emoji.with_skin_tone(skin))
        .unwrap_or(emoji);
    Some(emoji.as_str().to_owned())
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
    if let Some(glyph) = display_glyph(shortcode) {
        return div().text_size(size).child(glyph).into_any_element();
    }
    if let Some(image) = fixtures.custom_emoji(base_name(shortcode)) {
        return img(image.source())
            .size(size)
            .rounded_sm()
            .into_any_element();
    }
    div().child(format!(":{shortcode}:")).into_any_element()
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

/// A picked emoji: the shortcode as Slack names it (tone suffix included) and its glyph;
/// `glyph` is `None` for workspace custom emoji, which only exist as images.
#[derive(Clone)]
pub struct EmojiChoice {
    pub name: String,
    pub glyph: Option<String>,
}

pub enum EmojiPickerEvent {
    Pick(EmojiChoice),
}

pub struct EmojiPicker {
    fixtures: Arc<Fixtures>,
    search: Entity<TextareaState>,
    /// Lowercased search text; non-empty searches every category.
    query: String,
    category: Category,
    /// Slack skin tone, 1 (default) to 6.
    tone: u8,
    tones_open: bool,
    /// Base shortcodes, most recently picked first.
    frequent: Vec<String>,
    /// Cell under the pointer, previewed in the footer.
    hovered: Option<EmojiChoice>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<EmojiPickerEvent> for EmojiPicker {}

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
                        this.query = search
                            .read(cx)
                            .value()
                            .trim()
                            .to_lowercase()
                            .replace(' ', "_");
                        this.hovered = None;
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => {
                        if let Some(first) = this.cells().into_iter().next() {
                            this.pick(first, cx);
                        }
                    }
                    _ => {}
                },
            )];
            EmojiPicker {
                fixtures,
                search,
                query: String::new(),
                category: Category::Frequent,
                tone: 1,
                tones_open: false,
                frequent: FREQUENT_SEED
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect(),
                hovered: None,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Reset to the frequently-used tab with an empty, focused search.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.query.clear();
        self.category = Category::Frequent;
        self.tones_open = false;
        self.hovered = None;
        self.search
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.search.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn pick(&mut self, choice: EmojiChoice, cx: &mut Context<Self>) {
        let base = base_name(&choice.name).to_owned();
        self.frequent.retain(|name| *name != base);
        self.frequent.insert(0, base);
        self.frequent.truncate(FREQUENT_MAX);
        cx.emit(EmojiPickerEvent::Pick(choice));
    }

    fn catalog_choice(&self, emoji: &Emoji) -> EmojiChoice {
        let tone = if emoji.skin { self.tone } else { 1 };
        EmojiChoice {
            name: toned_name(emoji.name, tone),
            glyph: Some(toned(emoji, tone)),
        }
    }

    fn custom_choices(&self) -> impl Iterator<Item = EmojiChoice> + '_ {
        self.fixtures.custom_emoji.keys().map(|name| EmojiChoice {
            name: name.clone(),
            glyph: None,
        })
    }

    /// The cells the grid shows: search results across everything, or the active tab.
    fn cells(&self) -> Vec<EmojiChoice> {
        if !self.query.is_empty() {
            let query = self.query.as_str();
            let mut found: Vec<EmojiChoice> = CATALOG
                .iter()
                .flat_map(|(_, list)| list.iter())
                .filter(|emoji| emoji.name.contains(query))
                .map(|emoji| self.catalog_choice(emoji))
                .collect();
            found.extend(
                self.custom_choices()
                    .filter(|choice| choice.name.contains(query)),
            );
            return found;
        }
        match self.category {
            Category::Frequent => self
                .frequent
                .iter()
                .filter_map(|name| match lookup(name) {
                    Some(emoji) => Some(self.catalog_choice(emoji)),
                    None => self.fixtures.custom_emoji(name).map(|_| EmojiChoice {
                        name: name.clone(),
                        glyph: None,
                    }),
                })
                .collect(),
            Category::Custom => self.custom_choices().collect(),
            category => CATALOG
                .iter()
                .filter(|(group, _)| *group == category)
                .flat_map(|(_, list)| list.iter())
                .map(|emoji| self.catalog_choice(emoji))
                .collect(),
        }
    }

    fn cell(&self, choice: EmojiChoice, cx: &mut Context<Self>) -> impl IntoElement {
        let hover = cx.theme().secondary_hover;
        let face = match &choice.glyph {
            Some(glyph) => div()
                .text_size(px(22.))
                .child(glyph.clone())
                .into_any_element(),
            None => emoji_element(&choice.name, &self.fixtures, px(22.)),
        };
        let hovered = choice.clone();
        div()
            .id(SharedString::from(format!("emoji-cell-{}", choice.name)))
            .size(px(CELL))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .hover(|cell| cell.bg(hover))
            .child(face)
            .on_hover(cx.listener(move |this, inside: &bool, _, cx| {
                if *inside {
                    this.hovered = Some(hovered.clone());
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.pick(choice.clone(), cx)))
    }
}

impl Render for EmojiPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.border;
        let input_border = theme.input;
        let background = theme.background;
        let muted = theme.muted_foreground;
        let foreground = theme.foreground;
        let link = theme.link;
        let hover = theme.secondary_hover;
        let active_bg = theme.secondary_active;

        let raised_hand = lookup("raised_hand").map(|hand| toned(hand, self.tone));
        let search = h_flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_1p5()
            .px_2()
            .py_0p5()
            .rounded_md()
            .border_1()
            .border_color(input_border)
            .bg(background)
            .child(Icon::new(IconName::Search).text_color(muted).size(px(14.)))
            .child(
                div().flex_1().min_w_0().text_sm().child(
                    Textarea::new(&self.search)
                        .appearance(false)
                        .bordered(false),
                ),
            );
        let tone_button = div()
            .id("emoji-tone")
            .flex_none()
            .size(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .text_size(px(18.))
            .hover(|el| el.bg(hover))
            .children(raised_hand)
            .on_click(cx.listener(|this, _, _, cx| {
                this.tones_open = !this.tones_open;
                cx.notify();
            }));
        let header = h_flex()
            .items_center()
            .gap_2()
            .p_2()
            .child(search)
            .child(tone_button);

        let tones = self.tones_open.then(|| {
            let hand = lookup("raised_hand");
            h_flex()
                .items_center()
                .gap_1()
                .px_2()
                .pb_2()
                .child(div().text_xs().text_color(muted).mr_1().child("Skin tone"))
                .children((1..=6u8).map(|tone| {
                    let selected = tone == self.tone;
                    div()
                        .id(SharedString::from(format!("emoji-tone-{tone}")))
                        .size(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .text_size(px(18.))
                        .when(selected, |cell| cell.border_1().border_color(link))
                        .hover(|cell| cell.bg(hover))
                        .children(hand.map(|hand| toned(hand, tone)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.tone = tone;
                            this.tones_open = false;
                            cx.notify();
                        }))
                }))
        });

        let searching = !self.query.is_empty();
        let tabs = h_flex()
            .items_center()
            .justify_between()
            .px_2()
            .border_b_1()
            .border_color(border)
            .children(TABS.iter().map(|(category, label, glyph)| {
                let active = !searching && *category == self.category;
                let category = *category;
                div()
                    .id(SharedString::from(format!("emoji-tab-{label}")))
                    .size(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_size(px(16.))
                    .when(active, |tab| {
                        tab.border_b_2().border_color(link).bg(active_bg)
                    })
                    .when(!active, |tab| tab.opacity(0.7))
                    .hover(|tab| tab.opacity(1.))
                    .child(*glyph)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.category = category;
                        this.query.clear();
                        cx.notify();
                    }))
            }));

        let section = if searching {
            "Search results".to_owned()
        } else {
            TABS.iter()
                .find(|(category, _, _)| *category == self.category)
                .map(|(_, label, _)| (*label).to_owned())
                .unwrap_or_default()
        };
        let cells = self.cells();
        let empty = cells.is_empty();
        let grid = div().flex().flex_wrap().children(
            cells
                .into_iter()
                .map(|choice| self.cell(choice, cx))
                .collect::<Vec<_>>(),
        );
        let body = v_flex()
            .id("emoji-grid")
            .h(px(GRID_HEIGHT))
            .overflow_y_scroll()
            .px_2()
            .pb_1()
            .child(
                div()
                    .py_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child(section),
            )
            .when(empty, |body| {
                body.child(
                    div()
                        .py_4()
                        .text_sm()
                        .text_color(muted)
                        .child(format!("No emoji match \u{201C}{}\u{201D}", self.query)),
                )
            })
            .when(!empty, |body| body.child(grid));

        let footer = h_flex()
            .items_center()
            .gap_2()
            .h(px(48.))
            .px_3()
            .border_t_1()
            .border_color(border)
            .map(|footer| match &self.hovered {
                Some(choice) => footer
                    .child(match &choice.glyph {
                        Some(glyph) => div()
                            .text_size(px(28.))
                            .child(glyph.clone())
                            .into_any_element(),
                        None => emoji_element(&choice.name, &self.fixtures, px(28.)),
                    })
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(foreground)
                            .child(format!(":{}:", choice.name)),
                    ),
                None => footer.child(div().text_sm().text_color(muted).child("Pick an emoji")),
            });

        v_flex()
            .id("emoji-picker")
            .w(px(PICKER_WIDTH))
            .rounded_lg()
            .border_1()
            .border_color(border)
            .bg(background)
            .shadow_lg()
            .text_color(foreground)
            .child(header)
            .children(tones)
            .child(tabs)
            .child(body)
            .child(footer)
    }
}
