//! Static Slack emoji catalog, category tabs, aliases, and fallback Unicode lookup.

use std::collections::HashMap;
use std::sync::OnceLock;

use gpui_kit::assets::IconName;

/// One catalog emoji: Slack shortcode, default glyph, and whether it takes a skin tone.
pub(super) struct Emoji {
    pub(super) name: &'static str,
    pub(super) glyph: &'static str,
    pub(super) skin: bool,
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
    s("handshake", "🤝"),
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

/// Standalone Fitzpatrick skin-tone swatch shortcodes (`:skin-tone-2:`..`:skin-tone-6:`).
pub(super) fn skin_tone_swatch(name: &str) -> Option<&'static str> {
    Some(match name {
        "skin-tone-2" | "skin_tone_2" => "\u{1F3FB}",
        "skin-tone-3" | "skin_tone_3" => "\u{1F3FC}",
        "skin-tone-4" | "skin_tone_4" => "\u{1F3FD}",
        "skin-tone-5" | "skin_tone_5" => "\u{1F3FE}",
        "skin-tone-6" | "skin_tone_6" => "\u{1F3FF}",
        _ => return None,
    })
}

/// Normalize an official Unicode CLDR emoji name (e.g. `"man mage"`, `"flag: Sweden"`,
/// `"men’s room"`) into an underscored lowercase lookup key. When `strip_punct` is true,
/// colons and apostrophes are dropped rather than converted into separators.
fn normalize_cldr_name(name: &str, strip_punct: bool) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_sep = true;
    for ch in name.chars() {
        if strip_punct && matches!(ch, ':' | '\'' | '\u{2019}') {
            continue;
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_sep = false;
        } else if !prev_sep {
            out.push('_');
            prev_sep = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

/// Lazily indexed map over every default/untone emoji in `emojis::iter()`, keyed by:
/// 1. All GitHub shortcodes (`emoji.shortcodes()`) — inserted first so GitHub shortcodes
///    take precedence on any collision.
/// 2. Normalized Unicode CLDR names (`emoji.name()`), both separator-normalized and with
///    `:` / `’` / `'` stripped (e.g. `"briefs"` -> 🩲, `"man_mage"` -> 🧙‍♂️,
///    `"woman_mage"` -> 🧙‍♀️, `"person_shrugging"` -> 🤷, `"man_gesturing_no"` -> 🙅‍♂️).
fn unicode_index() -> &'static HashMap<String, &'static emojis::Emoji> {
    static INDEX: OnceLock<HashMap<String, &'static emojis::Emoji>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut map = HashMap::with_capacity(4096);
        let default_emojis = emojis::iter().filter(|emoji| {
            emoji.skin_tone().is_none() || emoji.skin_tone() == Some(emojis::SkinTone::Default)
        });
        for emoji in default_emojis.clone() {
            for shortcode in emoji.shortcodes() {
                map.entry(shortcode.to_owned()).or_insert(emoji);
            }
        }
        for emoji in default_emojis {
            let name = emoji.name();
            let norm = normalize_cldr_name(name, false);
            if !norm.is_empty() {
                map.entry(norm).or_insert(emoji);
            }
            let stripped = normalize_cldr_name(name, true);
            if !stripped.is_empty() {
                map.entry(stripped).or_insert(emoji);
            }
        }
        map
    })
}

/// A GitHub shortcode or a normalized Unicode CLDR name.
fn crate_emoji(key: &str) -> Option<&'static emojis::Emoji> {
    emojis::get_by_shortcode(key).or_else(|| unicode_index().get(key).copied())
}

/// A Slack (`iamcal/emoji-data`) name the crate knows: a Slack alias for the crate's own name
/// first (GitHub gives `train` to the tram car), else a GitHub shortcode, else a normalized
/// Unicode CLDR name.
fn lookup_unicode_map(key: &str) -> Option<&'static emojis::Emoji> {
    slack_alias(key)
        .and_then(crate_emoji)
        .or_else(|| crate_emoji(key))
}

