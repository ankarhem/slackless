//! The picker's emoji catalog and tabs, and Slack shortcode lookup through the `emojis` crate.

use std::collections::HashMap;
use std::sync::OnceLock;

use gpui_kit::assets::IconName;

/// One emoji in the picker's catalog: the Slack shortcode it is named and picked by, and the
/// `emojis` crate entry it draws as (glyph, CLDR name, skin tones).
pub(super) struct Emoji {
    pub(super) name: &'static str,
    pub(super) emoji: &'static emojis::Emoji,
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

/// The picker's categories in Slack's order, under Slack's section titles: the Slack shortcodes
/// of every emoji in the section, in Slack's order. Glyphs, skin tones and names to search come
/// from the `emojis` crate through [`lookup`]; the flags follow Slack's naming (`flag-se`, the
/// bare code for the ten oldest).
#[rustfmt::skip]
pub(super) const CATALOG: [(Category, &str, &[&str]); 8] = [
    (Category::People, "Smileys & People", &[
        "grinning", "smiley", "smile", "grin", "laughing", "sweat_smile", "joy",
        "rolling_on_the_floor_laughing", "slightly_smiling_face", "upside_down_face", "wink",
        "blush", "innocent", "heart_eyes", "star-struck", "kissing_heart", "yum",
        "stuck_out_tongue", "thinking_face", "face_with_raised_eyebrow", "neutral_face",
        "expressionless", "no_mouth", "smirk", "unamused", "face_with_rolling_eyes", "grimacing",
        "relieved", "pensive", "sleepy", "sleeping", "mask", "face_with_thermometer", "nerd_face",
        "sunglasses", "confused", "worried", "open_mouth", "astonished", "flushed", "cry", "sob",
        "scream", "rage", "skull", "clown_face", "ghost", "robot_face", "wave", "raised_hand",
        "ok_hand", "v", "crossed_fingers", "point_up", "thumbsup", "thumbsdown", "clap",
        "raised_hands", "pray", "muscle", "handshake", "eyes", "brain", "face_palm", "shrug",
    ]),
    (Category::Nature, "Animals & Nature", &[
        "dog", "cat", "fox_face", "bear", "panda_face", "unicorn_face", "bee", "bug", "butterfly",
        "turtle", "octopus", "whale", "sunflower", "rose", "seedling", "evergreen_tree",
        "palm_tree", "cactus", "fallen_leaf", "sunny", "cloud", "rainbow", "snowflake", "zap",
        "fire", "droplet", "ocean", "star", "sparkles", "crescent_moon",
    ]),
    (Category::Food, "Food & Drink", &[
        "coffee", "tea", "beer", "wine_glass", "pizza", "hamburger", "fries", "taco", "sushi",
        "ramen", "sandwich", "cake", "birthday", "doughnut", "cookie", "apple", "banana", "avocado",
        "popcorn", "ice_cream",
    ]),
    (Category::Travel, "Travel & Places", &[
        "rocket", "airplane", "car", "bus", "train", "bike", "ship", "house", "house_with_garden",
        "office", "earth_africa", "mountain", "beach_with_umbrella", "world_map", "tent",
        "construction",
    ]),
    (Category::Activity, "Activities", &[
        "tada", "confetti_ball", "balloon", "gift", "trophy", "medal", "soccer", "basketball",
        "tennis", "video_game", "dart", "game_die", "art", "guitar", "headphones", "microphone",
        "runner", "bicyclist",
    ]),
    (Category::Objects, "Objects", &[
        "computer", "keyboard", "iphone", "bulb", "calendar", "spiral_calendar_pad", "memo",
        "pencil2", "books", "package", "lock", "key", "hammer_and_wrench", "gear", "mag", "link",
        "paperclip", "pushpin", "chart_with_upwards_trend", "bell", "hourglass", "alarm_clock",
        "money_with_wings", "speech_balloon", "email",
    ]),
    (Category::Symbols, "Symbols", &[
        "heart", "orange_heart", "yellow_heart", "green_heart", "blue_heart", "purple_heart",
        "broken_heart", "100", "white_check_mark", "heavy_check_mark", "x", "warning", "no_entry",
        "question", "exclamation", "bangbang", "recycle", "zzz", "red_circle", "large_green_circle",
        "large_blue_circle", "arrow_up", "arrow_down", "repeat", "new", "sos",
    ]),
    (Category::Flags, "Flags", &[
        "checkered_flag", "triangular_flag_on_post", "crossed_flags", "waving_black_flag",
        "waving_white_flag", "rainbow-flag", "transgender_flag", "pirate_flag", "flag-ar",
        "flag-at", "flag-au", "flag-be", "flag-br", "flag-ca", "flag-ch", "cn", "de", "flag-dk",
        "flag-ee", "es", "flag-eu", "flag-fi", "fr", "gb", "flag-ie", "flag-in", "flag-is", "it",
        "jp", "kr", "flag-mx", "flag-nl", "flag-no", "flag-nz", "flag-pl", "flag-pt", "flag-se",
        "flag-ua", "us", "flag-za",
    ]),
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

/// The catalog entry a Slack shortcode denotes, under the catalog's own name: `thumbsup` itself,
/// or another name for the same emoji (`+1`, `satisfied` for `laughing`).
pub(super) fn lookup(name: &str) -> Option<&'static Emoji> {
    static INDEX: OnceLock<HashMap<&'static emojis::Emoji, Emoji>> = OnceLock::new();
    let index = INDEX.get_or_init(|| {
        let mut index = HashMap::new();
        for (_, _, list) in CATALOG {
            for &name in list {
                if let Some(emoji) = unicode_emoji(name) {
                    index.insert(emoji, Emoji { name, emoji });
                }
            }
        }
        index
    });
    unicode_emoji(name).and_then(|emoji| index.get(emoji))
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

/// Slack (`iamcal/emoji-data`) shortcodes the crate spells differently, with the crate's name (a
/// GitHub shortcode or normalized CLDR name). The catalog's own names are all here, CLDR spellings
/// included, so the picker resolves without building `unicode_index`.
fn slack_alias(name: &str) -> Option<&'static str> {
    Some(match name {
        "simple_smile" => "slightly_smiling_face",
        "thumbsup_all" => "+1",
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
        "rolling_on_the_floor_laughing" => "rofl",
        "star-struck" => "star_struck",
        "thinking_face" => "thinking",
        "robot_face" => "robot",
        "spiral_calendar_pad" => "spiral_calendar",
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
        "female-construction-worker" | "female_construction_worker" => "woman_construction_worker",
        "male-construction-worker" | "male_construction_worker" => "man_construction_worker",
        "female-police-officer" | "female_police_officer" => "woman_police_officer",
        "male-police-officer" | "male_police_officer" => "man_police_officer",
        "spiral_note_pad" => "spiral_notepad",
        "knife_fork_plate" => "plate_with_cutlery",
        "the_horns" => "metal",
        "spock-hand" | "spock_hand" => "vulcan_salute",
        "mostly_sunny" => "sun_behind_small_cloud",
        "barely_sunny" => "sun_behind_large_cloud",
        "partly_sunny_rain" => "sun_behind_rain_cloud",
        "lightning_cloud" => "cloud_with_lightning",
        "snow_cloud" => "cloud_with_snow",
        "rain_cloud" => "cloud_with_rain",
        "tornado_cloud" => "tornado",
        "thunder_cloud_and_rain" => "cloud_with_lightning_and_rain",
        "smiling_face_with_3_hearts" => "smiling_face_with_three_hearts",
        "face_with_cowboy_hat" => "cowboy_hat_face",
        "keycap_star" => "asterisk",
        "eject" => "eject_button",
        "black_square_for_stop" => "stop_button",
        "double_vertical_bar" => "pause_button",
        "black_circle_for_record" => "record_button",
        "house_buildings" => "houses",
        "derelict_house_building" => "derelict_house",
        "lower_left_ballpoint_pen" => "pen",
        "lower_left_fountain_pen" => "fountain_pen",
        "lower_left_paintbrush" => "paintbrush",
        "lower_left_crayon" => "crayon",
        "compression" | "compression_clamp" => "clamp",
        "dagger_knife" => "dagger",
        "scales" => "balance_scale",
        "frame_with_picture" => "framed_picture",
        "three_button_mouse" => "computer_mouse",
        "white_frowning_face" => "frowning_face",
        "hugging_face" => "hugs",
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

/// Every standard emoji Slack names, the catalog's included, resolved through Slack
/// `iamcal/emoji-data` aliases, GitHub shortcodes, normalized Unicode CLDR names, `flag-XX`
/// regional indicators, and Slack's gender/prefix conventions (`male_X`, `female_X`, `large_X`,
/// `family_X`).
pub(super) fn unicode_emoji(name: &str) -> Option<&'static emojis::Emoji> {
    if let Some(code) = name
        .strip_prefix("flag-")
        .or_else(|| name.strip_prefix("flag_"))
        && let Some(emoji) = regional_flag(code)
    {
        return Some(emoji);
    }
    if let Some(emoji) = lookup_unicode_map(name) {
        return Some(emoji);
    }
    let underscored = name.replace('-', "_");
    if underscored != name
        && let Some(emoji) = lookup_unicode_map(&underscored)
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
