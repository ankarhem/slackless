//! File attachments under a message body: image thumbnails rendered from in-memory bytes
//! (gpui-kit-guide section 9) or, live, loaded from Slack's thumbnail URL through the app's
//! HTTP client, and Slack-style file cards (type tile, name, type and size) for the rest.
//! Several images show as Slack's gallery ([`Gallery`]); every image preview has Slack's
//! hover Download button.

use std::collections::HashMap;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, ClipboardItem, Div, ElementId, FontWeight, Global, Hsla, ImageSource,
    InteractiveElement as _, IntoElement, MouseButton, ObjectFit, ParentElement as _, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, canvas, div, px,
};
use slackless_core::types::{File, FileId, FileMode, Message};

use crate::fixtures::Fixtures;
use crate::image_viewer::{ImageClick, ImageOrigin};

/// Largest rendered thumbnail box (px); images scale down to fit, keeping their aspect.
const MAX_THUMB_W: f32 = 360.;
const MAX_THUMB_H: f32 = 240.;
const CARD_W: f32 = 340.;

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
    let mut column = v_flex().gap_2().pt_1();
    let images: Vec<GalleryImage> = message
        .files
        .iter()
        .enumerate()
        .filter_map(|(ix, file)| {
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
        column = column.child(if is_audio(file) {
            audio_card(file, cx)
        } else if is_video(file) {
            video_card(file, cx)
        } else if is_snippet(file) {
            // Reserve a disjoint selection document-order slice per file after the message body.
            let order_base =
                crate::messages::message_order(&message.ts) + 512_000 + (ix as u64) * 65_536;
            div().child(SnippetCard {
                file: file.clone(),
                order_base,
            })
        } else {
            match thumbnail(fixtures, file) {
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

/// A file's preview: its fixture pixels, else (live) its thumbnail url; `None` for a file
/// that is not an image.
fn thumbnail(fixtures: &Fixtures, file: &File) -> Option<ImageSource> {
    match fixtures.thumbnails.get(&file.id) {
        Some(image) => Some(ImageSource::Image(image.clone())),
        None => remote_thumbnail(file).map(|url| ImageSource::from(SharedString::from(url))),
    }
}

/// Where a file's original downloads from: Slack's download link, else its private url.
pub(crate) fn download_url(file: &File) -> Option<String> {
    file.url_private_download
        .clone()
        .or_else(|| file.url_private.clone())
}

/// One image of a [`Gallery`]: its index in `Message::files` (the viewer's origin), its
/// preview and the file.
struct GalleryImage {
    ix: usize,
    source: ImageSource,
    file: File,
}

/// Slack's file gallery (`c-file_gallery`) for a message with several images: the
/// "N attachments ▾ | Download all" row over the tiles. Below [`COMPACT_BELOW`] it is two
/// columns of square cells, each image a little narrower than its cell; wider, a grid of
/// 40px columns and 8px gutters where each image spans columns by its shape, in rows up to
/// 136px (`narrow`) or, from [`WIDE_FROM`], 232px (`wide`) tall. The layout follows the
/// width the gallery is given, measured as it lays out (the first frame assumes `wide`).
#[derive(IntoElement)]
struct Gallery {
    id: SharedString,
    images: Vec<GalleryImage>,
    on_open: Option<ImageClick>,
}

/// Slack's gallery grid: 40px columns with 8px gutters.
const COLUMN: f32 = 40.;
const GUTTER: f32 = 8.;
/// Where Slack's gallery changes layout: two 4-column (184px) cells and a gutter, and two
/// 7-column (328px) cells and a gutter.
const COMPACT_BELOW: f32 = 376.;
const WIDE_FROM: f32 = 664.;

/// Each image's `(cell width, tile width, tile height)` in a gallery `width` wide.
fn gallery_tiles(width: f32, images: &[GalleryImage]) -> Vec<(f32, f32, f32)> {
    if width < COMPACT_BELOW {
        let cell = (width - GUTTER) / 2.;
        let tile = (width / 2. - GUTTER).max(COLUMN);
        return images.iter().map(|_| (cell, tile, cell)).collect();
    }
    let (container, max_row, spans) = if width < WIDE_FROM {
        (184., 136., [2., 3., 4.])
    } else {
        (328., 232., [3., 5., 7.])
    };
    // The row fits the tallest original plus the tiles' borders and gutters, within bounds.
    let tallest = images
        .iter()
        .map(|image| image.file.original_h.unwrap_or(720) as f32)
        .fold(0., f32::max);
    let row = (tallest + 2. * (GUTTER + 1.)).clamp(COLUMN, max_row);
    images
        .iter()
        .map(|image| {
            let size = |side: Option<i64>| side.filter(|&side| side > 0).unwrap_or(40) as f32;
            let (w, h) = (size(image.file.original_w), size(image.file.original_h));
            // An image smaller than a cell spans just the columns it needs; a larger one
            // spans by shape: tall, squarish, or wide.
            let span = if h <= row || w <= container {
                ((w.clamp(COLUMN, container) + GUTTER) / (COLUMN + GUTTER)).ceil()
            } else if h / w >= 1.1 {
                spans[0]
            } else if h / w >= 0.9 {
                spans[1]
            } else {
                spans[2]
            };
            let tile = COLUMN + (COLUMN + GUTTER) * (span - 1.);
            (tile, tile, row)
        })
        .collect()
}

impl RenderOnce for Gallery {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let width =
            window.use_keyed_state(ElementId::from(self.id.clone()), cx, |_, _| None::<f32>);
        let collapsed = window.use_keyed_state(
            ElementId::from(SharedString::from(format!("{}-collapsed", self.id))),
            cx,
            |_, _| false,
        );
        let is_collapsed = *collapsed.read(cx);
        let theme = cx.theme();
        // Slack's `c-message_kit__file__meta`: 13px at 70% of the foreground.
        let meta = theme.foreground.opacity(0.7);
        let downloads: Vec<(String, String)> = self
            .images
            .iter()
            .filter_map(|image| Some((download_url(&image.file)?, file_name(&image.file))))
            .collect();
        let header = h_flex()
            .items_center()
            .text_size(px(13.))
            .text_color(meta)
            .child(
                h_flex()
                    .id(SharedString::from(format!("{}-collapse", self.id)))
                    .gap_1()
                    .items_center()
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        collapsed.update(cx, |collapsed, cx| {
                            *collapsed = !*collapsed;
                            cx.notify();
                        });
                    })
                    .child(format!("{} attachments", self.images.len()))
                    .child(div().text_xs().child(if is_collapsed {
                        "\u{25B8}"
                    } else {
                        "\u{25BE}"
                    })),
            )
            .when(!downloads.is_empty(), |row| {
                // A 1px divider, then the Download all button.
                row.child(div().ml_2().w(px(1.)).h(px(12.)).bg(theme.border))
                    .child(
                        h_flex()
                            .id(SharedString::from(format!("{}-download-all", self.id)))
                            .ml_2()
                            .px_1()
                            .gap_1()
                            .items_center()
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(|button| button.bg(theme.secondary))
                            .child(Icon::new(IconName::Download).size(px(14.)).text_color(meta))
                            .child("Download all")
                            .on_click(move |_, window, cx| {
                                for (url, name) in &downloads {
                                    crate::media::download(url.clone(), name.clone(), window, cx);
                                }
                            }),
                    )
            });
        let mut column = v_flex().relative().w_full().gap_1().child(header);
        if !is_collapsed {
            let tiles = gallery_tiles(width.read(cx).unwrap_or(WIDE_FROM), &self.images);
            let row = h_flex()
                .flex_wrap()
                .gap_2()
                .children(self.images.iter().zip(tiles).map(|(image, (cell, w, h))| {
                    div().flex_none().w(px(cell)).h(px(h)).child(image_tile(
                        image,
                        w,
                        h,
                        self.on_open.as_ref(),
                        cx,
                    ))
                }));
            column = column.child(row);
        }
        // The width the gallery lays out at; a change lays the tiles out again.
        let measure = canvas(
            move |bounds, _, cx| {
                let laid_out = f32::from(bounds.size.width);
                if width
                    .read(cx)
                    .is_none_or(|known| (known - laid_out).abs() >= 0.5)
                {
                    cx.defer(move |cx| {
                        width.update(cx, |known, cx| {
                            *known = Some(laid_out);
                            cx.notify();
                        });
                    });
                }
            },
            |_, (), _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .w_full()
        .h(px(0.));
        column.child(measure)
    }
}

/// One gallery tile: the image cropped to fill it (`object-fit: cover`) in Slack's 8px-radius
/// frame, opening the viewer on click, with the hover Download button.
fn image_tile(
    image: &GalleryImage,
    w: f32,
    h: f32,
    on_open: Option<&ImageClick>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let file = &image.file;
    let group = SharedString::from(format!("file-tile-{}", file.id));
    div()
        .id(SharedString::from(format!("file-open-{}", file.id)))
        .group(group.clone())
        .relative()
        .w(px(w))
        .h(px(h))
        .rounded_lg()
        .overflow_hidden()
        .border_1()
        .border_color(theme.foreground.opacity(0.13))
        .when_some(on_open.cloned(), |tile, on_open| {
            let origin = ImageOrigin::File(image.ix);
            tile.cursor_pointer()
                .on_click(move |_, window, cx| on_open(origin.clone(), window, cx))
        })
        .child(
            crate::media::Picture::new(image.source.clone())
                .fit(ObjectFit::Cover)
                .radius(px(8.)),
        )
        .children(download_button(file, group, cx))
        .into_any_element()
}

/// Slack's hover action on an image preview: a Download button in its top-right corner,
/// saving the original into ~/Downloads.
fn download_button(file: &File, group: SharedString, cx: &App) -> Option<AnyElement> {
    let url = download_url(file)?;
    let name = file_name(file);
    let theme = cx.theme();
    Some(
        div()
            .id(SharedString::from(format!("file-download-{}", file.id)))
            .absolute()
            .top(px(8.))
            .right(px(8.))
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .opacity(0.)
            .group_hover(group, |button| button.opacity(1.))
            .cursor_pointer()
            .hover(|button| button.bg(theme.secondary))
            .child(
                Icon::new(IconName::Download)
                    .size(px(16.))
                    .text_color(theme.foreground),
            )
            // The press is the button's, not the image's (which opens the viewer): a click
            // fires on the elements its mouse-down reached.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| {
                crate::media::download(url.clone(), name.clone(), window, cx);
            })
            .into_any_element(),
    )
}

/// A live image's thumbnail URL, sized for the inline preview on a 2x display; for a GIF its
/// animated rendition, which Slack plays inline.
fn remote_thumbnail(file: &File) -> Option<String> {
    if !file
        .mimetype
        .as_deref()
        .is_some_and(|mime| mime.starts_with("image/"))
    {
        return None;
    }
    file.thumb_480_gif
        .clone()
        .or_else(|| file.thumb_360_gif.clone())
        .or_else(|| file.thumb_480.clone())
        .or_else(|| file.thumb_360.clone())
        .or_else(|| file.thumb_720.clone())
}

fn image_attachment(
    file: &File,
    ix: usize,
    image: ImageSource,
    on_open: Option<&ImageClick>,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let (w, h) = fit(file.original_w, file.original_h);
    // Slack's `.p-file_image_thumbnail__wrapper`: an 8px-radius frame with a 1px border of
    // the foreground at 13% and the zoom cursor; clicking opens the image viewer.
    let openable = on_open.is_some();
    let group = SharedString::from(format!("file-tile-{}", file.id));
    let mut frame = div()
        .id(SharedString::from(format!("file-open-{}", file.id)))
        .group(group.clone())
        .relative()
        .w(px(w))
        .h(px(h))
        .rounded_lg()
        .overflow_hidden()
        .border_1()
        .border_color(if openable {
            theme.foreground.opacity(0.13)
        } else {
            theme.border
        });
    if let Some(on_open) = on_open {
        let on_open = on_open.clone();
        let origin = ImageOrigin::File(ix);
        frame = frame
            .cursor_pointer()
            .on_click(move |_, window, cx| on_open(origin.clone(), window, cx));
    }
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
            frame
                .child(crate::media::Picture::new(image).radius(px(8.)))
                .children(download_button(file, group, cx)),
        )
}

