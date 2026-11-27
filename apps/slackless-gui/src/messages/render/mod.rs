//! Message body, rich-text / mrkdwn layout, and attachment / huddle-card rendering.

mod blocks;
mod body;
mod cards;
mod inline;
mod model;
#[cfg(test)]
mod tests;

pub(super) use super::code_block::CodeModel;
pub(crate) use super::code_block::{selectable_code, selectable_text};
pub(super) use super::selectable::SelectableParagraph;
pub(crate) use super::time::format_clock_with;
pub(super) use super::time::{CivilTime, card_stamp, date_label, day_index, format_clock};
pub(crate) use super::time::{
    clock, day_and_time, day_label, file_day, iso_date, list_stamp, local_offset_now, now_secs,
    short_date,
};

#[cfg(test)]
pub(super) use blocks::render_mrkdwn;
pub(super) use blocks::{message_body, message_body_cached};
pub(crate) use blocks::{mrkdwn, mrkdwn_with_verbatim, render_block};
pub(in crate::messages) use body::BodyCache;
#[cfg(test)]
pub(in crate::messages) use body::BodyModel;
#[cfg(test)]
pub(in crate::messages) use body::MrkdwnBlock;
pub(crate) use body::is_deleted_ghost;
#[cfg(test)]
pub(crate) use cards::format_huddle_duration;
#[cfg(test)]
pub(super) use inline::jumbomoji_elements;
pub(super) use inline::{DEFAULT_FLAGS, RunFlags, RunTheme, paragraph_id, text_hash};
pub(crate) use inline::{Preview, hex_color, memoized_preview, plain_text, render_preview};
#[cfg(test)]
pub(super) use model::{
    RunStyle, Segment, TextOpts, Tone, line_segments, mrkdwn_fences, strip_quote_prefix,
};
