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

mod catalog;
mod gifs;
mod picker;
mod resolve;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub use catalog::Emoji;
pub(crate) use gifs::evict_gif_asset;
pub use picker::{EmojiChoice, EmojiPicker, EmojiPickerEvent, PickerTab};
pub(crate) use resolve::{
    EmojiArt, display_glyph, emoji_element, frequent_from_prefs, glyph, parse_shortcode,
    replace_shortcodes, skin_tone_from_prefs,
};
