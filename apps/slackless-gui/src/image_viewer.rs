//! The image viewer overlay (gpui-kit-guide section 9 images at full size): clicking a
//! message image - a file upload, a Block Kit image or an attachment image - opens Slack's
//! viewer over the window below the title bar: the uploader chip top-left, the fitted
//! rendition centered, round prev/next arrows stepping through the conversation's images,
//! zoom out / slider / zoom in bottom-left, Download bottom-right and a close X; Escape
//! (or the X) closes it and drops the large rendition.
//!
//! The overlay is mounted by the root ([`crate::app`]) when a transcript image click emits
//! `MessageListEvent::OpenImage`; while it is open only the shown image's bytes are alive
//! (fixture pixels are `Arc`s into [`crate::fixtures`], live URLs load through the app's
//! HTTP client and drop with this entity on close).
//!
//! Chrome colors are fixed rather than theme tokens: Slack renders its viewer dark in both
//! modes (the reference screenshot is a light workspace under the same dark viewer), like
//! the avatar identity chips [`crate::theme`] keeps constant.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Div, Entity, EventEmitter,
    FontWeight, Image, ImageFormat, ImageSource, InteractiveElement as _, IntoElement, MouseButton,
    ObjectFit, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, div, px, relative,
};
use slackless_core::huddle::NativeVideoPlayer;
use slackless_core::types::{Block, FileId, Message, UserId, time};

use crate::avatars;
use crate::fixtures::Fixtures;
use crate::theme::{self, ViewerChrome};

/// Zoom steps as multiples of the fitted size; the viewer opens at [`FIT_INDEX`].
const ZOOM_STEPS: [f32; 10] = [0.1, 0.15, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0];
/// The fitted size: the image scales into the free area, exactly as Slack first shows it.
const FIT_INDEX: usize = 5;

/// Round viewer arrows (the prev/next pair).
const ARROW_BUTTON: f32 = 36.;
/// The square download button and the round close button.
const CONTROL_BUTTON: f32 = 36.;
/// Slack's zoom slider width between the - and + buttons.
const SLIDER_WIDTH: f32 = 120.;
/// Gutter the arrows reserve at the sides of the image area.
const ARROW_GUTTER: f32 = 72.;
/// Width of the toggleable right-side video transcript drawer.
const TRANSCRIPT_PANEL_WIDTH: f32 = 320.;
/// Disjoint selection document order for the overlay's selectable transcript text.
const VIEWER_TRANSCRIPT_ORDER: u64 = 1u64 << 62;

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

/// Playback metadata for a video item displayed natively inside [`ImageViewer`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewerVideo {
    pub file_id: String,
    pub url: String,
    pub ext: String,
    pub duration_ms: Option<u64>,
    pub inline_transcript: Option<String>,
    pub vtt_url: Option<String>,
}

/// One image or video the viewer can show: the rendition (fixture pixels or a live url the app's
/// HTTP client loads only while the viewer paints it) plus the header's metadata.
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
    /// Present when this viewer item is a playable video rather than a static/animated image.
    pub video: Option<ViewerVideo>,
}

/// A conversation image paired with the click identity that opened it.
pub struct ImageEntry {
    pub origin: ImageOrigin,
    pub image: ViewerImage,
}

/// What the overlay tells the root: the close X was pressed (Escape is intercepted by the
/// root itself).
pub enum ViewerEvent {
    Close,
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
                    video: Some(ViewerVideo {
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
                video: None,
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

fn empty_placeholder_source() -> ImageSource {
    ImageSource::Image(Arc::new(Image::from_bytes(ImageFormat::Png, Vec::new())))
}

fn poster_source_if_present(source: &ImageSource) -> Option<ImageSource> {
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
        video: None,
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

/// Number of post-pause/seek ticks before the viewer's video timer idles at 0% CPU.
const VIEWER_SETTLE_TICKS: u32 = 15;

struct ViewerVideoPlayback {
    generation: u64,
    loading: bool,
    player: Option<NativeVideoPlayer>,
    last_frame: Option<Arc<crate::media::VideoSurface>>,
    error: Option<String>,
    fallback_duration_secs: f64,
}

/// The viewer overlay: an entity so the root can mount it like the other overlays and
/// drop it (and the large rendition / video player it holds) on close.
pub struct ImageViewer {
    fixtures: Arc<Fixtures>,
    /// The conversation label the header shows ("# design").
    channel: String,
    /// Every image/video in the conversation (transcript order); only the shown one is painted,
    /// so only it loads.
    images: Rc<Vec<ViewerImage>>,
    index: usize,
    zoom_ix: usize,
    slider: Entity<SliderState>,
    _subscription: Subscription,
    video_gen: u64,
    video_playback: Option<ViewerVideoPlayback>,
    video_load_task: Option<Task<()>>,
    video_tick_task: Option<Task<()>>,
    show_transcript: bool,
    transcript_copied: bool,
}

impl Drop for ImageViewer {
    fn drop(&mut self) {
        self.stop_video();
    }
}

impl EventEmitter<ViewerEvent> for ImageViewer {}

impl ImageViewer {
    /// Evict decoded full-resolution image textures loaded by this viewer from GPUI's window &
    /// asset caches and immediately stop any active video playback so audio never leaks after
    /// close.
    pub fn evict_assets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_video();
        for entry in self.images.iter() {
            if let ImageSource::Resource(ref resource) = entry.source {
                if let Some(Ok(data)) =
                    window.get_asset::<gpui_kit::gpui::ImgResourceLoader>(resource, cx)
                {
                    let _ = window.drop_image(data);
                }
                entry.source.remove_asset(cx);
            }
        }
    }

    pub fn new(
        fixtures: Arc<Fixtures>,
        channel: String,
        images: Rc<Vec<ViewerImage>>,
        index: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let index = index.min(images.len().saturating_sub(1));
        let slider = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max((ZOOM_STEPS.len() - 1) as f32)
                .step(1.)
                .default_value(FIT_INDEX as f32)
        });
        let viewer = cx.new(|cx| {
            let _subscription = cx.subscribe(&slider, |viewer: &mut Self, _, event, cx| {
                if let SliderEvent::Change(SliderValue::Single(step)) = event {
                    let ix = (step.round() as usize).min(ZOOM_STEPS.len() - 1);
                    if viewer.zoom_ix != ix {
                        viewer.zoom_ix = ix;
                        cx.notify();
                    }
                }
            });
            Self {
                fixtures,
                channel,
                images,
                index,
                zoom_ix: FIT_INDEX,
                slider,
                _subscription,
                video_gen: 0,
                video_playback: None,
                video_load_task: None,
                video_tick_task: None,
                show_transcript: false,
                transcript_copied: false,
            }
        });
        viewer.update(cx, |v, cx| v.sync_current_media(window, cx));
        viewer
    }

    /// The shown item.
    fn current(&self) -> &ViewerImage {
        &self.images[self.index]
    }

    /// Update workspace fixtures (e.g. when a newly transcribed video's transcript lands live).
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        cx.notify();
    }

