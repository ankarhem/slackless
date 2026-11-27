//! What the viewer can show: the conversation's items - still images, videos and PDFs with the
//! header's metadata - collected from the transcript in display order, and the header's
//! relative time stamp.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::gpui::{App, Image, ImageFormat, ImageSource, SharedString, Window};
use slackless_core::types::{Block, FileId, Message, UserId, time};

use crate::fixtures::Fixtures;

/// Which message image a click stands for, so the transcript can find it again while it
/// builds the conversation's image list: a file upload by its index in `Message::files`,
/// a body image by its url.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageOrigin {
    File(usize),
    Url(String),
}

/// What an image click hands to the transcript: identify the clicked image, the root opens
/// the viewer. An `Rc` closure like [`crate::actions::ActionHandler`], passed down to the
/// image elements through [`crate::blockkit::KitCx`] and [`crate::files::attachments`].
pub type ImageClick = Rc<dyn Fn(ImageOrigin, &mut Window, &mut App)>;

/// Playback metadata for a video item displayed natively inside [`super::ImageViewer`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewerVideo {
    pub file_id: String,
    pub url: String,
    pub ext: String,
    pub duration_ms: Option<u64>,
    pub inline_transcript: Option<String>,
    pub vtt_url: Option<String>,
}

/// Metadata for a PDF document displayed page-by-page inside [`super::ImageViewer`]. The page
/// count is the document's own, learned when its first page renders and kept by the media cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewerPdf {
    pub file_id: String,
    pub url: String,
    /// Optional local file path (used by tests or already-local PDFs without network fetch).
    pub local_path: Option<PathBuf>,
}

/// What a viewer item shows beyond its still rendition (a video's poster, a PDF's thumbnail):
/// exactly one of a still image, a playable video or a paged document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewerMedia {
    Image,
    Video(ViewerVideo),
    Pdf(ViewerPdf),
}

/// One image, video, or PDF the viewer can show: the rendition (fixture pixels or a live url the
/// app's HTTP client loads only while the viewer paints it) plus the header's metadata.
pub struct ViewerImage {
    pub source: ImageSource,
    /// The file name under the header ("<file name>").
    pub name: String,
    /// The uploader (a file's `user`, else the carrying message's author).
    pub user: Option<UserId>,
    /// Upload time (unix seconds): a file's `timestamp`, else the message's.
    pub taken_at: Option<i64>,
    /// What Download saves into ~/Downloads: the url and the file name.
    pub download: Option<(String, String)>,
    /// What it shows: a still image, a video or a PDF.
    pub media: ViewerMedia,
}

impl ViewerImage {
    /// Build a standalone [`ViewerImage`] from an active inline video's [`InlineVideoMeta`] so
    /// the floating mini-player / picture-in-picture dock can expand directly into [`super::ImageViewer`].
    pub fn from_inline_video_meta(
        fixtures: &Fixtures,
        meta: &crate::media::InlineVideoMeta,
    ) -> Self {
        let fid = FileId::from(meta.file_id.clone());
        let source = match fixtures.thumbnails.get(&fid) {
            Some(image) => ImageSource::Image(image.clone()),
            None => match meta.poster_url.as_ref() {
                Some(poster) => ImageSource::from(SharedString::from(poster.clone())),
                None => empty_placeholder_source(),
            },
        };
        Self {
            source,
            name: meta.title.clone(),
            user: meta.user.clone(),
            taken_at: meta.taken_at,
            download: meta.download.clone(),
            media: ViewerMedia::Video(ViewerVideo {
                file_id: meta.file_id.clone(),
                url: meta.url.clone(),
                ext: meta.ext.clone(),
                duration_ms: meta.duration_ms,
                inline_transcript: meta.inline_transcript.clone(),
                vtt_url: meta.vtt_url.clone(),
            }),
        }
    }
}

/// A conversation image paired with the click identity that opened it.
pub struct ImageEntry {
    pub origin: ImageOrigin,
    pub image: ViewerImage,
}

