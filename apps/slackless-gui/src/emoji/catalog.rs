//! Static Slack emoji catalog, category tabs, aliases, and fallback Unicode lookup.

use std::collections::HashMap;
use std::sync::OnceLock;

use gpui_kit::assets::IconName;

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
pub(super) enum Category {
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
pub(super) const TABS: [(Category, IconName); 10] = [
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

pub(super) const PEOPLE: &[Emoji] = &[
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

pub(super) const NATURE: &[Emoji] = &[
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

pub(super) const FOOD: &[Emoji] = &[
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

pub(super) const ACTIVITY: &[Emoji] = &[
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

pub(super) const TRAVEL: &[Emoji] = &[
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

pub(super) const OBJECTS: &[Emoji] = &[
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

pub(super) const SYMBOLS: &[Emoji] = &[
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
pub(super) const FLAGS: &[Emoji] = &[
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
pub(super) const CATALOG: [(Category, &str, &[Emoji]); 8] = [
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
pub(super) const WORK: [&str; 26] = [
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
pub(super) const ALIASES: &[(&str, &str)] = &[
    ("+1", "thumbsup"),
    ("-1", "thumbsdown"),
    ("simple_smile", "slightly_smiling_face"),
    ("hand", "raised_hand"),
    ("facepalm", "face_palm"),
];

/// Frequently Used when the user's prefs carry no `emoji_use` (the sample workspace).
pub(super) const FREQUENT_SEED: &[&str] = &[
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
pub(super) const FREQUENT_MAX: usize = 36;

pub(super) fn lookup(name: &str) -> Option<&'static Emoji> {
    static INDEX: OnceLock<HashMap<&'static str, &'static Emoji>> = OnceLock::new();
    let map = INDEX.get_or_init(|| {
        let mut map = HashMap::new();
        for (_, _, list) in CATALOG {
            for emoji in list.iter() {
                map.insert(emoji.name, emoji);
            }
        }
        for &(alias, target) in ALIASES {
            if let Some(&emoji) = map.get(target) {
                map.insert(alias, emoji);
            }
        }
        map
    });
    map.get(name).copied()
}

/// Slack-specific shortcode names that differ from the GitHub/CLDR shortcodes in `emojis`.
fn slack_alias(name: &str) -> Option<&'static str> {
    Some(match name {
        "spiral_calendar_pad" => "spiral_calendar",
        "spiral_note_pad" => "spiral_notepad",
        "knife_fork_plate" => "plate_with_cutlery",
        "the_horns" | "sign_of_the_horns" => "metal",
        "spock-hand" | "spock_hand" => "vulcan_salute",
        "star-struck" => "star_struck",
        "heavy_check_mark" => "heavy_check_mark",
        "mostly_sunny" => "sun_behind_small_cloud",
        "barely_sunny" => "sun_behind_large_cloud",
        "partly_sunny_rain" => "sun_behind_rain_cloud",
        "lightning_cloud" => "cloud_with_lightning",
        "snow_cloud" => "cloud_with_snow",
        "rain_cloud" => "cloud_with_rain",
        "tornado_cloud" => "tornado",
        "thunder_cloud_and_rain" => "cloud_with_lightning_and_rain",
        "face_with_hand_over_mouth" => "hand_over_mouth",
        "face_with_symbols_on_mouth" => "cursing_face",
        "rolling_on_the_floor_laughing" => "rofl",
        "smiling_face_with_3_hearts" => "smiling_face_with_three_hearts",
        "partying_face" => "partying_face",
        "face_with_cowboy_hat" => "cowboy_hat_face",
        "clown_face" => "clown_face",
        "man-shrugging" => "man_shrugging",
        "woman-shrugging" => "woman_shrugging",
        "man-facepalming" => "man_facepalming",
        "woman-facepalming" => "woman_facepalming",
        "thumbsup_all" => "+1",
        "hankey" | "poop" | "shit" => "poop",
        "keycap_star" => "asterisk",
        "eject" => "eject_button",
        "black_square_for_stop" => "stop_button",
        "double_vertical_bar" => "pause_button",
        "black_circle_for_record" => "record_button",
        "barber" => "barber",
        "studio_microphone" => "studio_microphone",
        "level_slider" => "level_slider",
        "control_knobs" => "control_knobs",
        "film_frames" => "film_strip",
        "admission_tickets" => "tickets",
        "stadium" => "stadium",
        "house_buildings" => "houses",
        "derelict_house_building" => "derelict_house",
        "cityscape" => "cityscape",
        "desert_island" => "desert_island",
        "national_park" => "national_park",
        "classical_building" => "classical_building",
        "building_construction" => "building_construction",
        "linked_paperclips" => "paperclips",
        "lower_left_ballpoint_pen" => "pen",
        "lower_left_fountain_pen" => "fountain_pen",
        "lower_left_paintbrush" => "paintbrush",
        "lower_left_crayon" => "crayon",
        "card_index_dividers" => "card_index_dividers",
        "card_file_box" => "card_file_box",
        "file_cabinet" => "file_cabinet",
        "wastebasket" => "wastebasket",
        "old_key" => "old_key",
        "compression" => "clamp",
        "dagger_knife" => "dagger",
        "crossed_swords" => "crossed_swords",
        "shield" => "shield",
        "hammer_and_wrench" => "hammer_and_wrench",
        "pick" => "pick",
        "chains" => "chains",
        "gear" => "gear",
        "alembic" => "alembic",
        "scales" => "balance_scale",
        "couch_and_lamp" => "couch_and_lamp",
        "shopping_bags" => "shopping",
        "bellhop_bell" => "bellhop_bell",
        "bed" => "bed",
        "frame_with_picture" => "framed_picture",
        "world_map" => "world_map",
        "stopwatch" => "stopwatch",
        "timer_clock" => "timer_clock",
        "mantelpiece_clock" => "mantelpiece_clock",
        "thermometer" => "thermometer",
        "candle" => "candle",
        "desktop_computer" => "desktop_computer",
        "printer" => "printer",
        "three_button_mouse" => "computer_mouse",
        "trackball" => "trackball",
        "compression_clamp" => "clamp",
        "label" => "label",
        "bookmark_tabs" => "bookmark_tabs",
        "rolled_up_newspaper" => "newspaper_roll",
        "ballot_box_with_ballot" => "ballot_box",
        "white_frowning_face" => "frowning_face",
        "slightly_smiling_face" => "slightly_smiling_face",
        "slightly_frowning_face" => "slightly_frowning_face",
        "upside_down_face" => "upside_down_face",
        "zipper_mouth_face" => "zipper_mouth_face",
        "money_mouth_face" => "money_mouth_face",
        "nerd_face" => "nerd_face",
        "thinking_face" => "thinking",
        "face_with_head_bandage" => "face_with_head_bandage",
        "face_with_thermometer" => "face_with_thermometer",
        "robot_face" => "robot",
        "hugging_face" => "hugs",
        "eye-in-speech-bubble" => "eye_speech_bubble",
        _ => return None,
    })
}

/// Every standard emoji beyond the picker's [`CATALOG`], by the `emojis` crate's GitHub
/// shortcodes, which match Slack's for nearly all of them. Slack hyphenates multi-person
/// sequences (`man-woman-girl-boy`) where GitHub underscores them, families under `family_`,
/// prefixes colored circles/squares with `large_` (`large_yellow_circle` -> `yellow_circle`),
/// and uses `male-`/`female-` where GitHub uses `man_`/`woman_`.
pub(super) fn unicode_emoji(name: &str) -> Option<&'static emojis::Emoji> {
    if let Some(emoji) = emojis::get_by_shortcode(name) {
        return Some(emoji);
    }
    if let Some(alias) = slack_alias(name)
        && let Some(emoji) = emojis::get_by_shortcode(alias)
    {
        return Some(emoji);
    }
    let underscored = name.replace('-', "_");
    if let Some(emoji) = emojis::get_by_shortcode(&underscored) {
        return Some(emoji);
    }
    if let Some(stripped) = underscored.strip_prefix("large_")
        && let Some(emoji) = emojis::get_by_shortcode(stripped)
    {
        return Some(emoji);
    }
    if let Some(rest) = underscored.strip_prefix("female_")
        && let Some(emoji) = emojis::get_by_shortcode(&format!("woman_{rest}"))
    {
        return Some(emoji);
    }
    if let Some(rest) = underscored.strip_prefix("male_")
        && let Some(emoji) = emojis::get_by_shortcode(&format!("man_{rest}"))
    {
        return Some(emoji);
    }
    emojis::get_by_shortcode(&format!("family_{underscored}"))
}