    /// Whether the currently shown item is a playable video.
    pub fn is_video(&self) -> bool {
        self.current().video.is_some()
    }

    /// Toggle visibility of the right-side video transcript drawer; lazily fetches WebVTT when
    /// needed.
    pub fn toggle_transcript(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_video() {
            return;
        }
        self.show_transcript = !self.show_transcript;
        self.transcript_copied = false;
        if self.show_transcript {
            self.ensure_transcript_loaded(window, cx);
        }
        cx.notify();
    }

    fn ensure_transcript_loaded(&self, window: &mut Window, cx: &mut App) {
        let Some(video) = self.current().video.as_ref() else {
            return;
        };
        let fid = FileId::from(video.file_id.clone());
        let has_local_or_inline =
            self.fixtures.file_transcript(&fid).is_some() || video.inline_transcript.is_some();
        if !has_local_or_inline && let Some(vtt_url) = video.vtt_url.as_ref() {
            crate::files::fetch_clip_vtt_if_needed(fid, vtt_url.clone(), window, cx);
        }
    }

    fn resolve_video_transcript(
        &self,
        video: &ViewerVideo,
        cx: &App,
    ) -> (Option<String>, bool, Option<String>) {
        let fid = FileId::from(video.file_id.clone());
        let cached = crate::files::clip_transcript_status(&fid, cx);
        let text = self
            .fixtures
            .file_transcript(&fid)
            .map(str::to_owned)
            .or(cached.text)
            .or_else(|| video.inline_transcript.clone());
        (text, cached.loading, cached.error)
    }

    /// Stop and drop any active video player and background tasks immediately.
    pub fn stop_video(&mut self) {
        self.video_gen = self.video_gen.wrapping_add(1);
        self.video_load_task = None;
        self.video_tick_task = None;
        self.video_playback = None;
    }

    /// Sync video playback to `self.current()`: stops any previous video, and if the current
    /// item is a video, hands off from inline playback, fetches/caches, opens `NativeVideoPlayer`,
    /// and starts playback.
    fn sync_current_media(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_video();
        let Some(video) = self.current().video.clone() else {
            return;
        };
        let generation = self.video_gen;
        let carry = crate::media::take_inline_handoff_for_viewer(&video.file_id, window, cx);
        let fallback_duration_secs = video.duration_ms.map_or(0.0, |ms| ms as f64 / 1000.0);
        let hls = crate::media::is_hls_url(&video.url);

        let fetcher = if hls {
            crate::media::active_fetcher(cx)
        } else {
            let Some(fetcher) = crate::media::active_fetcher(cx) else {
                self.video_playback = Some(ViewerVideoPlayback {
                    generation,
                    loading: false,
                    player: None,
                    last_frame: None,
                    error: Some("Playback needs a signed-in workspace.".to_owned()),
                    fallback_duration_secs,
                });
                cx.notify();
                return;
            };
            Some(fetcher)
        };

        self.video_playback = Some(ViewerVideoPlayback {
            generation,
            loading: true,
            player: None,
            last_frame: None,
            error: None,
            fallback_duration_secs,
        });
        cx.notify();

        let file_id = video.file_id;
        let url = video.url;
        let ext = video.ext;
        let fetch_url = url.clone();
        let fetch_ext = ext.clone();
        let cache_id = file_id.clone();
        self.video_load_task = Some(cx.spawn_in(window, async move |weak, cx| {
            let bg = cx.background_executor().clone();
            let open_res = if hls {
                bg.spawn(async move { NativeVideoPlayer::open_url(&fetch_url, None) })
                    .await
                    .map_err(|e| e.to_string())
            } else {
                let fetcher = fetcher.expect("fetcher validated for non-HLS media");
                let path = crate::media::fetch_cached_media(
                    fetcher,
                    cache_id,
                    fetch_url,
                    fetch_ext,
                    bg.clone(),
                )
                .await;
                match path {
                    Ok(path) => bg
                        .spawn(async move { NativeVideoPlayer::open(&path) })
                        .await
                        .map_err(|e| e.to_string()),
                    Err(err) => Err(err),
                }
            };
            let _ = weak.update_in(cx, |viewer, window, cx| {
                if viewer.video_gen != generation {
                    drop(open_res);
                    return;
                }
                match open_res {
                    Ok(player) => {
                        if let Some(handoff) = carry {
                            if (handoff.rate - 1.0).abs() > 0.01 {
                                player.set_rate(handoff.rate);
                            }
                            if handoff.position_secs.is_finite() && handoff.position_secs > 0.05 {
                                player.seek(handoff.position_secs);
                            }
                        }
                        player.play();
                        let initial_frame = crate::media::copy_player_surface(&player);
                        if let Some(pb) = viewer.video_playback.as_mut() {
                            pb.loading = false;
                            pb.error = None;
                            pb.last_frame = initial_frame;
                            pb.player = Some(player);
                        }
                        if !hls && !file_id.starts_with("bk-video-") {
                            crate::media::transcribe_media(
                                slackless_core::types::FileId::from(file_id.clone()),
                                url,
                                ext,
                                true,
                                window,
                                cx,
                            );
                        }
                        viewer.spawn_video_tick_loop(generation, window, cx);
                        cx.notify();
                    }
                    Err(err) => {
                        if let Some(pb) = viewer.video_playback.as_mut() {
                            pb.loading = false;
                            pb.error = Some(err);
                        }
                        cx.notify();
                    }
                }
            });
        }));
    }

