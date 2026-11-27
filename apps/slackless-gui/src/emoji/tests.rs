use super::catalog::{CATALOG, FREQUENT_MAX, WORK, lookup};
use super::gifs::filter_gif_presets;
use super::resolve::{
    base_name, normalize_emoji_query, record_frequent, search_emoji_choices, tone_of, toned,
    toned_choice,
};
use super::*;
use crate::blockkit::Json;
use crate::blockkit::serde_json::{self, json};

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
    let many: serde_json::Map<String, Json> = CATALOG[0]
        .2
        .iter()
        .map(|name| ((*name).to_owned(), json!(1)))
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

#[test]
fn slack_circle_square_and_alias_shortcodes_resolve() {
    for code in [
        "large_yellow_circle",
        "large_orange_circle",
        "large_purple_circle",
        "large_brown_circle",
        "large_green_circle",
        "large_blue_circle",
        "large_yellow_square",
        "large_red_square",
        "spiral_calendar_pad",
        "spiral_note_pad",
        "robot_face",
        "hugging_face",
        "thinking_face",
        "female-technologist",
        "male-technologist",
    ] {
        assert!(
            display_glyph(code).is_some(),
            "expected Slack shortcode :{code}: to resolve"
        );
    }
}

#[test]
fn crate_built_catalog_keeps_slack_names_and_glyphs() {
    // The catalog is built from the crate. Slack spells these shortcodes differently from it (an
    // alias entry or a `flag-XX` code); each keeps the glyph the hand-kept catalog drew it as.
    for (name, glyph) in [
        ("rolling_on_the_floor_laughing", "🤣"),
        ("star-struck", "🤩"),
        ("thinking_face", "🤔"),
        ("face_with_raised_eyebrow", "🤨"),
        ("face_with_rolling_eyes", "🙄"),
        ("robot_face", "🤖"),
        ("face_palm", "🤦"),
        ("unicorn_face", "🦄"),
        ("medal", "🏅"),
        ("beach_with_umbrella", "🏖️"),
        ("train", "🚆"),
        ("spiral_calendar_pad", "🗓️"),
        ("large_green_circle", "🟢"),
        ("waving_black_flag", "🏴"),
        ("waving_white_flag", "🏳️"),
        ("rainbow-flag", "🏳️‍🌈"),
        ("flag-se", "🇸🇪"),
        ("flag-eu", "🇪🇺"),
        ("flag-za", "🇿🇦"),
    ] {
        assert_eq!(
            lookup(name).map(|emoji| emoji.emoji.as_str()),
            Some(glyph),
            ":{name}:"
        );
    }
    // Other names for a catalog emoji resolve to the catalog's own name.
    for (alias, canonical) in [
        ("+1", "thumbsup"),
        ("-1", "thumbsdown"),
        ("hand", "raised_hand"),
        ("facepalm", "face_palm"),
        ("simple_smile", "slightly_smiling_face"),
    ] {
        assert_eq!(
            lookup(alias).map(|emoji| emoji.name),
            Some(canonical),
            ":{alias}:"
        );
    }
    // Every catalog shortcode is its own catalog entry (a typo, or two names for one emoji, would
    // drop it from the picker), and no section shrinks below the hand-kept catalog's size.
    let lost: Vec<&str> = CATALOG
        .iter()
        .flat_map(|(_, _, list)| list.iter().copied())
        .filter(|&name| lookup(name).map(|emoji| emoji.name) != Some(name))
        .collect();
    assert!(lost.is_empty(), "{lost:?}");
    let sizes = CATALOG.map(|(_, _, list)| list.len());
    let minimums = [65, 30, 20, 16, 18, 25, 26, 40];
    assert!(
        sizes.iter().zip(minimums).all(|(size, min)| *size >= min),
        "{sizes:?}"
    );
}