/// The message's images in body order: the image urls of its Block Kit payloads, its file
/// uploads, then its legacy attachments' images. The transcript maps a click onto this
/// list to place the viewer inside the conversation's images.
pub fn message_images(fixtures: &Fixtures, message: &Message) -> Vec<ImageEntry> {
    let author = message.user.clone();
    let taken_at = message.ts.secs();
    let mut entries = Vec::new();

    // Block Kit payloads render inside the body: their image urls in document order.
    let mut body_urls = Vec::new();
    for block in &message.blocks {
        if let Block::Unknown { raw, .. } = block {
            collect_json_urls(raw, &mut body_urls);
        }
    }
    for (url, alt) in body_urls {
        if let Some(image) = url_image(fixtures, &url, alt, author.clone(), taken_at) {
            entries.push(ImageEntry {
                origin: ImageOrigin::Url(url),
                image,
            });
        }
    }

    // File uploads render under the body, indexed like `files::attachments` renders them.
    for (ix, file) in message.files.iter().enumerate() {
        let name = file
            .name
            .clone()
            .or_else(|| file.title.clone())
            .unwrap_or_else(|| file.id.to_string());
        if crate::files::is_video(file) {
            let Some(video_url) = crate::files::video_playback_url(file) else {
                continue;
            };
            let source = match fixtures.thumbnails.get(&file.id) {
                Some(image) => ImageSource::Image(image.clone()),
                None => match crate::files::video_poster_url(file) {
                    Some(poster) => ImageSource::from(SharedString::from(poster)),
                    None => empty_placeholder_source(),
                },
            };
            entries.push(ImageEntry {
                origin: ImageOrigin::File(ix),
                image: ViewerImage {
                    source,
                    download: crate::files::download_url(file).map(|url| (url, name.clone())),
                    name,
                    user: file.user.clone().or_else(|| author.clone()),
                    taken_at: file.timestamp.or(taken_at),
                    media: ViewerMedia::Video(ViewerVideo {
                        file_id: file.id.to_string(),
                        url: video_url,
                        ext: crate::files::file_ext(file, "mp4"),
                        duration_ms: crate::files::media_duration_ms(file),
                        inline_transcript: crate::files::inline_clip_transcript(file),
                        vtt_url: crate::files::clip_vtt_url(file),
                    }),
                },
            });
            continue;
        }
        if crate::files::is_pdf(file) {
            let Some(pdf_url) = crate::files::download_url(file) else {
                continue;
            };
            let source = match fixtures.thumbnails.get(&file.id) {
                Some(image) => ImageSource::Image(image.clone()),
                None => match crate::files::pdf_thumb_url(file) {
                    Some(thumb) => ImageSource::from(SharedString::from(thumb)),
                    None => empty_placeholder_source(),
                },
            };
            entries.push(ImageEntry {
                origin: ImageOrigin::File(ix),
                image: ViewerImage {
                    source,
                    download: Some((pdf_url.clone(), name.clone())),
                    name,
                    user: file.user.clone().or_else(|| author.clone()),
                    taken_at: file.timestamp.or(taken_at),
                    media: ViewerMedia::Pdf(ViewerPdf {
                        file_id: file.id.to_string(),
                        url: pdf_url,
                        local_path: None,
                    }),
                },
            });
            continue;
        }
        if !file
            .mimetype
            .as_deref()
            .is_some_and(|mime| mime.starts_with("image/"))
        {
            continue;
        }
        let source = match fixtures.thumbnails.get(&file.id) {
            Some(image) => ImageSource::Image(image.clone()),
            None => {
                // The large rendition (url_private) or the biggest thumbnail, loaded
                // through the app's authenticated HTTP client only while shown. For formats GPUI
                // cannot decode natively (e.g. `.heic`), prefer Slack's transcoded JPEG/PNG
                // thumbnails first.
                let preferred_original = file
                    .url_private
                    .as_deref()
                    .filter(|u| crate::files::is_decodable_image_url(u))
                    .map(str::to_owned);
                let url = preferred_original
                    .or_else(|| file.thumb_720.clone())
                    .or_else(|| file.thumb_480.clone())
                    .or_else(|| file.thumb_360.clone())
                    .or_else(|| file.url_private.clone());
                match url {
                    Some(url) => ImageSource::from(SharedString::from(url)),
                    None => continue,
                }
            }
        };
        entries.push(ImageEntry {
            origin: ImageOrigin::File(ix),
            image: ViewerImage {
                source,
                download: crate::files::download_url(file).map(|url| (url, name.clone())),
                name,
                user: file.user.clone().or_else(|| author.clone()),
                taken_at: file.timestamp.or(taken_at),
                media: ViewerMedia::Image,
            },
        });
    }

    // Legacy attachments (unfurl and attachment cards) render last.
    let mut attachment_urls = Vec::new();
    for attachment in &message.attachments {
        if let Some(url) = attachment
            .image_url
            .as_deref()
            .or(attachment.thumb_url.as_deref())
        {
            attachment_urls.push((url.to_owned(), url_tail(url)));
        }
        for block in &attachment.blocks {
            if let Block::Unknown { raw, .. } = block {
                collect_json_urls(raw, &mut attachment_urls);
            }
        }
    }
    for (url, alt) in attachment_urls {
        if let Some(image) = url_image(fixtures, &url, alt, author.clone(), taken_at) {
            entries.push(ImageEntry {
                origin: ImageOrigin::Url(url),
                image,
            });
        }
    }

    entries
}