    fn spawn_video_tick_loop(
        &mut self,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.video_tick_task = Some(cx.spawn_in(window, async move |weak, cx| {
            let mut idle_ticks = 0u32;
            loop {
                cx.background_executor()
                    .timer(crate::media::VIDEO_FRAME_INTERVAL)
                    .await;
                let keep_going = weak
                    .update(cx, |viewer, cx| {
                        if viewer.video_gen != generation {
                            return false;
                        }
                        let Some(pb) = viewer
                            .video_playback
                            .as_mut()
                            .filter(|pb| pb.generation == generation)
                        else {
                            return false;
                        };
                        let Some(player) = pb.player.as_ref() else {
                            return false;
                        };
                        if let Some(frame) = crate::media::copy_player_surface(player) {
                            pb.last_frame = Some(frame);
                        }
                        let st = player.state();
                        cx.notify();
                        if st.failed {
                            pb.error = Some("Video playback failed.".to_owned());
                            return false;
                        }
                        if st.playing && !st.ended {
                            idle_ticks = 0;
                            true
                        } else {
                            idle_ticks += 1;
                            idle_ticks < VIEWER_SETTLE_TICKS
                        }
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        }));
    }

    /// Toggle play/pause on the viewer's currently open video.
    pub fn toggle_video_play(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pb) = self.video_playback.as_mut() else {
            return;
        };
        let Some(player) = pb.player.as_ref() else {
            return;
        };
        player.toggle_play();
        if let Some(frame) = crate::media::copy_player_surface(player) {
            pb.last_frame = Some(frame);
        }
        let gen_id = pb.generation;
        self.spawn_video_tick_loop(gen_id, window, cx);
        cx.notify();
    }

    /// Toggle mute/unmute on the viewer's currently open video.
    pub fn toggle_video_mute(&mut self, cx: &mut Context<Self>) {
        let Some(player) = self
            .video_playback
            .as_ref()
            .and_then(|pb| pb.player.as_ref())
        else {
            return;
        };
        let muted = !player.state().muted;
        player.set_muted(muted);
        cx.notify();
    }

    /// Cycle playback speed (`1x -> 1.5x -> 2x -> 1x`) on the viewer's currently open video.
    pub fn cycle_video_rate(&mut self, cx: &mut Context<Self>) {
        let Some(player) = self
            .video_playback
            .as_ref()
            .and_then(|pb| pb.player.as_ref())
        else {
            return;
        };
        player.cycle_rate();
        cx.notify();
    }

    /// Seek the viewer's currently open video relative to its current position by `delta_secs`.
    pub fn seek_video_relative(
        &mut self,
        delta_secs: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cur) = self
            .video_playback
            .as_ref()
            .and_then(|pb| pb.player.as_ref())
            .map(|p| crate::media::sanitize_secs(p.state().position_secs))
        else {
            return;
        };
        self.seek_video((cur + delta_secs).max(0.0), window, cx);
    }

    /// Seek the viewer's currently open video to `secs`.
    pub fn seek_video(&mut self, secs: f64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pb) = self.video_playback.as_mut() else {
            return;
        };
        let Some(player) = pb.player.as_ref() else {
            return;
        };
        let duration = crate::media::sanitize_secs(player.state().duration_secs);
        let target = if duration > 0.0 {
            crate::media::sanitize_secs(secs).min(duration)
        } else {
            crate::media::sanitize_secs(secs)
        };
        player.seek(target);
        if let Some(frame) = crate::media::copy_player_surface(player) {
            pb.last_frame = Some(frame);
        }
        let gen_id = pb.generation;
        self.spawn_video_tick_loop(gen_id, window, cx);
        cx.notify();
    }

    /// Step prev (-1) / next (+1) through the conversation's images/videos; each item starts
    /// fitted again, like Slack's viewer.
    pub fn step(&mut self, direction: isize, window: &mut Window, cx: &mut Context<Self>) {
        let next = self.index as isize + direction;
        if next < 0 || next >= self.images.len() as isize {
            return;
        }
        self.index = next as usize;
        self.transcript_copied = false;
        self.zoom_to(FIT_INDEX, window, cx);
        self.sync_current_media(window, cx);
        if self.show_transcript {
            self.ensure_transcript_loaded(window, cx);
        }
        cx.notify();
    }

    /// Zoom to the step at `zoom_ix` (the slider drags set it directly), moving the
    /// slider along when the buttons or the keyboard step it.
    fn zoom_to(&mut self, zoom_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let zoom_ix = zoom_ix.min(ZOOM_STEPS.len() - 1);
        if zoom_ix == self.zoom_ix {
            return;
        }
        self.zoom_ix = zoom_ix;
        self.slider.update(cx, |slider, cx| {
            slider.set_value(zoom_ix as f32, window, cx);
        });
        cx.notify();
    }

