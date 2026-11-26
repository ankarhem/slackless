//! Message body, rich-text / mrkdwn layout, and attachment / huddle-card rendering.

mod blocks;
mod cards;
mod inline;
#[cfg(test)]
mod tests;

pub(super) use super::code_block::{code_block, render_code};
pub(crate) use super::code_block::{selectable_code, selectable_text};
pub(super) use super::selectable::SelectableParagraph;
pub(super) use super::time::{
    CivilTime, card_stamp, date_label, date_label_absolute, day_index, format_clock,
    format_clock_with,
};
pub(crate) use super::time::{
    clock, day_and_time, day_label, file_day, iso_date, list_stamp, local_offset_now, now_secs,
    short_date,
};

pub(super) use blocks::message_body;
pub(crate) use blocks::{is_deleted_ghost, mrkdwn, mrkdwn_with_verbatim, render_block};
#[cfg(test)]
pub(crate) use cards::format_huddle_duration;
#[cfg(test)]
pub(crate) use inline::expand_shortcodes_in_text;
pub(super) use inline::{DEFAULT_FLAGS, RunFlags, RunTheme, paragraph_id};
#[cfg(test)]
pub(super) use inline::{
    ParagraphBuilder, angle_token, jumbomoji_elements, mrkdwn_fences, push_mrkdwn_text,
    strip_quote_prefix, styled_label, toggle_marker,
};
pub(crate) use inline::{hex_color, mrkdwn_preview, plain_text};