fn is_audio(file: &File) -> bool {
    if file
        .mimetype
        .as_deref()
        .is_some_and(|m| m.starts_with("audio/"))
    {
        return true;
    }
    matches!(
        file.filetype.as_deref().unwrap_or_default(),
        "mp3" | "m4a" | "wav" | "aac" | "ogg" | "flac" | "opus"
    )
}

fn is_video(file: &File) -> bool {
    if file
        .mimetype
        .as_deref()
        .is_some_and(|m| m.starts_with("video/"))
    {
        return true;
    }
    matches!(
        file.filetype.as_deref().unwrap_or_default(),
        "mp4" | "mov" | "webm" | "mkv" | "avi" | "m4v"
    )
}

fn media_duration_ms(file: &File) -> Option<u64> {
    file.extra
        .get("duration_ms")
        .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
        .filter(|&ms| ms > 0)
}

fn format_duration(ms: u64) -> String {
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{mins}:{secs:02}")
}

fn file_ext(file: &File, default: &str) -> String {
    if let Some(ft) = file.filetype.as_deref().filter(|s| !s.is_empty()) {
        return ft.to_owned();
    }
    file.name
        .as_deref()
        .and_then(|n| n.rsplit_once('.').map(|(_, ext)| ext.to_owned()))
        .unwrap_or_else(|| default.to_owned())
}