/// The rendition of an item with no pixels of its own: an empty PNG the stage scales to nothing.
pub(super) fn empty_placeholder_source() -> ImageSource {
    ImageSource::Image(Arc::new(Image::from_bytes(ImageFormat::Png, Vec::new())))
}

/// The item's still rendition when it has pixels (a video's poster, a PDF's thumbnail).
pub(super) fn poster_source_if_present(source: &ImageSource) -> Option<ImageSource> {
    match source {
        ImageSource::Image(img) if img.bytes.is_empty() => None,
        other => Some(other.clone()),
    }
}

fn url_tail(url: &str) -> String {
    url.rsplit('?')
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or(url)
        .to_owned()
}

/// A body image's rendition: fixture pixels, or the live url through the app's HTTP
/// client (fixture mode resolves nothing outside its decoded images, like the message
/// renderer's `picture`).
fn url_image(
    fixtures: &Fixtures,
    url: &str,
    alt: String,
    user: Option<UserId>,
    taken_at: Option<i64>,
) -> Option<ViewerImage> {
    let source = match fixtures.block_images.get(url) {
        Some(image) => ImageSource::Image(image.clone()),
        None if !fixtures.sample && url.starts_with("https://") => {
            ImageSource::from(SharedString::from(url.to_owned()))
        }
        None => return None,
    };
    Some(ViewerImage {
        source,
        name: if alt.is_empty() { url_tail(url) } else { alt },
        user,
        taken_at,
        download: Some((url.to_owned(), url_tail(url))),
        media: ViewerMedia::Image,
    })
}

/// The `image_url`s of a Block Kit payload in document order, each with its alt text.
fn collect_json_urls(value: &crate::blockkit::serde_json::Value, out: &mut Vec<(String, String)>) {
    use crate::blockkit::serde_json::Value;
    match value {
        Value::Object(map) => {
            let mut alt = String::new();
            for (key, child) in map {
                match key.as_str() {
                    "image_url" => {
                        if let Some(url) = child.as_str() {
                            out.push((
                                url.to_owned(),
                                if alt.is_empty() {
                                    url_tail(url)
                                } else {
                                    alt.clone()
                                },
                            ));
                        }
                    }
                    "slack_file" => {
                        if let Some(url) = child.get("url").and_then(|v| v.as_str()) {
                            out.push((
                                url.to_owned(),
                                if alt.is_empty() {
                                    url_tail(url)
                                } else {
                                    alt.clone()
                                },
                            ));
                        }
                    }
                    "alt_text" => alt = child.as_str().unwrap_or_default().to_owned(),
                    _ => collect_json_urls(child, out),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| collect_json_urls(item, out)),
        _ => {}
    }
}

/// Walk `messages` in display order, collecting every viewer image and locating the index of
/// the clicked `(ts, origin)`.
pub fn collect_viewer_images<'a>(
    fixtures: &Fixtures,
    messages: impl IntoIterator<Item = &'a Message>,
    ts: &slackless_core::types::Ts,
    origin: &ImageOrigin,
) -> Option<(Rc<Vec<ViewerImage>>, usize)> {
    let mut images = Vec::new();
    let mut index = None;
    for message in messages {
        for entry in message_images(fixtures, message) {
            if index.is_none() && &message.ts == ts && &entry.origin == origin {
                index = Some(images.len());
            }
            images.push(entry.image);
        }
    }
    index.map(|ix| (Rc::new(images), ix))
}

/// Slack's header stamp: "29 days ago in # praise - image.png" for a day-old upload,
/// "just now" / "N minutes ago" / "N hours ago" / "yesterday" / "N days ago" within a
/// week, then a plain "Mon D" date.
pub(crate) fn relative_stamp(secs: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let now = time::now_epoch_secs();
    let ago = (now - secs).max(0);
    match ago {
        0..=59 => "just now".to_owned(),
        60..=3_599 => plural(ago / 60, "minute"),
        3_600..=86_399 => plural(ago / 3_600, "hour"),
        86_400..=172_799 => "yesterday".to_owned(),
        172_800..=604_799 => plural(ago / 86_400, "day"),
        _ => {
            let (_year, month, day) = time::civil_from_days(secs.div_euclid(86_400));
            let month_name = MONTHS
                .get(month.saturating_sub(1) as usize)
                .copied()
                .unwrap_or("Jan");
            format!("{month_name} {day}")
        }
    }
}

fn plural(count: i64, unit: &str) -> String {
    if count == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{count} {unit}s ago")
    }
}