/// Slack-specific (`iamcal/emoji-data`) shortcode names that differ from both GitHub and CLDR.
fn slack_alias(name: &str) -> Option<&'static str> {
    Some(match name {
        "simple_smile" => "slightly_smiling_face",
        "thumbsup" | "thumbsup_all" => "+1",
        "thumbsdown" => "-1",
        "sleuth_or_spy" => "detective",
        "train" => "train2",
        "face_palm" => "facepalm",
        "unicorn_face" => "unicorn",
        "medal" => "medal_sports",
        "beach_with_umbrella" => "beach_umbrella",
        "face_with_raised_eyebrow" => "raised_eyebrow",
        "face_with_rolling_eyes" => "roll_eyes",
        "large_green_circle" => "green_circle",
        "waving_black_flag" => "black_flag",
        "waving_white_flag" => "white_flag",
        "rainbow-flag" => "rainbow_flag",
        "man_in_business_suit_levitating" => "business_suit_levitating",
        "speaking_head_in_silhouette" => "speaking_head",
        "female-doctor" | "female_doctor" => "woman_health_worker",
        "male-doctor" | "male_doctor" => "man_health_worker",
        "female-office-worker" | "female_office_worker" => "woman_office_worker",
        "male-office-worker" | "male_office_worker" => "man_office_worker",
        "female-scientist" | "female_scientist" => "woman_scientist",
        "male-scientist" | "male_scientist" => "man_scientist",
        "female-technologist" | "female_technologist" => "woman_technologist",
        "male-technologist" | "male_technologist" => "man_technologist",
        "female-teacher" | "female_teacher" => "woman_teacher",
        "male-teacher" | "male_teacher" => "man_teacher",
        "female-student" | "female_student" => "woman_student",
        "male-student" | "male_student" => "man_student",
        "female-cook" | "female_cook" => "woman_cook",
        "male-cook" | "male_cook" => "man_cook",
        "female-artist" | "female_artist" => "woman_artist",
        "male-artist" | "male_artist" => "man_artist",
        "female-pilot" | "female_pilot" => "woman_pilot",
        "male-pilot" | "male_pilot" => "man_pilot",
        "female-astronaut" | "female_astronaut" => "woman_astronaut",
        "male-astronaut" | "male_astronaut" => "man_astronaut",
        "female-firefighter" | "female_firefighter" => "woman_firefighter",
        "male-firefighter" | "male_firefighter" => "man_firefighter",
        "female-judge" | "female_judge" => "woman_judge",
        "male-judge" | "male_judge" => "man_judge",
        "female-mechanic" | "female_mechanic" => "woman_mechanic",
        "male-mechanic" | "male_mechanic" => "man_mechanic",
        "female-farmer" | "female_farmer" => "woman_farmer",
        "male-farmer" | "male_farmer" => "man_farmer",
        "female-factory-worker" | "female_factory_worker" => "woman_factory_worker",
        "male-factory-worker" | "male_factory_worker" => "man_factory_worker",
        "female-singer" | "female_singer" => "woman_singer",
        "male-singer" | "male_singer" => "man_singer",
        "female-guard" | "female_guard" => "woman_guard",
        "male-guard" | "male_guard" => "man_guard",
        "female-detective" | "female_detective" => "woman_detective",
        "male-detective" | "male_detective" => "man_detective",
        "female-construction-worker" | "female_construction_worker" => "woman_construction_worker",
        "male-construction-worker" | "male_construction_worker" => "man_construction_worker",
        "female-police-officer" | "female_police_officer" => "woman_police_officer",
        "male-police-officer" | "male_police_officer" => "man_police_officer",
        "female_sign" => "female_sign",
        "male_sign" => "male_sign",
        "heavy_plus_sign" => "heavy_plus_sign",
        "heavy_minus_sign" => "heavy_minus_sign",
        "heavy_division_sign" => "heavy_division_sign",
        "heavy_multiplication_x" => "heavy_multiplication_x",
        "black_small_square" => "black_small_square",
        "white_small_square" => "white_small_square",
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
        "face_with_cowboy_hat" => "cowboy_hat_face",
        "man-shrugging" => "man_shrugging",
        "woman-shrugging" => "woman_shrugging",
        "man-facepalming" => "man_facepalming",
        "woman-facepalming" => "woman_facepalming",
        "hankey" | "poop" | "shit" => "poop",
        "keycap_star" => "asterisk",
        "eject" => "eject_button",
        "black_square_for_stop" => "stop_button",
        "double_vertical_bar" => "pause_button",
        "black_circle_for_record" => "record_button",
        "film_frames" => "film_strip",
        "admission_tickets" => "tickets",
        "house_buildings" => "houses",
        "derelict_house_building" => "derelict_house",
        "linked_paperclips" => "paperclips",
        "lower_left_ballpoint_pen" => "pen",
        "lower_left_fountain_pen" => "fountain_pen",
        "lower_left_paintbrush" => "paintbrush",
        "lower_left_crayon" => "crayon",
        "compression" | "compression_clamp" => "clamp",
        "dagger_knife" => "dagger",
        "scales" => "balance_scale",
        "shopping_bags" => "shopping",
        "frame_with_picture" => "framed_picture",
        "three_button_mouse" => "computer_mouse",
        "rolled_up_newspaper" => "newspaper_roll",
        "ballot_box_with_ballot" => "ballot_box",
        "white_frowning_face" => "frowning_face",
        "thinking_face" => "thinking",
        "robot_face" => "robot",
        "hugging_face" => "hugs",
        "eye-in-speech-bubble" | "eye_in_speech_bubble" => "eye_speech_bubble",
        _ => return None,
    })
}

