//! Thin GUI wrappers around `slackless_core::domain::canvas_doc` adding the local Unicode emoji
//! catalog (`crate::emoji`) for canvas HTML/text rendering (`F-COLLAB-134..148`).

pub(crate) use slackless_core::domain::canvas_doc::{
    classify_media_url, is_markdown_table_separator, media_format_pill, split_markdown_table_row,
    strip_numbered_prefix,
};

/// Resolve `:shortcode:` tokens inside canvas text into Unicode emoji glyphs (`F-COLLAB-139`).
pub(crate) fn resolve_canvas_emoji(text: &str) -> String {
    crate::emoji::replace_shortcodes(text)
}