/// Slack-style inline audio player card for uploaded audio files and voice clips.
fn audio_card(file: &File, cx: &App) -> Div {
    let theme = cx.theme();
    let id_str = file.id.to_string();
    let state = crate::media::audio_state(&id_str, cx);
    let playing = state.as_ref().is_some_and(|s| s.playing);
    let paused = state.as_ref().is_some_and(|s| s.paused);
    let loading = state.as_ref().is_some_and(|s| s.loading);
    let duration_ms = state
        .as_ref()
        .and_then(|s| s.duration_ms)
        .or_else(|| media_duration_ms(file));
    let elapsed_ms = state.as_ref().map(|s| s.elapsed_ms).unwrap_or(0);
    let progress = match duration_ms {
        Some(total) if total > 0 && (playing || paused) => {
            (elapsed_ms as f32 / total as f32).clamp(0., 1.)
        }
        _ => 0.,
    };
    let status_text = if loading {
        "Loading audio\u{2026}".to_owned()
    } else if playing || paused {
        let prefix = if paused { "Paused \u{b7} " } else { "" };
        match duration_ms {
            Some(total) => format!(
                "{prefix}{} / {}",
                format_duration(elapsed_ms),
                format_duration(total)
            ),
            None => format!("{prefix}{}", format_duration(elapsed_ms)),
        }
    } else {
        let kind = file
            .pretty_type
            .clone()
            .or_else(|| file.filetype.as_ref().map(|t| t.to_uppercase()))
            .unwrap_or_else(|| "Audio".to_owned());
        match (duration_ms, file.size) {
            (Some(total), Some(size)) => {
                format!(
                    "{} \u{b7} {kind} \u{b7} {}",
                    format_duration(total),
                    human_size(size)
                )
            }
            (Some(total), None) => format!("{} \u{b7} {kind}", format_duration(total)),
            (None, Some(size)) => format!("{kind} \u{b7} {}", human_size(size)),
            (None, None) => kind,
        }
    };
    let play_url = file
        .url_private
        .clone()
        .or_else(|| file.url_private_download.clone());
    let dl_url = download_url(file);
    let name = file_name(file);
    let ext = file_ext(file, "m4a");
    let play_icon = if playing {
        IconName::Pause
    } else {
        IconName::Play
    };
    let stop_btn = (playing || paused).then(|| {
        div()
            .id(SharedString::from(format!("audio-stop-{}", file.id)))
            .size(px(28.))
            .flex_none()
            .rounded_sm()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|b| b.bg(theme.secondary_hover))
            .child(
                Icon::new(IconName::Square)
                    .text_color(theme.muted_foreground)
                    .size(px(13.)),
            )
            .on_click(move |_, window, cx| {
                crate::media::stop_audio(window, cx);
            })
    });
    let mut play_btn = div()
        .id(SharedString::from(format!("audio-play-{}", file.id)))
        .size(px(36.))
        .flex_none()
        .rounded_full()
        .bg(theme.primary)
        .flex()
        .items_center()
        .justify_center()
        .child(
            Icon::new(play_icon)
                .text_color(theme.primary_foreground)
                .size(px(16.)),
        );
    if let Some(url) = play_url {
        play_btn = play_btn
            .cursor_pointer()
            .hover(|b| b.bg(theme.primary_hover))
            .on_click(move |_, window, cx| {
                crate::media::toggle_audio(
                    id_str.clone(),
                    url.clone(),
                    ext.clone(),
                    duration_ms,
                    window,
                    cx,
                );
            });
    }
    let dl_btn = dl_url.map(|url| {
        div()
            .id(SharedString::from(format!("audio-dl-{}", file.id)))
            .size(px(28.))
            .flex_none()
            .rounded_sm()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|b| b.bg(theme.secondary_hover))
            .child(
                Icon::new(IconName::Download)
                    .text_color(theme.muted_foreground)
                    .size(px(15.)),
            )
            .on_click(move |_, window, cx| {
                crate::media::download(url.clone(), name.clone(), window, cx);
            })
    });
    h_flex()
        .w(px(CARD_W))
        .max_w_full()
        .items_center()
        .gap_3()
        .p_2p5()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .child(play_btn)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(file_name(file)),
                )
                .when(playing || duration_ms.is_some(), |col| {
                    col.child(
                        div()
                            .w_full()
                            .h(px(4.))
                            .rounded_full()
                            .bg(theme.border)
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(px((220. * progress).max(if playing { 6. } else { 0. })))
                                    .rounded_full()
                                    .bg(theme.primary),
                            ),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(status_text),
                ),
        )
        .children(stop_btn)
        .children(dl_btn)
}

