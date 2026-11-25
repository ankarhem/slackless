//! Thin GUI wrappers around `slackless_core::domain::canvas_doc` adding the local Unicode emoji
//! catalog (`crate::emoji`) for canvas HTML/text rendering (`F-COLLAB-134..148`).

#[cfg(test)]
pub use slackless_core::domain::canvas_doc::parse_canvas_markdown;
#[cfg(test)]
pub(crate) use slackless_core::domain::canvas_doc::{
    classify_media_mime, parse_slack_doc_url, parse_slack_profile_url,
};
pub(crate) use slackless_core::domain::canvas_doc::{
    classify_media_url, is_markdown_table_separator, media_format_pill, split_markdown_table_row,
    strip_numbered_prefix,
};

#[cfg(test)]
use crate::canvas::model::CanvasBlock;

/// Resolve `:shortcode:` tokens inside canvas text into Unicode emoji glyphs (`F-COLLAB-139`).
pub(crate) fn resolve_canvas_emoji(text: &str) -> String {
    crate::emoji::replace_shortcodes(text)
}

/// Parse Slack's Quip/Canvas HTML export into structured [`CanvasBlock`]s, resolving emoji
/// shortcodes via the GUI's Unicode emoji catalog.
#[cfg(test)]
pub fn parse_canvas_html(html: &str) -> Vec<CanvasBlock> {
    slackless_core::domain::canvas_doc::parse_canvas_html_with_ids_and_emoji(
        html,
        &crate::emoji::display_glyph,
    )
    .0
}