    /// Step zoom in (`+1`) or out (`-1`) through `ZOOM_STEPS` (no-op while showing a video).
    pub fn zoom_step(&mut self, direction: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_video() {
            return;
        }
        let next = (self.zoom_ix as isize + direction).clamp(0, ZOOM_STEPS.len() as isize - 1);
        self.zoom_to(next as usize, window, cx);
    }

    /// Reset zoom back to fit (`100%`; no-op while showing a video).
    pub fn reset_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_video() {
            return;
        }
        self.zoom_to(FIT_INDEX, window, cx);
    }

    /// Current index in `ZOOM_STEPS` (used by tests).
    #[cfg(test)]
    pub fn zoom_index(&self) -> usize {
        self.zoom_ix
    }

    /// Current image index (used by tests).
    #[cfg(test)]
    pub fn current_index(&self) -> usize {
        self.index
    }

    /// Whether the video transcript panel is currently visible (used by tests).
    #[cfg(test)]
    pub fn showing_transcript(&self) -> bool {
        self.show_transcript && self.is_video()
    }

    /// The uploader's name (Slack shows the file's uploader on the chip).
    fn uploader_name(&self) -> String {
        match self.current().user.as_ref() {
            Some(user) => self.fixtures.user_name(user).to_owned(),
            None => "Unknown".to_owned(),
        }
    }

    /// A round chrome arrow button: dark glass, hairline border, hover lift.
    fn arrow_button(
        id: &'static str,
        icon: IconName,
        chrome: ViewerChrome,
    ) -> gpui_kit::gpui::Stateful<Div> {
        div()
            .id(id)
            .flex_none()
            .size(px(ARROW_BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(chrome.button)
            .border_1()
            .border_color(chrome.border)
            .cursor_pointer()
            .hover(move |button| button.bg(chrome.button_hover))
            .child(Icon::new(icon).text_color(chrome.text).size(px(18.)))
    }

    /// A square glass button (the download control, Slack's bottom-right tiles).
    fn tile_button(
        id: &'static str,
        icon: IconName,
        chrome: ViewerChrome,
    ) -> gpui_kit::gpui::Stateful<Div> {
        div()
            .id(id)
            .flex_none()
            .size(px(CONTROL_BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .rounded_lg()
            .bg(chrome.button)
            .border_1()
            .border_color(chrome.border)
            .cursor_pointer()
            .hover(move |button| button.bg(chrome.button_hover))
            .child(Icon::new(icon).text_color(chrome.text).size(px(16.)))
    }
}

impl Render for ImageViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = theme::viewer_chrome();
        let image = self.current();
        let zoom = ZOOM_STEPS[self.zoom_ix];
        let multiple = self.images.len() > 1;
        let name = SharedString::from(image.name.clone());
        let stamp = image
            .taken_at
            .map(relative_stamp)
            .unwrap_or_else(|| "uploaded".to_owned());
        let prev_enabled = self.index > 0;
        let next_enabled = self.index + 1 < self.images.len();
        let zoom_out_enabled = self.zoom_ix > 0;
        let zoom_in_enabled = self.zoom_ix + 1 < ZOOM_STEPS.len();

        // The header chip: the uploader's avatar, name, and "<relative time> in
        // # <channel> - <file name>".
        let chip = h_flex()
            .min_w_0()
            .items_center()
            .gap_3()
            .pl(px(10.))
            .pr(px(14.))
            .py(px(8.))
            .rounded_lg()
            .bg(chrome.glass)
            .when_some(image.user.clone(), |chip, user| {
                chip.child(avatars::user_chip(&self.fixtures, &user, 36.))
            })
            .child(
                v_flex()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(chrome.text)
                            .truncate()
                            .child(self.uploader_name()),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_1()
                            .text_xs()
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(chrome.text_muted)
                                    .child(SharedString::from(format!("{stamp} in"))),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(chrome.text_muted)
                                    .child(SharedString::from(self.channel.clone())),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .max_w(px(280.))
                                    .text_color(chrome.text_dim)
                                    .child(SharedString::from(format!("- {name}"))),
                            ),
                    ),
            );

        let is_video = image.video.is_some();
        let (
            vid_loading,
            vid_playing,
            vid_muted,
            vid_ended,
            vid_pos,
            vid_dur,
            vid_rate,
            vid_surface,
            vid_err,
        ) = if is_video {
            if let Some(pb) = self.video_playback.as_ref() {
                if let Some(player) = pb.player.as_ref() {
                    let st = player.state();
                    let surface =
                        crate::media::copy_player_surface(player).or_else(|| pb.last_frame.clone());
                    let dur = crate::media::sanitize_secs(st.duration_secs);
                    let effective_dur = if dur > 0.0 {
                        dur
                    } else {
                        pb.fallback_duration_secs
                    };
                    let rate = if st.rate.is_finite() && st.rate > 0.0 {
                        st.rate
                    } else {
                        1.0
                    };
                    (
                        pb.loading,
                        st.playing,
                        st.muted,
                        st.ended,
                        crate::media::sanitize_secs(st.position_secs),
                        effective_dur,
                        rate,
                        surface,
                        pb.error.clone(),
                    )
                } else {
                    (
                        pb.loading,
                        false,
                        false,
                        false,
                        0.0,
                        pb.fallback_duration_secs,
                        1.0,
                        pb.last_frame.clone(),
                        pb.error.clone(),
                    )
                }
            } else {
                (false, false, false, false, 0.0, 0.0, 1.0, None, None)
            }
        } else {
            (false, false, false, false, 0.0, 0.0, 1.0, None, None)
        };

        let stage_content = if is_video {
            let poster = poster_source_if_present(&image.source);
            let fg = theme::media_overlay_foreground();
            let overlay = if vid_loading {
                Some(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .px_3()
                                .py_1p5()
                                .rounded_full()
                                .bg(theme::media_overlay_scrim_strong())
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg)
                                .child("Loading video\u{2026}"),
                        ),
                )
            } else if let Some(err) = vid_err.clone() {
                Some(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .px_3()
                                .py_2()
                                .rounded_lg()
                                .bg(theme::media_overlay_scrim_strong())
                                .border_1()
                                .border_color(chrome.border)
                                .text_xs()
                                .text_color(fg)
                                .child(err),
                        ),
                )
            } else if !vid_playing || vid_ended {
                Some(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .size(px(56.))
                                .rounded_full()
                                .bg(theme::media_overlay_scrim())
                                .border_1()
                                .border_color(theme::media_overlay_border())
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Icon::new(IconName::Play).size(px(26.)).text_color(fg)),
                        ),
                )
            } else {
                None
            };
            div()
                .id("viewer-video-stage")
                .relative()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .on_click(cx.listener(|viewer, _, window, cx| {
                    viewer.toggle_video_play(window, cx);
                }))
                .child(crate::media::render_video_surface(
                    vid_surface,
                    poster,
                    px(8.),
                ))
                .children(overlay)
                .into_any_element()
        } else {
            div()
                .w(relative(zoom))
                .h(relative(zoom))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    crate::media::Picture::new(image.source.clone())
                        .fit(ObjectFit::Contain)
                        .radius(px(8.)),
                )
                .into_any_element()
        };

        let transcript_panel = if self.show_transcript
            && let Some(video) = image.video.clone()
        {
            let fid = FileId::from(video.file_id.clone());
            let (transcript_text, loading_vtt, vtt_error) =
                self.resolve_video_transcript(&video, cx);
            let transcribing = crate::media::is_transcribing(&fid, cx);
            let copied = self.transcript_copied;
            let copy_text = transcript_text.clone();
            let can_transcribe = transcript_text.is_none()
                && !loading_vtt
                && !transcribing
                && !video.url.is_empty()
                && !crate::media::is_hls_url(&video.url);
            let body_el: AnyElement = if let Some(text) = transcript_text {
                crate::messages::selectable_text(
                    &format!("viewer-transcript-{}", video.file_id),
                    0,
                    text,
                    Some(chrome.text),
                    VIEWER_TRANSCRIPT_ORDER,
                    cx,
                )
            } else if loading_vtt {
                div()
                    .text_color(chrome.text_muted)
                    .child("Loading transcript\u{2026}")
                    .into_any_element()
            } else if transcribing {
                div()
                    .text_color(chrome.text_muted)
                    .child("Transcribing\u{2026}")
                    .into_any_element()
            } else if let Some(err) = vtt_error {
                div()
                    .text_color(chrome.text_muted)
                    .child(format!("Could not load transcript: {err}"))
                    .into_any_element()
            } else {
                let transcribe_fid = fid.clone();
                let transcribe_url = video.url.clone();
                let transcribe_ext = video.ext.clone();
                v_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .text_color(chrome.text_muted)
                            .child("No transcript available yet."),
                    )
                    .when(can_transcribe, |col| {
                        col.child(
                            h_flex()
                                .id("viewer-transcript-generate")
                                .items_center()
                                .gap_1p5()
                                .px_2p5()
                                .py_1()
                                .rounded_md()
                                .bg(chrome.button)
                                .border_1()
                                .border_color(chrome.border)
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(chrome.text)
                                .cursor_pointer()
                                .hover(move |b| b.bg(chrome.button_hover))
                                .child(
                                    Icon::new(IconName::Captions)
                                        .size(px(13.))
                                        .text_color(chrome.text),
                                )
                                .child("Transcribe")
                                .on_click(move |_, window, cx| {
                                    crate::media::transcribe_media(
                                        transcribe_fid.clone(),
                                        transcribe_url.clone(),
                                        transcribe_ext.clone(),
                                        false,
                                        window,
                                        cx,
                                    );
                                }),
                        )
                    })
                    .into_any_element()
            };

            Some(
                v_flex()
                    .id("viewer-transcript-panel")
                    .w(px(TRANSCRIPT_PANEL_WIDTH))
                    .flex_none()
                    .h_full()
                    .mr(px(12.))
                    .rounded_lg()
                    .bg(chrome.glass)
                    .border_1()
                    .border_color(chrome.border)
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .border_b_1()
                            .border_color(chrome.border)
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1p5()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(chrome.text)
                                    .child(
                                        Icon::new(IconName::Captions)
                                            .size(px(14.))
                                            .text_color(chrome.text_muted),
                                    )
                                    .child("Transcript"),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .when_some(copy_text, |row, text| {
                                        row.child(
                                            h_flex()
                                                .id("viewer-transcript-copy")
                                                .items_center()
                                                .gap_1()
                                                .px_1p5()
                                                .py(px(2.))
                                                .rounded_sm()
                                                .text_xs()
                                                .text_color(if copied {
                                                    chrome.text
                                                } else {
                                                    chrome.text_muted
                                                })
                                                .cursor_pointer()
                                                .hover(move |b| b.bg(chrome.button_hover))
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                    cx.stop_propagation()
                                                })
                                                .child(
                                                    Icon::new(if copied {
                                                        IconName::Check
                                                    } else {
                                                        IconName::Copy
                                                    })
                                                    .size(px(12.))
                                                    .text_color(if copied {
                                                        chrome.text
                                                    } else {
                                                        chrome.text_muted
                                                    }),
                                                )
                                                .child(if copied { "Copied" } else { "Copy" })
                                                .on_click(cx.listener(move |viewer, _, _, cx| {
                                                    cx.write_to_clipboard(
                                                        ClipboardItem::new_string(text.clone()),
                                                    );
                                                    viewer.transcript_copied = true;
                                                    cx.notify();
                                                })),
                                        )
                                    })
                                    .child(
                                        div()
                                            .id("viewer-transcript-close")
                                            .size(px(24.))
                                            .rounded_sm()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .cursor_pointer()
                                            .hover(move |b| b.bg(chrome.button_hover))
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation()
                                            })
                                            .child(
                                                Icon::new(IconName::X)
                                                    .size(px(13.))
                                                    .text_color(chrome.text_muted),
                                            )
                                            .on_click(cx.listener(|viewer, _, window, cx| {
                                                viewer.toggle_transcript(window, cx);
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("viewer-transcript-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px_3()
                            .py_2p5()
                            .text_xs()
                            .line_height(px(18.))
                            .text_color(chrome.text)
                            .on_scroll(|_, _, cx| cx.stop_propagation())
                            .child(body_el),
                    ),
            )
        } else {
            None
        };

        let bottom_controls = if is_video {
            let progress = crate::media::progress_fraction(vid_pos, vid_dur);
            let time_readout = if vid_dur > 0.0 {
                format!(
                    "{} / {}",
                    crate::media::format_video_time(vid_pos),
                    crate::media::format_video_time(vid_dur)
                )
            } else {
                crate::media::format_video_time(vid_pos)
            };
            let weak = cx.weak_entity();
            let scrub = crate::media::video_scrub_bar(
                SharedString::from("viewer-video-scrub"),
                progress,
                vid_dur,
                chrome.border,
                chrome.text,
                move |secs, window, cx| {
                    let _ = weak.update(cx, |viewer, cx| {
                        viewer.seek_video(secs, window, cx);
                    });
                },
            );
            let transcript_active = self.show_transcript;
            h_flex()
                .items_center()
                .gap_2()
                .px_2p5()
                .h(px(CONTROL_BUTTON))
                .w(px(500.))
                .max_w_full()
                .rounded_lg()
                .bg(chrome.glass)
                .border_1()
                .border_color(chrome.border)
                .child(
                    div()
                        .id("viewer-video-play")
                        .flex_none()
                        .size(px(28.))
                        .rounded_md()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(move |b| b.bg(chrome.button_hover))
                        .child(
                            Icon::new(if vid_playing {
                                IconName::Pause
                            } else {
                                IconName::Play
                            })
                            .text_color(chrome.text)
                            .size(px(15.)),
                        )
                        .on_click(cx.listener(|viewer, _, window, cx| {
                            viewer.toggle_video_play(window, cx);
                        })),
                )
                .child(scrub)
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(chrome.text_muted)
                        .child(time_readout),
                )
                .child(
                    div()
                        .id("viewer-video-rate")
                        .flex_none()
                        .px_1p5()
                        .h(px(24.))
                        .rounded_md()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(chrome.text)
                        .cursor_pointer()
                        .hover(move |b| b.bg(chrome.button_hover))
                        .child(crate::media::format_playback_rate(vid_rate))
                        .on_click(cx.listener(|viewer, _, _, cx| {
                            viewer.cycle_video_rate(cx);
                        })),
                )
                .child(
                    div()
                        .id("viewer-video-mute")
                        .flex_none()
                        .size(px(28.))
                        .rounded_md()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(move |b| b.bg(chrome.button_hover))
                        .child(
                            Icon::new(if vid_muted {
                                IconName::VolumeX
                            } else {
                                IconName::Volume2
                            })
                            .text_color(chrome.text)
                            .size(px(15.)),
                        )
                        .on_click(cx.listener(|viewer, _, _, cx| {
                            viewer.toggle_video_mute(cx);
                        })),
                )
                .child(
                    div()
                        .id("viewer-video-transcript")
                        .flex_none()
                        .size(px(28.))
                        .rounded_md()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .when(transcript_active, move |b| {
                            b.bg(chrome.button_hover)
                                .border_1()
                                .border_color(chrome.border)
                        })
                        .hover(move |b| b.bg(chrome.button_hover))
                        .child(
                            Icon::new(IconName::Captions)
                                .text_color(if transcript_active {
                                    chrome.text
                                } else {
                                    chrome.text_muted
                                })
                                .size(px(15.)),
                        )
                        .on_click(cx.listener(|viewer, _, window, cx| {
                            viewer.toggle_transcript(window, cx);
                        })),
                )
                .into_any_element()
        } else {
            h_flex()
                .items_center()
                .child(
                    div()
                        .id("viewer-zoom-out")
                        .flex_none()
                        .size(px(CONTROL_BUTTON))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .when(zoom_out_enabled, |button| {
                            button.hover(move |button| button.bg(chrome.button))
                        })
                        .child(
                            Icon::new(IconName::Minus)
                                .text_color(if zoom_out_enabled {
                                    chrome.text_muted
                                } else {
                                    chrome.text_dim
                                })
                                .size(px(16.)),
                        )
                        .on_click(cx.listener(|viewer, _, window, cx| {
                            viewer.zoom_step(-1, window, cx);
                        })),
                )
                .child(
                    div()
                        .w(px(SLIDER_WIDTH))
                        .mx_1()
                        .child(Slider::new(&self.slider)),
                )
                .child(
                    div()
                        .id("viewer-zoom-in")
                        .flex_none()
                        .size(px(CONTROL_BUTTON))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .when(zoom_in_enabled, |button| {
                            button.hover(move |button| button.bg(chrome.button))
                        })
                        .child(
                            Icon::new(IconName::Plus)
                                .text_color(if zoom_in_enabled {
                                    chrome.text_muted
                                } else {
                                    chrome.text_dim
                                })
                                .size(px(16.)),
                        )
                        .on_click(cx.listener(|viewer, _, window, cx| {
                            viewer.zoom_step(1, window, cx);
                        })),
                )
                .into_any_element()
        };

        let download_control: AnyElement = match image.download.clone() {
            Some((url, name)) => Self::tile_button("viewer-download", IconName::Download, chrome)
                .on_click(move |_, window, cx| {
                    crate::media::download(url.clone(), name.clone(), window, cx)
                })
                .into_any_element(),
            None => {
                let button: Div = div()
                    .flex_none()
                    .size(px(CONTROL_BUTTON))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .bg(chrome.button)
                    .border_1()
                    .border_color(chrome.border)
                    .opacity(0.35)
                    .child(
                        Icon::new(IconName::Download)
                            .text_color(chrome.text_dim)
                            .size(px(16.)),
                    );
                button.into_any_element()
            }
        };

        let footer = if is_video {
            h_flex()
                .items_center()
                .gap_2()
                .flex_none()
                .px(px(12.))
                .pb(px(12.))
                .pt(px(4.))
                .child(div().flex_1().min_w_0())
                .child(div().flex_none().child(bottom_controls))
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .justify_end()
                        .child(download_control),
                )
        } else {
            h_flex()
                .items_center()
                .justify_between()
                .flex_none()
                .px(px(12.))
                .pb(px(12.))
                .pt(px(4.))
                .child(bottom_controls)
                .child(download_control)
        };

        div()
            .id("image-viewer")
            .occlude()
            .on_scroll(
                cx.listener(|viewer, event: &gpui_kit::gpui::ScrollEvent, window, cx| {
                    cx.stop_propagation();
                    let dy = match event.delta {
                        gpui_kit::gpui::ScrollDelta::Pixels(p) => f32::from(p.y),
                        gpui_kit::gpui::ScrollDelta::Lines(p) => p.y * 24.0,
                    };
                    if dy.abs() >= 12.0 {
                        viewer.zoom_step(if dy > 0.0 { 1 } else { -1 }, window, cx);
                    }
                }),
            )
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(chrome.panel)
            // Header: chip top-left, close X top-right.
            .child(
                h_flex()
                    .items_start()
                    .justify_between()
                    .flex_none()
                    .px(px(12.))
                    .pt(px(12.))
                    .pb(px(4.))
                    .child(chip)
                    .child(
                        div()
                            .id("viewer-close")
                            .flex_none()
                            .size(px(CONTROL_BUTTON))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(chrome.button)
                            .border_1()
                            .border_color(chrome.border)
                            .cursor_pointer()
                            .hover(move |button| button.bg(chrome.button_hover))
                            .child(Icon::new(IconName::X).text_color(chrome.text).size(px(16.)))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(ViewerEvent::Close);
                            })),
                    ),
            )
            // The fitted rendition; prev/next arrows hang at the edges of the free area,
            // with an optional right-side transcript drawer for videos.
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .py(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .relative()
                            .flex()
                            .items_center()
                            .justify_center()
                            .px(px(ARROW_GUTTER))
                            .overflow_hidden()
                            .child(stage_content)
                            .when(multiple, |body| {
                                body.child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .top_0()
                                        .bottom_0()
                                        .w(px(ARROW_GUTTER))
                                        .pl(px(16.))
                                        .flex()
                                        .items_center()
                                        .child(
                                            Self::arrow_button(
                                                "viewer-prev",
                                                IconName::ArrowLeft,
                                                chrome,
                                            )
                                            .on_click(cx.listener(|viewer, _, window, cx| {
                                                viewer.step(-1, window, cx);
                                            }))
                                            .when(!prev_enabled, |button| button.opacity(0.35)),
                                        ),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .right_0()
                                        .top_0()
                                        .bottom_0()
                                        .w(px(ARROW_GUTTER))
                                        .pr(px(16.))
                                        .flex()
                                        .items_center()
                                        .justify_end()
                                        .child(
                                            Self::arrow_button(
                                                "viewer-next",
                                                IconName::ArrowRight,
                                                chrome,
                                            )
                                            .on_click(cx.listener(|viewer, _, window, cx| {
                                                viewer.step(1, window, cx);
                                            }))
                                            .when(!next_enabled, |button| button.opacity(0.35)),
                                        ),
                                )
                            }),
                    )
                    .when_some(transcript_panel, |row, panel| row.child(panel)),
            )
            // Controls: zoom slider bottom-left (images) or centered video transport bar (videos), Download bottom-right.
            .child(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockkit::serde_json::json;
    use slackless_core::types::{File, FileId, Ts};

    #[test]
    fn message_images_collects_videos_alongside_images_in_order() {
        let fixtures = crate::fixtures::load();
        let mut video_file = File {
            id: FileId::from("F_VID1"),
            name: Some("demo.mp4".into()),
            mimetype: Some("video/mp4".into()),
            filetype: Some("mp4".into()),
            thumb_480: Some("https://files.slack.com/tmb/demo_480.jpg".into()),
            url_private: Some("https://files.slack.com/pri/demo.mp4".into()),
            url_private_download: Some("https://files.slack.com/dl/demo.mp4".into()),
            user: Some(UserId::from("U_ALICE")),
            timestamp: Some(1_700_000_123),
            ..Default::default()
        };
        video_file.extra.insert("duration_ms".into(), json!(18_500));
        video_file.extra.insert(
            "mp4".into(),
            json!("https://files.slack.com/stream/demo.mp4"),
        );
        video_file
            .extra
            .insert("transcript".into(), json!("Hello from the demo video"));
        video_file
            .extra
            .insert("vtt".into(), json!("https://files.slack.com/demo.vtt"));

        let image_file = File {
            id: FileId::from("F_IMG1"),
            name: Some("shot.png".into()),
            mimetype: Some("image/png".into()),
            url_private: Some("https://files.slack.com/pri/shot.png".into()),
            ..Default::default()
        };

        let video_no_poster = File {
            id: FileId::from("F_VID2"),
            name: Some("clip.mov".into()),
            filetype: Some("mov".into()),
            thumb_video: Some("https://files.slack.com/tmb/clip.mp4".into()),
            url_private: Some("https://files.slack.com/pri/clip.mov".into()),
            ..Default::default()
        };

        let video_no_url = File {
            id: FileId::from("F_VID_EMPTY"),
            name: Some("broken.mp4".into()),
            mimetype: Some("video/mp4".into()),
            ..Default::default()
        };

        let msg = Message {
            ts: Ts::from("1700000000.000100"),
            user: Some(UserId::from("U_BOB")),
            files: vec![image_file, video_file, video_no_poster, video_no_url],
            ..Default::default()
        };

        let entries = message_images(&fixtures, &msg);
        assert_eq!(entries.len(), 3, "video without playback URL is skipped");

        assert_eq!(entries[0].origin, ImageOrigin::File(0));
        assert!(entries[0].image.video.is_none());
        assert_eq!(entries[0].image.name, "shot.png");

        assert_eq!(entries[1].origin, ImageOrigin::File(1));
        assert_eq!(entries[1].image.name, "demo.mp4");
        assert_eq!(entries[1].image.user, Some(UserId::from("U_ALICE")));
        assert_eq!(entries[1].image.taken_at, Some(1_700_000_123));
        assert_eq!(
            entries[1].image.download,
            Some((
                "https://files.slack.com/dl/demo.mp4".to_owned(),
                "demo.mp4".to_owned()
            ))
        );
        assert!(poster_source_if_present(&entries[1].image.source).is_some());
        assert_eq!(
            entries[1].image.video,
            Some(ViewerVideo {
                file_id: "F_VID1".to_owned(),
                url: "https://files.slack.com/stream/demo.mp4".to_owned(),
                ext: "mp4".to_owned(),
                duration_ms: Some(18_500),
                inline_transcript: Some("Hello from the demo video".to_owned()),
                vtt_url: Some("https://files.slack.com/demo.vtt".to_owned()),
            })
        );

        assert_eq!(entries[2].origin, ImageOrigin::File(2));
        assert!(
            poster_source_if_present(&entries[2].image.source).is_none(),
            "video with only .mp4 thumb_video uses empty placeholder"
        );
        assert_eq!(
            entries[2].image.video,
            Some(ViewerVideo {
                file_id: "F_VID2".to_owned(),
                url: "https://files.slack.com/pri/clip.mov".to_owned(),
                ext: "mov".to_owned(),
                duration_ms: None,
                inline_transcript: None,
                vtt_url: None,
            })
        );

        // `collect_viewer_images` resolves the clicked video origin accurately.
        let (all, ix) = collect_viewer_images(
            &fixtures,
            std::iter::once(&msg),
            &msg.ts,
            &ImageOrigin::File(1),
        )
        .expect("clicked video must be found");
        assert_eq!(all.len(), 3);
        assert_eq!(ix, 1);
        assert_eq!(all[ix].video.as_ref().unwrap().file_id, "F_VID1");
    }

    #[gpui::test]
    fn viewer_steps_between_image_and_video_and_cleans_up_on_close(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let items = Rc::new(vec![
            ViewerImage {
                source: empty_placeholder_source(),
                name: "photo.png".into(),
                user: None,
                taken_at: None,
                download: None,
                video: None,
            },
            ViewerImage {
                source: empty_placeholder_source(),
                name: "demo.mp4".into(),
                user: None,
                taken_at: None,
                download: Some((
                    "https://files.slack.com/dl/demo.mp4".into(),
                    "demo.mp4".into(),
                )),
                video: Some(ViewerVideo {
                    file_id: "F_DEMO".into(),
                    url: "https://files.slack.com/pri/demo.mp4".into(),
                    ext: "mp4".into(),
                    duration_ms: Some(12_000),
                    inline_transcript: Some("Welcome to the walkthrough clip.".into()),
                    vtt_url: None,
                }),
            },
        ]);
        let (viewer, cx) = cx.add_window_view(|window, cx| {
            let entity = ImageViewer::new(
                Arc::new(crate::fixtures::load()),
                "#norce".into(),
                items,
                0,
                window,
                cx,
            );
            // Unwrap the created entity into a host view that mounts `ImageViewer`.
            struct Host(Entity<ImageViewer>);
            impl Render for Host {
                fn render(
                    &mut self,
                    _window: &mut Window,
                    _cx: &mut Context<Self>,
                ) -> impl IntoElement {
                    self.0.clone()
                }
            }
            Host(entity)
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let iv = viewer.read(cx).0.clone();
            assert!(!iv.read(cx).is_video());
            iv.update(cx, |v, cx| v.step(1, window, cx));
            assert!(iv.read(cx).is_video());
            assert!(iv.read(cx).video_playback.is_some());
            assert!(!iv.read(cx).showing_transcript());
            iv.update(cx, |v, cx| v.toggle_transcript(window, cx));
            assert!(iv.read(cx).showing_transcript());
            let _ = window.draw(cx);
            // Stepping back to an image stops and clears video playback immediately and hides the transcript drawer.
            iv.update(cx, |v, cx| v.step(-1, window, cx));
            assert!(!iv.read(cx).is_video());
            assert!(!iv.read(cx).showing_transcript());
            assert!(iv.read(cx).video_playback.is_none());
            // Stepping onto video then closing via `evict_assets` stops playback immediately.
            iv.update(cx, |v, cx| v.step(1, window, cx));
            assert!(iv.read(cx).video_playback.is_some());
            iv.update(cx, |v, cx| v.toggle_transcript(window, cx));
            assert!(!iv.read(cx).showing_transcript());
            iv.update(cx, |v, cx| v.evict_assets(window, cx));
            assert!(iv.read(cx).video_playback.is_none());
        });
        cx.run_until_parked();
    }
}