/// Slack-style inline video card with poster thumbnail, centered play trigger, duration badge,
/// and hover download button.
fn video_card(file: &File, cx: &App) -> Div {
    let theme = cx.theme();
    let id_str = file.id.to_string();
    let loading = crate::media::video_loading(&id_str, cx);
    let (w, h) = fit(file.original_w, file.original_h);
    let poster_url = file
        .thumb_video
        .clone()
        .or_else(|| file.thumb_480.clone())
        .or_else(|| file.thumb_360.clone())
        .or_else(|| file.thumb_720.clone());
    let video_url = file
        .extra
        .get("mp4")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .or_else(|| file.url_private.clone())
        .or_else(|| file.url_private_download.clone());
    let ext = file_ext(file, "mp4");
    let duration = media_duration_ms(file).map(format_duration);
    let group = SharedString::from(format!("video-tile-{}", file.id));
    let mut frame = div()
        .id(SharedString::from(format!("video-open-{}", file.id)))
        .group(group.clone())
        .relative()
        .w(px(w))
        .h(px(h))
        .rounded_lg()
        .overflow_hidden()
        .border_1()
        .border_color(theme.foreground.opacity(0.13))
        .bg(theme.secondary);
    if let Some(url) = video_url {
        frame = frame.cursor_pointer().on_click(move |_, window, cx| {
            crate::media::play_video(id_str.clone(), url.clone(), ext.clone(), window, cx);
        });
    }
    if let Some(poster) = poster_url {
        frame = frame.child(
            crate::media::Picture::new(ImageSource::from(SharedString::from(poster)))
                .fit(ObjectFit::Cover)
                .radius(px(8.)),
        );
    }
    let play_badge = div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .size(px(48.))
                .rounded_full()
                .bg(gpui_kit::gpui::hsla(0., 0., 0., 0.62))
                .border_1()
                .border_color(gpui_kit::gpui::hsla(0., 0., 1., 0.25))
                .flex()
                .items_center()
                .justify_center()
                .child(if loading {
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(gpui_kit::gpui::hsla(0., 0., 1., 0.95))
                        .child("\u{2026}")
                        .into_any_element()
                } else {
                    Icon::new(IconName::Play)
                        .size(px(22.))
                        .text_color(gpui_kit::gpui::hsla(0., 0., 1., 0.95))
                        .into_any_element()
                }),
        );
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
            frame
                .child(play_badge)
                .when_some(duration, |el, dur| {
                    el.child(
                        div()
                            .absolute()
                            .bottom(px(8.))
                            .right(px(8.))
                            .px_1p5()
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(gpui_kit::gpui::hsla(0., 0., 0., 0.7))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(gpui_kit::gpui::hsla(0., 0., 1., 0.95))
                            .child(dur),
                    )
                })
                .children(download_button(file, group, cx)),
        )
}

const SNIPPET_COLLAPSED_LINES: usize = 5;
/// Maximum lines rendered inline when a snippet is expanded: high enough to show full files like
/// Slack does inline (e.g. 252-line configs) while keeping multi-thousand-line files responsive.
const SNIPPET_MAX_EXPANDED_LINES: usize = 1_000;

/// `(visible_count, omitted_lines_when_expanded)` for a snippet card.
fn snippet_visible_and_omitted(total_loaded: usize, expanded: bool) -> (usize, usize) {
    if !expanded {
        return (total_loaded.min(SNIPPET_COLLAPSED_LINES), 0);
    }
    let visible = total_loaded.min(SNIPPET_MAX_EXPANDED_LINES);
    (visible, total_loaded.saturating_sub(visible))
}

fn extra_usize(file: &File, key: &str) -> Option<usize> {
    let val = file.extra.get(key)?;
    val.as_u64()
        .map(|n| n as usize)
        .or_else(|| val.as_str().and_then(|s| s.parse::<usize>().ok()))
}

fn text_lines(text: &str) -> Vec<String> {
    text.trim_end_matches(['\n', '\r'])
        .lines()
        .map(str::to_owned)
        .collect()
}

fn is_snippet(file: &File) -> bool {
    let has_preview = file
        .preview
        .as_deref()
        .is_some_and(|p| !p.trim().is_empty());
    has_preview
        && (file.mode == Some(FileMode::Snippet)
            || (!is_audio(file)
                && !is_video(file)
                && remote_thumbnail(file).is_none()
                && !matches!(file.filetype.as_deref(), Some("quip" | "list" | "pdf"))))
}

fn snippet_lines(file: &File) -> Vec<String> {
    text_lines(file.preview.as_deref().unwrap_or_default())
}

