//! File attachments under a message body: image thumbnails rendered from in-memory bytes
//! (gpui-kit-guide section 9) or, live, loaded from Slack's thumbnail URL through the app's
//! HTTP client, and Slack-style file cards (type tile, name, type and size) for the rest.
//! Several images show as Slack's gallery ([`Gallery`]); every image preview has Slack's
//! hover Download button.

mod cards;
mod gallery;
mod media_cards;
mod meta;
mod snippet;

#[cfg(test)]
mod tests;

use gpui_kit::component::v_flex;
use gpui_kit::gpui::{App, Div, ParentElement as _, SharedString, Styled as _, div};
use slackless_core::types::Message;

use crate::fixtures::Fixtures;
use crate::image_viewer::ImageClick;

use cards::{external_file_card, file_card};
use gallery::{Gallery, GalleryImage, image_attachment, thumbnail};
use media_cards::{audio_card, video_card};
use meta::{inline_allowed, is_audio, is_snippet};
use snippet::SnippetCard;

#[cfg(test)]
pub(crate) use media_cards::set_clip_transcript_expanded;
pub(crate) use media_cards::{clip_transcript_status, fetch_clip_vtt_if_needed};
pub(crate) use meta::{
    clip_vtt_url, download_url, file_ext, file_icon, fit, human_size, inline_clip_transcript,
    is_decodable_image_url, is_video, media_duration_ms, video_playback_url, video_poster_url,
};

/// The message's attachments, or `None` when it has no files. `on_open` makes image
/// attachments open the viewer on click (the transcript and thread panel provide it; static
/// cards do not). Two or more images and nothing else show as Slack's [`Gallery`].
pub fn attachments(
    fixtures: &Fixtures,
    message: &Message,
    on_open: Option<&ImageClick>,
    cx: &App,
) -> Option<Div> {
    if message.files.is_empty() {
        return None;
    }
    let show_large = crate::preferences::display_prefs(cx).large_files;
    let mut column = v_flex().gap_2().pt_1();
    let images: Vec<GalleryImage> = message
        .files
        .iter()
        .enumerate()
        .filter_map(|(ix, file)| {
            if !inline_allowed(file, show_large) {
                return None;
            }
            Some(GalleryImage {
                ix,
                source: thumbnail(fixtures, file)?,
                file: file.clone(),
            })
        })
        .collect();
    if images.len() >= 2 && images.len() == message.files.len() {
        return Some(column.child(Gallery {
            id: SharedString::from(format!("gallery-{}", message.ts.as_str())),
            images,
            on_open: on_open.cloned(),
        }));
    }
    for (ix, file) in message.files.iter().enumerate() {
        // Reserve a disjoint selection document-order slice per file after the message body.
        let order_base =
            crate::messages::message_order(&message.ts) + 512_000 + (ix as u64) * 65_536;
        column = column.child(if is_audio(file) {
            audio_card(fixtures, file, order_base, cx)
        } else if is_video(file) {
            video_card(fixtures, file, ix, order_base, on_open, cx)
        } else if is_snippet(file) {
            div().child(SnippetCard {
                file: file.clone(),
                order_base,
            })
        } else {
            match inline_allowed(file, show_large)
                .then(|| thumbnail(fixtures, file))
                .flatten()
            {
                Some(source) => image_attachment(file, ix, source, on_open, cx),
                None if file.is_external || file.external_type.is_some() => {
                    external_file_card(file, cx)
                }
                None => file_card(file, cx),
            }
        });
    }
    Some(column)
}
