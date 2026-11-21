//! File attachments under a message body: image thumbnails rendered from in-memory bytes
//! (gpui-kit-guide section 9) or, live, loaded from Slack's thumbnail URL through the app's
//! HTTP client, and Slack-style file cards (type tile, name, type and size) for the rest.
//! Several images show as Slack's gallery ([`Gallery`]); every image preview has Slack's
//! hover Download button.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, Div, ElementId, FontWeight, Hsla, ImageSource, InteractiveElement as _,
    IntoElement, MouseButton, ObjectFit, ParentElement as _, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, canvas, div, px,
};
use slackless_core::types::{File, Message};

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
        column = column.child(match thumbnail(fixtures, file) {
            Some(source) => image_attachment(file, ix, source, on_open, cx),
            None => file_card(file, cx),
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