/// Whether Slack truncated `file.preview` on the server (typically to 5 lines), so expanding or
/// copying needs to fetch the full file text from `url_private`.
fn snippet_is_truncated(file: &File, preview_len: usize) -> bool {
    let flagged = file
        .extra
        .get("preview_is_truncated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let lines_more = extra_usize(file, "lines_more").unwrap_or(0);
    let server_total = extra_usize(file, "lines").unwrap_or(0);
    flagged || lines_more > 0 || server_total > preview_len
}

/// Extra lines beyond the collapsed 5-line preview: uses Slack's `lines` / `lines_more` metadata
/// when available so truncated previews report the real line count (`Show more (247 lines)`).
fn snippet_hidden_lines(file: &File, loaded_len: usize, has_full_text: bool) -> usize {
    let from_loaded = loaded_len.saturating_sub(SNIPPET_COLLAPSED_LINES);
    if has_full_text {
        return from_loaded;
    }
    let from_total = extra_usize(file, "lines")
        .unwrap_or(0)
        .saturating_sub(SNIPPET_COLLAPSED_LINES);
    let from_more = extra_usize(file, "lines_more").unwrap_or(0);
    from_loaded.max(from_total).max(from_more)
}

#[derive(Clone, Default)]
struct SnippetCardState {
    expanded: bool,
    copied: bool,
    loading: bool,
    full_text: Option<String>,
    error: Option<String>,
}

/// Persistent per-file snippet state stored as a GPUI [`Global`] so expanded snippets and
/// fetched full-file text survive when virtualized [`crate::messages::MessageList`] rows scroll
/// out of view and back in.
#[derive(Default)]
struct SnippetStore {
    entries: HashMap<FileId, SnippetCardState>,
}

impl Global for SnippetStore {}

/// Slack's collapsible line-numbered code snippet card (`Chalk_Custom.terminal`, etc.):
/// shows 5 lines collapsed with a centered `Show more (N lines)` toggle plus Copy & Download.
/// For server-truncated Slack previews (`preview_is_truncated` / `lines_more > 0`), lazily fetches
/// the full file text via [`crate::media::fetch`] on expand or Copy. Code lines participate in
/// window text selection (without selecting line numbers).
#[derive(IntoElement)]
struct SnippetCard {
    file: File,
    order_base: u64,
}

impl RenderOnce for SnippetCard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let file = self.file;
        let order_base = self.order_base;
        let file_id = file.id.clone();
        let id_str = file.id.to_string();
        let SnippetCardState {
            expanded,
            copied,
            loading,
            full_text,
            error,
        } = cx
            .try_global::<SnippetStore>()
            .and_then(|store| store.entries.get(&file_id))
            .cloned()
            .unwrap_or_default();
        let theme = cx.theme();
        let preview_lines = snippet_lines(&file);
        let is_truncated = snippet_is_truncated(&file, preview_lines.len());
        let has_full_text = full_text.is_some();
        let lines = match &full_text {
            Some(full) => text_lines(full),
            None => preview_lines,
        };
        let hidden_lines = snippet_hidden_lines(&file, lines.len(), has_full_text);
        let can_expand = lines.len() > SNIPPET_COLLAPSED_LINES || hidden_lines > 0 || is_truncated;
        let (visible_count, omitted_lines) = snippet_visible_and_omitted(lines.len(), expanded);
        let gutter_digits = visible_count.max(1).to_string().len().max(2);
        let title = file
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| file_name(&file));
        let kind = file
            .pretty_type
            .clone()
            .or_else(|| file.filetype.as_ref().map(|t| t.to_uppercase()));
        let dl_url = download_url(&file);
        let fetch_url = file.url_private.clone().or_else(|| dl_url.clone());
        let name = file_name(&file);
        let fallback_copy = full_text
            .clone()
            .or_else(|| file.preview.clone())
            .unwrap_or_default();
        let code_bg = crate::theme::code_block_background(cx);
        let border = theme.border;
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let mono = theme.mono_font_family.clone();
        let secondary = theme.secondary;
        let secondary_hover = theme.secondary_hover;
        let success = theme.success;

        let copy_file_id = file_id.clone();
        let copy_fetch_url = fetch_url.clone();
        let needs_copy_fetch = is_truncated && !has_full_text;
        let copy_btn = h_flex()
            .id(SharedString::from(format!("snippet-copy-{id_str}")))
            .items_center()
            .gap_1()
            .px_1p5()
            .py(px(2.))
            .rounded_sm()
            .text_xs()
            .text_color(if copied { success } else { muted })
            .cursor_pointer()
            .hover(move |b| b.bg(secondary_hover).text_color(fg))
            .child(
                Icon::new(if copied {
                    IconName::Check
                } else {
                    IconName::Copy
                })
                .size(px(12.))
                .text_color(if copied { success } else { muted }),
            )
            .child(if copied { "Copied" } else { "Copy" })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| {
                if needs_copy_fetch {
                    if let Some(fut) = copy_fetch_url
                        .clone()
                        .and_then(|url| crate::media::fetch(url, cx))
                    {
                        let fid = copy_file_id.clone();
                        let fallback = fallback_copy.clone();
                        window
                            .spawn(cx, async move |cx| {
                                let res = fut.await;
                                let _ = cx.update(|window, cx| {
                                    let text = match res {
                                        Ok(bytes) => {
                                            let decoded =
                                                String::from_utf8_lossy(&bytes).into_owned();
                                            let entry = cx
                                                .default_global::<SnippetStore>()
                                                .entries
                                                .entry(fid.clone())
                                                .or_default();
                                            entry.full_text = Some(decoded.clone());
                                            entry.loading = false;
                                            entry.error = None;
                                            decoded
                                        }
                                        Err(_) => fallback,
                                    };
                                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                                    cx.default_global::<SnippetStore>()
                                        .entries
                                        .entry(fid)
                                        .or_default()
                                        .copied = true;
                                    window.refresh();
                                });
                            })
                            .detach();
                        return;
                    }
                }
                cx.write_to_clipboard(ClipboardItem::new_string(fallback_copy.clone()));
                cx.default_global::<SnippetStore>()
                    .entries
                    .entry(copy_file_id.clone())
                    .or_default()
                    .copied = true;
                window.refresh();
            });

        let dl_btn = dl_url.map(|url| {
            let name = name.clone();
            h_flex()
                .id(SharedString::from(format!("snippet-dl-{id_str}")))
                .items_center()
                .gap_1()
                .px_1p5()
                .py(px(2.))
                .rounded_sm()
                .text_xs()
                .text_color(muted)
                .cursor_pointer()
                .hover(move |b| b.bg(secondary_hover).text_color(fg))
                .child(
                    Icon::new(IconName::Download)
                        .size(px(12.))
                        .text_color(muted),
                )
                .child("Download")
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    crate::media::download(url.clone(), name.clone(), window, cx);
                })
        });

        let header = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_1p5()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::FileCode)
                            .size(px(14.))
                            .text_color(muted),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .child(title),
                    )
                    .when_some(kind, |row, k| {
                        row.child(div().text_xs().text_color(muted).child(k))
                    }),
            )
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .child(copy_btn)
                    .children(dl_btn),
            );

        let line_key = format!("snippet-{id_str}");
        let sample = lines
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        let lang = crate::syntax::Lang::detect(
            file.filetype.as_deref(),
            file.name.as_deref().or(file.title.as_deref()),
            &sample,
        );
        let mut code_body = v_flex()
            .w_full()
            .px_2p5()
            .py_2()
            .font_family(mono)
            .text_size(px(12.))
            .line_height(px(18.));
        for (ix, line) in lines.iter().take(visible_count).enumerate() {
            let num = format!("{:>width$}", ix + 1, width = gutter_digits);
            let line_el: AnyElement = if line.is_empty() {
                div().flex_1().min_w_0().child(" ").into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_w_0()
                    .child(crate::messages::selectable_code(
                        &line_key,
                        ix,
                        line.clone(),
                        lang,
                        order_base + ix as u64,
                        cx,
                    ))
                    .into_any_element()
            };
            code_body = code_body.child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_start()
                    .gap_3()
                    .child(div().flex_none().text_color(muted.opacity(0.65)).child(num))
                    .child(line_el),
            );
        }
        if expanded && loading {
            code_body = code_body.child(
                div()
                    .pt_1()
                    .text_xs()
                    .text_color(muted)
                    .child("Loading full snippet\u{2026}"),
            );
        } else if expanded {
            if let Some(err) = error.as_ref() {
                code_body =
                    code_body.child(div().pt_1().text_xs().text_color(muted).child(err.clone()));
            }
        }
        let truncation_notice = if expanded && !loading && omitted_lines > 0 {
            Some(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_1p5()
                    .px_3()
                    .py_1p5()
                    .border_t_1()
                    .border_color(border)
                    .bg(secondary.opacity(0.5))
                    .text_xs()
                    .text_color(muted)
                    .child(Icon::new(IconName::Info).size(px(12.)).text_color(muted))
                    .child(format!(
                        "Showing first {visible_count} of {} lines ({omitted_lines} more not shown) \u{2014} copy or download for the full file.",
                        lines.len()
                    )),
            )
        } else if expanded && !loading && is_truncated && !has_full_text && error.is_none() {
            let total = lines.len() + hidden_lines;
            Some(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_1p5()
                    .px_3()
                    .py_1p5()
                    .border_t_1()
                    .border_color(border)
                    .bg(secondary.opacity(0.5))
                    .text_xs()
                    .text_color(muted)
                    .child(Icon::new(IconName::Info).size(px(12.)).text_color(muted))
                    .child(format!(
                        "Showing preview ({visible_count} of {total} lines) \u{2014} download for the full file."
                    )),
            )
        } else {
            None
        };

        let toggle_row = can_expand.then(|| {
            let toggle_file_id = file_id.clone();
            let expand_fetch_url = fetch_url.clone();
            let label = if expanded {
                "Show less".to_owned()
            } else if hidden_lines > 0 {
                format!(
                    "Show more ({hidden_lines} {})",
                    if hidden_lines == 1 { "line" } else { "lines" }
                )
            } else {
                "Show more".to_owned()
            };
            h_flex()
                .w_full()
                .justify_center()
                .py_1p5()
                .border_t_1()
                .border_color(border)
                .child(
                    div()
                        .id(SharedString::from(format!("snippet-toggle-{id_str}")))
                        .px_2p5()
                        .py(px(2.))
                        .rounded(px(999.))
                        .bg(secondary)
                        .border_1()
                        .border_color(border)
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg)
                        .cursor_pointer()
                        .hover(move |b| b.bg(secondary_hover))
                        .child(label)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, window, cx| {
                            let should_fetch = {
                                let entry = cx
                                    .default_global::<SnippetStore>()
                                    .entries
                                    .entry(toggle_file_id.clone())
                                    .or_default();
                                entry.expanded = !entry.expanded;
                                let fetch = entry.expanded
                                    && is_truncated
                                    && entry.full_text.is_none()
                                    && !entry.loading;
                                if fetch {
                                    entry.error = None;
                                }
                                fetch
                            };
                            window.refresh();
                            if should_fetch {
                                if let Some(fut) = expand_fetch_url
                                    .clone()
                                    .and_then(|url| crate::media::fetch(url, cx))
                                {
                                    cx.default_global::<SnippetStore>()
                                        .entries
                                        .entry(toggle_file_id.clone())
                                        .or_default()
                                        .loading = true;
                                    window.refresh();
                                    let fid = toggle_file_id.clone();
                                    window
                                        .spawn(cx, async move |cx| {
                                            let res = fut.await;
                                            let _ = cx.update(|window, cx| {
                                                let entry = cx
                                                    .default_global::<SnippetStore>()
                                                    .entries
                                                    .entry(fid)
                                                    .or_default();
                                                entry.loading = false;
                                                match res {
                                                    Ok(bytes) => {
                                                        entry.full_text = Some(
                                                            String::from_utf8_lossy(&bytes)
                                                                .into_owned(),
                                                        );
                                                        entry.error = None;
                                                    }
                                                    Err(err) => {
                                                        entry.error = Some(format!(
                                                            "Could not load full snippet ({err})."
                                                        ));
                                                    }
                                                }
                                                window.refresh();
                                            });
                                        })
                                        .detach();
                                } else {
                                    cx.default_global::<SnippetStore>()
                                        .entries
                                        .entry(toggle_file_id.clone())
                                        .or_default()
                                        .error = Some(
                                        "Full snippet requires a live Slack session (showing preview)."
                                            .to_owned(),
                                    );
                                    window.refresh();
                                }
                            }
                        }),
                )
        });

        v_flex()
            .w_full()
            .max_w(px(720.))
            .rounded(px(6.))
            .border_1()
            .border_color(border)
            .bg(code_bg)
            .overflow_hidden()
            .child(header)
            .child(code_body)
            .children(truncation_notice)
            .children(toggle_row)
    }
}

