//! Shortcode resolution, skin-tone modifiers, inline rendering, preferences, and search helpers.

use std::collections::HashMap;

use gpui_kit::gpui::{
    AnyElement, ImageSource, IntoElement as _, ParentElement as _, Pixels, Styled as _, div, px,
};

use super::catalog::{CATALOG, Emoji, FREQUENT_MAX, lookup, skin_tone_swatch, unicode_emoji};
use super::picker::EmojiChoice;
use crate::blockkit::{Json, serde_json};
use crate::fixtures::Fixtures;

/// `wave` from `wave::skin-tone-3`.
pub(super) fn base_name(shortcode: &str) -> &str {
    shortcode.split("::").next().unwrap_or(shortcode)
}

/// The Slack skin tone (2..=6) a shortcode carries; 1 is the default yellow.
pub(super) fn tone_of(shortcode: &str) -> u8 {
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
pub(super) fn toned(emoji: &Emoji, tone: u8) -> String {
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
pub(super) fn toned_name(name: &str, tone: u8) -> String {
    if tone < 2 {
        name.to_owned()
    } else {
        format!("{name}::skin-tone-{tone}")
    }
}

/// The `emojis` crate's skin tone for Slack tone `tone` (2..=6); `None` for the default.
pub(super) fn skin_tone(tone: u8) -> Option<emojis::SkinTone> {
    match tone {
        2 => Some(emojis::SkinTone::Light),
        3 => Some(emojis::SkinTone::MediumLight),
        4 => Some(emojis::SkinTone::Medium),
        5 => Some(emojis::SkinTone::MediumDark),
        6 => Some(emojis::SkinTone::Dark),
        _ => None,
    }
}

/// The default glyph for a bare shortcode (`palm_tree`, no colons; a skin-tone suffix is
/// ignored), if it is a standard emoji.
pub(crate) fn glyph(shortcode: &str) -> Option<&'static str> {
    let name = base_name(shortcode);
    lookup(name)
        .map(|emoji| emoji.glyph)
        .or_else(|| skin_tone_swatch(name))
        .or_else(|| unicode_emoji(name).map(emojis::Emoji::as_str))
}

/// The glyph a shortcode displays as, with its `::skin-tone-N` suffix applied.
pub(crate) fn display_glyph(shortcode: &str) -> Option<String> {
    let name = base_name(shortcode);
    let tone = tone_of(shortcode);
    if let Some(emoji) = lookup(name) {
        return Some(toned(emoji, tone));
    }
    if let Some(swatch) = skin_tone_swatch(name) {
        return Some(swatch.to_owned());
    }
    let emoji = unicode_emoji(name)?;
    let emoji = skin_tone(tone)
        .and_then(|skin| emoji.with_skin_tone(skin))
        .unwrap_or(emoji);
    Some(emoji.as_str().to_owned())
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

/// Move `picked_name`'s base shortcode to the front of `frequent`, deduplicating and capping at [`FREQUENT_MAX`].
pub(super) fn record_frequent(frequent: &mut Vec<String>, picked_name: &str) {
    let base = base_name(picked_name).to_owned();
    frequent.retain(|name| *name != base);
    frequent.insert(0, base);
    frequent.truncate(FREQUENT_MAX);
}

/// Normalize a user's emoji search query (trimmed, lowercased, spaces to underscores).
pub(super) fn normalize_emoji_query(raw: &str) -> String {
    raw.trim().to_lowercase().replace(' ', "_")
}

/// Build an [`EmojiChoice`] for a catalog [`Emoji`] under skin `tone`.
pub(super) fn catalog_choice(emoji: &Emoji, tone: u8) -> EmojiChoice {
    let tone = if emoji.skin { tone } else { 1 };
    EmojiChoice {
        name: toned_name(emoji.name, tone),
        glyph: Some(toned(emoji, tone)),
    }
}

/// Search catalog emoji, then custom emoji names, whose shortcode contains `query`.
pub(super) fn search_emoji_choices<'a>(
    query: &str,
    tone: u8,
    custom_names: impl Iterator<Item = &'a str>,
) -> Vec<EmojiChoice> {
    CATALOG
        .iter()
        .flat_map(|(_, _, list)| list.iter())
        .filter(|emoji| emoji.name.contains(query))
        .map(|emoji| catalog_choice(emoji, tone))
        .chain(
            custom_names
                .filter(|name| name.contains(query))
                .map(|name| EmojiChoice {
                    name: name.to_owned(),
                    glyph: None,
                }),
        )
        .collect()
}