#[test]
fn catalog_aliases_and_skin_tone_modifiers_work() {
    // Catalog aliases map to the canonical catalog emoji.
    assert_eq!(lookup("+1").map(|e| e.name), Some("thumbsup"));
    assert_eq!(lookup("-1").map(|e| e.name), Some("thumbsdown"));
    assert_eq!(
        lookup("simple_smile").map(|e| e.name),
        Some("slightly_smiling_face")
    );
    assert_eq!(lookup("hand").map(|e| e.name), Some("raised_hand"));
    assert_eq!(lookup("facepalm").map(|e| e.name), Some("face_palm"));

    // Base name & tone parsing.
    assert_eq!(base_name("wave::skin-tone-3"), "wave");
    assert_eq!(base_name("wave"), "wave");
    assert_eq!(tone_of("wave"), 1);
    assert_eq!(tone_of("wave::skin-tone-2"), 2);
    assert_eq!(tone_of("wave::skin-tone-6"), 6);
    assert_eq!(tone_of("wave::skin-tone-1"), 1);
    assert_eq!(tone_of("wave::skin-tone-7"), 1);
    assert_eq!(tone_of("wave::other"), 1);

    assert_eq!(
        toned_choice("wave", lookup("wave").unwrap().emoji, 1).name,
        "wave"
    );
    assert_eq!(
        toned_choice("wave", lookup("wave").unwrap().emoji, 4).name,
        "wave::skin-tone-4"
    );

    // A skin-tone capable emoji gets the Fitzpatrick modifier, which replaces the variation
    // selector; a non-skin emoji ignores the tone.
    let wave = lookup("wave").unwrap();
    assert_eq!(toned(wave.emoji, 1), "👋");
    assert_eq!(toned(wave.emoji, 2), "👋\u{1F3FB}");
    assert_eq!(toned(wave.emoji, 6), "👋\u{1F3FF}");

    let victory = lookup("v").unwrap();
    let toned_v = toned(victory.emoji, 3);
    assert!(!toned_v.contains('\u{FE0F}'));
    assert!(toned_v.contains('\u{1F3FC}'));

    let rocket = lookup("rocket").unwrap();
    assert_eq!(toned(rocket.emoji, 5), "🚀");

    // `glyph` ignores skin-tone suffix; `display_glyph` applies it (including unicode fallback).
    assert_eq!(glyph("wave::skin-tone-4"), Some("👋"));
    assert_eq!(
        display_glyph("wave::skin-tone-4").as_deref(),
        Some("👋\u{1F3FD}")
    );
    let fallback_toned = display_glyph("raised_back_of_hand::skin-tone-5");
    assert!(fallback_toned.is_some_and(|g| g.contains('\u{1F3FE}')));
}

#[test]
fn parse_and_replace_shortcodes_handles_edge_cases() {
    assert_eq!(parse_shortcode(":smile: rest"), Some((7, "smile".into())));
    assert_eq!(
        parse_shortcode(":+1::skin-tone-3: ok"),
        Some((17, "+1::skin-tone-3".into()))
    );
    // Rejects empty, uppercase, spaces, missing closing colon, and overly long (>40) names.
    assert_eq!(parse_shortcode("::"), None);
    assert_eq!(parse_shortcode(":Smile:"), None);
    assert_eq!(parse_shortcode(":two words:"), None);
    assert_eq!(parse_shortcode(":unclosed"), None);
    let too_long = format!(":{}:", "a".repeat(41));
    assert_eq!(parse_shortcode(&too_long), None);

    // `replace_shortcodes` replaces known standard/toned codes while leaving unknown & prose intact.
    assert_eq!(
        replace_shortcodes("Go :rocket: :wave::skin-tone-2: :unknown_xyz: 12:30!"),
        "Go 🚀 👋\u{1F3FB} :unknown_xyz: 12:30!"
    );
}

#[test]
fn emoji_art_resolves_glyph_custom_image_and_literal_fallback() {
    let fixtures = crate::fixtures::load();
    assert!(matches!(EmojiArt::of("rocket", &fixtures), EmojiArt::Glyph(g) if g == "🚀"));
    assert!(matches!(
        EmojiArt::of("shipit", &fixtures),
        EmojiArt::Image(_)
    ));
    assert!(matches!(
        EmojiArt::of("shipit::skin-tone-3", &fixtures),
        EmojiArt::Image(_)
    ));
    assert!(matches!(
        EmojiArt::of("definitely_missing_emoji", &fixtures),
        EmojiArt::Text(t) if t == ":definitely_missing_emoji:"
    ));
}