/// Human-readable provider label for an external Slack file (`F-COLLAB-586`).
fn external_provider_label(file: &File) -> String {
    match file
        .external_type
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "gdrive" => "Google Drive".to_owned(),
        "dropbox" => "Dropbox".to_owned(),
        "box" => "Box".to_owned(),
        "onedrive" | "sharepoint" => "OneDrive".to_owned(),
        "notion" => "Notion".to_owned(),
        "figma" => "Figma".to_owned(),
        "jira" => "Jira".to_owned(),
        "github" => "GitHub".to_owned(),
        other if !other.is_empty() => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => "External file".to_owned(),
            }
        }
        _ => "External file".to_owned(),
    }
}

/// Compact external file title preview card (`F-COLLAB-586`): shows provider badge, clickable
/// title, and an open-in-browser affordance.
fn external_file_card(file: &File, cx: &App) -> Div {
    let theme = cx.theme();
    let (icon, tile) = file_icon(file, cx);
    let provider = external_provider_label(file);
    let kind = file
        .pretty_type
        .clone()
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case(&provider));
    let subtitle = match kind {
        Some(k) => format!("{provider} \u{b7} {k}"),
        None => provider,
    };
    let open_url = file
        .permalink
        .clone()
        .or_else(|| file.url_private.clone())
        .or_else(|| file.permalink_public.clone());
    let mut card = h_flex()
        .id(SharedString::from(format!("ext-file-card-{}", file.id)))
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
                .size(px(36.))
                .flex_none()
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .bg(tile)
                .child(
                    Icon::new(icon)
                        .text_color(crate::theme::avatar_foreground())
                        .size(px(18.)),
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
                        .text_color(theme.link)
                        .child(file_name(file)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(subtitle),
                ),
        )
        .child(
            Icon::new(IconName::ExternalLink)
                .text_color(theme.muted_foreground)
                .size(px(15.)),
        );
    if let Some(url) = open_url {
        card = card
            .cursor_pointer()
            .hover(|c| c.bg(theme.secondary))
            .on_click(move |_, _, cx| cx.open_url(&url));
    }
    div().child(card)
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
    let dl_url = download_url(file);
    let name = file_name(file);
    let card = h_flex()
        .id(SharedString::from(format!("file-card-{}", file.id)))
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
        );
    let save_as = dl_url.clone().map(|url| {
        let name = name.clone();
        div()
            .id(SharedString::from(format!("file-saveas-{}", file.id)))
            .px_1p5()
            .py_0p5()
            .rounded_sm()
            .text_xs()
            .text_color(theme.muted_foreground)
            .cursor_pointer()
            .hover(|b| b.bg(theme.secondary_hover).text_color(theme.foreground))
            .child("Save as\u{2026}")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| {
                crate::media::download_as(url.clone(), name.clone(), window, cx);
            })
    });
    let card = card.children(save_as).child(
        Icon::new(IconName::Download)
            .text_color(theme.muted_foreground)
            .size(px(16.)),
    );
    let card = match dl_url {
        Some(url) => card
            .cursor_pointer()
            .hover(|c| c.bg(theme.secondary))
            .on_click(move |_, window, cx| {
                crate::media::download(url.clone(), name.clone(), window, cx);
            }),
        None => card,
    };
    div().child(card)
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
        "mp4" | "mov" | "webm" | "mkv" | "avi" | "m4v" => (IconName::Video, theme.link),
        "mp3" | "m4a" | "wav" | "aac" | "ogg" | "flac" | "opus" => (IconName::Mic, theme.link),
        // Slack's canvases are `quip` files; its lists are `list` files.
        "quip" => (IconName::StickyNote, theme.link),
        "list" => (IconName::Table2, theme.link),
        _ => (IconName::File, theme.link),
    }
}

