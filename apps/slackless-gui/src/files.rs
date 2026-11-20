//! File attachments under a message body: image thumbnails rendered from in-memory bytes
//! (gpui-kit-guide section 9) or, live, loaded from Slack's thumbnail URL through the app's
//! HTTP client, and Slack-style file cards (type tile, name, type and size) for the rest.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::{
    App, Div, FontWeight, Hsla, ImageSource, ParentElement as _, SharedString, Styled as _, div,
    img, px,
};
use slackless_core::types::{File, Message};

use crate::fixtures::Fixtures;

/// Largest rendered thumbnail box (px); images scale down to fit, keeping their aspect.
const MAX_THUMB_W: f32 = 360.;
const MAX_THUMB_H: f32 = 240.;
const CARD_W: f32 = 340.;

/// The message's attachments, or `None` when it has no files.
pub fn attachments(fixtures: &Fixtures, message: &Message, cx: &App) -> Option<Div> {
    if message.files.is_empty() {
        return None;
    }
    let mut column = v_flex().gap_2().pt_1();
    for file in &message.files {
        let thumbnail = match fixtures.thumbnails.get(&file.id) {
            Some(image) => Some(ImageSource::Image(image.clone())),
            None => remote_thumbnail(file).map(|url| ImageSource::from(SharedString::from(url))),
        };
        column = column.child(match thumbnail {
            Some(source) => image_attachment(file, source, cx),
            None => file_card(file, cx),
        });
    }
    Some(column)
}

/// A live image's thumbnail URL, sized for the inline preview on a 2x display.
fn remote_thumbnail(file: &File) -> Option<String> {
    if !file
        .mimetype
        .as_deref()
        .is_some_and(|mime| mime.starts_with("image/"))
    {
        return None;
    }
    file.thumb_480
        .clone()
        .or_else(|| file.thumb_360.clone())
        .or_else(|| file.thumb_720.clone())
}

fn image_attachment(file: &File, image: ImageSource, cx: &App) -> Div {
    let theme = cx.theme();
    let (w, h) = fit(file.original_w, file.original_h);
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .gap_1()
                .items_center()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(file_name(file))
                .child(
                    Icon::new(IconName::ChevronDown)
                        .text_color(theme.muted_foreground)
                        .size(px(12.)),
                ),
        )
        .child(
            div()
                .w(px(w))
                .h(px(h))
                .rounded_lg()
                .overflow_hidden()
                .border_1()
                .border_color(theme.border)
                .child(img(image).size_full()),
        )
}

fn file_card(file: &File, cx: &App) -> Div {
    let theme = cx.theme();
    let (icon, tile) = file_icon(file, cx);
    let kind = file
        .pretty_type
        .clone()
        .or_else(|| file.filetype.as_ref().map(|t| t.to_uppercase()))
        .unwrap_or_else(|| "File".to_owned());
    let meta = match file.size {
        Some(size) => format!("{kind} · {}", human_size(size)),
        None => kind,
    };
    h_flex()
        .w(px(CARD_W))
        .max_w_full()
        .items_center()
        .gap_3()
        .p_2()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .child(
            div()
                .size(px(40.))
                .flex_none()
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .bg(tile)
                .child(
                    Icon::new(icon)
                        .text_color(crate::theme::avatar_foreground())
                        .size(px(20.)),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(file_name(file)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(meta),
                ),
        )
        .child(
            Icon::new(IconName::Download)
                .text_color(theme.muted_foreground)
                .size(px(16.)),
        )
}

fn file_name(file: &File) -> String {
    file.name
        .clone()
        .or_else(|| file.title.clone())
        .unwrap_or_else(|| file.id.to_string())
}

/// Type icon and tile color: PDFs red, spreadsheets green, images, canvases, lists and the
/// rest blue.
pub(crate) fn file_icon(file: &File, cx: &App) -> (IconName, Hsla) {
    let theme = cx.theme();
    match file.filetype.as_deref().unwrap_or_default() {
        "pdf" => (IconName::FileText, theme.danger),
        "xlsx" | "xls" | "csv" | "gsheet" => (IconName::FileSpreadsheet, theme.success),
        "zip" | "gz" | "tar" => (IconName::FileArchive, theme.warning),
        "png" | "jpg" | "jpeg" | "gif" | "webp" => (IconName::FileImage, theme.link),
        // Slack's canvases are `quip` files; its lists are `list` files.
        "quip" => (IconName::StickyNote, theme.link),
        "list" => (IconName::Table2, theme.link),
        _ => (IconName::File, theme.link),
    }
}

/// Scale `(w, h)` into the thumbnail box, keeping the aspect ratio.
fn fit(width: Option<i64>, height: Option<i64>) -> (f32, f32) {
    let (w, h) = match (width, height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w as f32, h as f32),
        _ => (MAX_THUMB_W, MAX_THUMB_H),
    };
    let scale = (MAX_THUMB_W / w).min(MAX_THUMB_H / h).min(1.);
    (w * scale, h * scale)
}

/// Bytes as Slack labels them: `512 B`, `52 KB`, `2.3 MB`.
pub(crate) fn human_size(bytes: i64) -> String {
    const KB: f64 = 1024.;
    let bytes = bytes.max(0) as f64;
    if bytes < KB {
        format!("{bytes} B")
    } else if bytes < KB * KB {
        format!("{:.0} KB", bytes / KB)
    } else {
        format!("{:.1} MB", bytes / (KB * KB))
    }
}