#[test]
fn search_ranking_query_normalization_and_frequent_tracking() {
    assert_eq!(normalize_emoji_query("  Thumbs Up  "), "thumbs_up");

    // Catalog results come before custom emoji, and tone applies only to skin-capable matches.
    let custom = ["wave_custom", "shipit"];
    let results = search_emoji_choices("wave", 4, custom.iter().copied());
    assert!(results.len() >= 2);
    assert_eq!(results[0].name, "wave::skin-tone-4");
    assert_eq!(results[0].glyph.as_deref(), Some("👋\u{1F3FD}"));
    let last = results.last().unwrap();
    assert_eq!(last.name, "wave_custom");
    assert_eq!(last.glyph, None);

    assert!(search_emoji_choices("zzz_no_match_zzz", 1, custom.iter().copied()).is_empty());

    // `record_frequent` deduplicates toned names by base, moves to front, and caps at FREQUENT_MAX.
    let mut freq: Vec<String> = (0..FREQUENT_MAX).map(|i| format!("e{i}")).collect();
    record_frequent(&mut freq, "e5::skin-tone-3");
    assert_eq!(freq.len(), FREQUENT_MAX);
    assert_eq!(freq[0], "e5");
    assert_eq!(freq.iter().filter(|n| n.as_str() == "e5").count(), 1);

    record_frequent(&mut freq, "brand_new");
    assert_eq!(freq.len(), FREQUENT_MAX);
    assert_eq!(freq[0], "brand_new");

    // Offline GIF presets filter case-insensitively.
    assert_eq!(filter_gif_presets("").len(), 12);
    let thumbs = filter_gif_presets("THUMBS");
    assert_eq!(thumbs.len(), 1);
    assert_eq!(thumbs[0].0, "Thumbs up");
}

#[test]
fn cldr_gendered_zwj_flags_and_skin_toned_slack_shortcodes_resolve() {
    // Previously missing CLDR + gendered ZWJ shortcodes with and without skin tones.
    assert_eq!(display_glyph("male_mage").as_deref(), Some("🧙‍♂️"));
    assert_eq!(
        display_glyph("male_mage::skin-tone-2").as_deref(),
        Some("🧙🏻‍♂️")
    );
    assert_eq!(
        display_glyph("male_mage::skin-tone-4").as_deref(),
        Some("🧙🏽‍♂️")
    );
    assert_eq!(
        display_glyph("female_mage::skin-tone-3").as_deref(),
        Some("🧙🏼‍♀️")
    );
    assert_eq!(display_glyph("briefs").as_deref(), Some("🩲"));

    // Regional indicator flag-XX resolution (both catalog and non-catalog ISO codes).
    assert_eq!(display_glyph("flag-se").as_deref(), Some("🇸🇪"));
    assert_eq!(display_glyph("flag-us").as_deref(), Some("🇺🇸"));
    assert_eq!(display_glyph("flag-gb").as_deref(), Some("🇬🇧"));
    assert_eq!(display_glyph("flag-jp").as_deref(), Some("🇯🇵"));
    assert_eq!(display_glyph("flag-zz"), None);

    // Slack professions, fantasy characters, prefixes, and hyphenated ZWJ tones.
    assert_eq!(display_glyph("male-technologist").as_deref(), Some("👨‍💻"));
    assert_eq!(
        display_glyph("female-scientist::skin-tone-5").as_deref(),
        Some("👩🏾‍🔬")
    );
    assert_eq!(display_glyph("male_vampire").as_deref(), Some("🧛‍♂️"));
    assert_eq!(display_glyph("female_elf").as_deref(), Some("🧝‍♀️"));
    assert_eq!(display_glyph("large_yellow_circle").as_deref(), Some("🟡"));
    assert_eq!(
        display_glyph("man-shrugging::skin-tone-3").as_deref(),
        Some("🤷🏼‍♂️")
    );
    assert_eq!(display_glyph("female-doctor").as_deref(), Some("👩‍⚕️"));
    assert_eq!(display_glyph("male-doctor").as_deref(), Some("👨‍⚕️"));
    assert_eq!(display_glyph("sleuth_or_spy").as_deref(), Some("🕵️"));
    assert_eq!(
        display_glyph("man_in_business_suit_levitating").as_deref(),
        Some("🕴️")
    );
    assert_eq!(
        display_glyph("speaking_head_in_silhouette").as_deref(),
        Some("🗣️")
    );
    assert_eq!(display_glyph("skin-tone-4").as_deref(), Some("\u{1F3FD}"));

    // Inline parsing & shortcode replacement end-to-end.
    assert_eq!(
        parse_shortcode(":male_mage::skin-tone-4:"),
        Some((24, "male_mage::skin-tone-4".into()))
    );
    assert_eq!(
        replace_shortcodes(":male_mage::skin-tone-4: :briefs:"),
        "🧙🏽‍♂️ 🩲"
    );
}