/// Scale `(w, h)` into the thumbnail box, keeping the aspect ratio; smaller images keep
/// their size.
pub(crate) fn fit(width: Option<i64>, height: Option<i64>) -> (f32, f32) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockkit::serde_json::json;
    use gpui_kit::gpui::{self, Render, TestAppContext};
    use slackless_core::types::FileId;

    #[test]
    fn snippet_detection_truncation_and_line_counts_match_slack() {
        // Untruncated 7-line preview.
        let snippet = File {
            id: FileId::from("F0SNIP"),
            name: Some("Chalk_Custom.terminal".into()),
            mode: Some(FileMode::Snippet),
            filetype: Some("xml".into()),
            preview: Some("line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\n".into()),
            ..Default::default()
        };
        assert!(is_snippet(&snippet));
        let lines = snippet_lines(&snippet);
        assert_eq!(lines.len(), 7);
        assert!(!snippet_is_truncated(&snippet, lines.len()));
        assert_eq!(snippet_hidden_lines(&snippet, lines.len(), false), 2);

        // Server-truncated Slack snippet: only 5 preview lines, `lines: 252`, `lines_more: 247`.
        let mut truncated = File {
            id: FileId::from("F0TRUNC"),
            name: Some("Chalk.terminal".into()),
            mode: Some(FileMode::Snippet),
            filetype: Some("xml".into()),
            preview: Some("line 1\nline 2\nline 3\nline 4\nline 5\n".into()),
            ..Default::default()
        };
        truncated
            .extra
            .insert("preview_is_truncated".into(), json!(true));
        truncated.extra.insert("lines".into(), json!(252));
        truncated.extra.insert("lines_more".into(), json!(247));
        let trunc_lines = snippet_lines(&truncated);
        assert_eq!(trunc_lines.len(), 5);
        assert!(snippet_is_truncated(&truncated, trunc_lines.len()));
        assert_eq!(
            snippet_hidden_lines(&truncated, trunc_lines.len(), false),
            247
        );
        // Once full text is fetched (say 252 lines), hidden lines computes from loaded text.
        assert_eq!(snippet_hidden_lines(&truncated, 252, true), 247);

        // String-encoded `lines_more` without `lines` or `preview_is_truncated` still triggers expansion.
        let mut string_meta = File {
            id: FileId::from("F0STR"),
            mode: Some(FileMode::Snippet),
            preview: Some("a\nb\nc\nd\ne\n".into()),
            ..Default::default()
        };
        string_meta.extra.insert("lines_more".into(), json!("19"));
        assert!(snippet_is_truncated(&string_meta, 5));
        assert_eq!(snippet_hidden_lines(&string_meta, 5, false), 19);

        // Short 3-line snippet is NOT truncated and has 0 hidden lines.
        let short = File {
            id: FileId::from("F0SHORT"),
            mode: Some(FileMode::Snippet),
            preview: Some("a\nb\nc\n".into()),
            ..Default::default()
        };
        assert!(!snippet_is_truncated(&short, 3));
        assert_eq!(snippet_hidden_lines(&short, 3, false), 0);

        // Non-snippet canvas / PDF files must not render as code snippet cards.
        let canvas = File {
            id: FileId::from("F0CANV"),
            filetype: Some("quip".into()),
            preview: Some("hello".into()),
            ..Default::default()
        };
        assert!(!is_snippet(&canvas));

        // Expanding a 252-line file shows the entire file inline; files beyond the inline cap
        // report the exact omitted line count so the UI tells the user it was truncated.
        assert_eq!(snippet_visible_and_omitted(252, false), (5, 0));
        assert_eq!(snippet_visible_and_omitted(252, true), (252, 0));
        assert_eq!(
            snippet_visible_and_omitted(SNIPPET_MAX_EXPANDED_LINES + 500, true),
            (SNIPPET_MAX_EXPANDED_LINES, 500)
        );
    }

    #[gpui::test]
    fn snippet_card_renders_and_persists_expansion_across_unmount(cx: &mut TestAppContext) {
        struct Host {
            visible: bool,
            files: Vec<File>,
        }
        impl Render for Host {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                v_flex()
                    .size_full()
                    .gap_4()
                    .p_4()
                    .when(self.visible, |col| {
                        col.children(self.files.iter().enumerate().map(|(ix, file)| SnippetCard {
                            file: file.clone(),
                            order_base: (ix as u64 + 1) * 1024,
                        }))
                    })
            }
        }

        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });

        let mut truncated = File {
            id: FileId::from("F0TRUNC"),
            name: Some("Chalk_Custom.terminal".into()),
            title: Some("Chalk_Custom.terminal".into()),
            mode: Some(FileMode::Snippet),
            pretty_type: Some("Plain Text".into()),
            preview: Some("line 1\nline 2\nline 3\nline 4\nline 5\n".into()),
            ..Default::default()
        };
        truncated
            .extra
            .insert("preview_is_truncated".into(), json!(true));
        truncated.extra.insert("lines".into(), json!(252));
        truncated.extra.insert("lines_more".into(), json!(247));

        let untruncated = File {
            id: FileId::from("F0FULL"),
            name: Some("deploy.sh".into()),
            title: Some("deploy.sh".into()),
            mode: Some(FileMode::Snippet),
            pretty_type: Some("Shell".into()),
            preview: Some("1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n".into()),
            ..Default::default()
        };

        let huge_text: String = (1..=SNIPPET_MAX_EXPANDED_LINES + 50)
            .map(|n| format!("line {n}\n"))
            .collect();
        let huge = File {
            id: FileId::from("F0HUGE"),
            name: Some("dump.log".into()),
            title: Some("dump.log".into()),
            mode: Some(FileMode::Snippet),
            pretty_type: Some("Plain Text".into()),
            preview: Some(huge_text),
            ..Default::default()
        };

        let (view, cx) = cx.add_window_view(|_, _| Host {
            visible: true,
            files: vec![truncated, untruncated, huge],
        });
        cx.update(|window, cx| {
            // Simulate expanding `F0TRUNC` with fetched full text cached in `SnippetStore`,
            // and expanding `F0HUGE` (> 1,000 lines) to exercise the truncation notice banner.
            let entries = &mut cx.default_global::<SnippetStore>().entries;
            let entry = entries.entry(FileId::from("F0TRUNC")).or_default();
            entry.expanded = true;
            entry.full_text = Some("line 1\nline 2\nline 3\nline 4\nline 5\nline 6\n".into());
            entries.entry(FileId::from("F0HUGE")).or_default().expanded = true;
            let _ = window.draw(cx);
        });

        // Unmount the snippet cards completely (simulating scrolling out of virtualized list view).
        view.update(cx, |host, cx| {
            host.visible = false;
            cx.notify();
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        // Remount (scroll back into view) and confirm expanded state & cached full text survived.
        view.update(cx, |host, cx| {
            host.visible = true;
            cx.notify();
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let stored = cx
                .global::<SnippetStore>()
                .entries
                .get(&FileId::from("F0TRUNC"))
                .expect("snippet state must survive unmount/scroll-out");
            assert!(stored.expanded);
            assert!(
                stored
                    .full_text
                    .as_deref()
                    .is_some_and(|t| t.contains("line 6"))
            );
        });
        cx.run_until_parked();
    }
}