/// Resolve a 2-letter ISO country code (`flag-se` / `flag_se`) into its regional-indicator pair.
fn regional_flag(code: &str) -> Option<&'static emojis::Emoji> {
    let bytes = code.as_bytes();
    if bytes.len() != 2 || !bytes[0].is_ascii_alphabetic() || !bytes[1].is_ascii_alphabetic() {
        return None;
    }
    let a = char::from_u32(0x1F1E6 + u32::from(bytes[0].to_ascii_lowercase() - b'a'))?;
    let b = char::from_u32(0x1F1E6 + u32::from(bytes[1].to_ascii_lowercase() - b'a'))?;
    let mut flag = String::with_capacity(8);
    flag.push(a);
    flag.push(b);
    emojis::get(&flag)
}

/// Every standard emoji beyond the picker's [`CATALOG`], resolved through GitHub shortcodes,
/// normalized Unicode CLDR names, Slack `iamcal/emoji-data` aliases, `flag-XX` regional
/// indicators, and Slack's gender/prefix conventions (`male_X`, `female_X`, `large_X`,
/// `family_X`).
pub(super) fn unicode_emoji(name: &str) -> Option<&'static emojis::Emoji> {
    if let Some(emoji) = lookup_unicode_map(name) {
        return Some(emoji);
    }
    let underscored = name.replace('-', "_");
    if underscored != name
        && let Some(emoji) = lookup_unicode_map(&underscored)
    {
        return Some(emoji);
    }
    if let Some(code) = underscored.strip_prefix("flag_")
        && let Some(emoji) = regional_flag(code)
    {
        return Some(emoji);
    }
    if let Some(stripped) = underscored.strip_prefix("large_")
        && let Some(emoji) = lookup_unicode_map(stripped)
    {
        return Some(emoji);
    }
    if let Some(rest) = underscored.strip_prefix("female_")
        && let Some(emoji) = lookup_unicode_map(&format!("woman_{rest}"))
            .or_else(|| lookup_unicode_map(&format!("{rest}_woman")))
    {
        return Some(emoji);
    }
    if let Some(rest) = underscored.strip_prefix("male_")
        && let Some(emoji) = lookup_unicode_map(&format!("man_{rest}"))
            .or_else(|| lookup_unicode_map(&format!("{rest}_man")))
    {
        return Some(emoji);
    }
    if let Some(rest) = underscored.strip_prefix("man_")
        && let Some(emoji) = lookup_unicode_map(&format!("{rest}_man"))
    {
        return Some(emoji);
    }
    if let Some(rest) = underscored.strip_prefix("woman_")
        && let Some(emoji) = lookup_unicode_map(&format!("{rest}_woman"))
    {
        return Some(emoji);
    }
    if let Some(rest) = underscored.strip_prefix("person_")
        && let Some(emoji) =
            lookup_unicode_map(rest).or_else(|| lookup_unicode_map(&format!("{rest}_person")))
    {
        return Some(emoji);
    }
    lookup_unicode_map(&format!("family_{underscored}"))
}
