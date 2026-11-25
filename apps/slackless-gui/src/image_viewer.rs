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
    App, AppContext as _, Context, Div, Entity, EventEmitter, FontWeight, ImageSource,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px, relative,
};
use slackless_core::types::{Block, Message, UserId, time};

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

/// One image the viewer can show: the rendition (fixture pixels or a live url the app's
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
        let name = file
            .name
            .clone()
            .or_else(|| file.title.clone())
            .unwrap_or_else(|| file.id.to_string());
        entries.push(ImageEntry {
            origin: ImageOrigin::File(ix),
            image: ViewerImage {
                source,
                download: crate::files::download_url(file).map(|url| (url, name.clone())),
                name,
                user: file.user.clone().or_else(|| author.clone()),
                taken_at: file.timestamp.or(taken_at),
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

/// The viewer overlay: an entity so the root can mount it like the other overlays and
/// drop it (and the large rendition it holds) on close.
pub struct ImageViewer {
    fixtures: Arc<Fixtures>,
    /// The conversation label the header shows ("# design").
    channel: String,
    /// Every image in the conversation (transcript order); only the shown one is painted,
    /// so only it loads.
    images: Arc<Vec<ViewerImage>>,
    index: usize,
    zoom_ix: usize,
    slider: Entity<SliderState>,
    _subscription: Subscription,
}

impl EventEmitter<ViewerEvent> for ImageViewer {}

impl ImageViewer {
    /// Evict decoded full-resolution image textures loaded by this viewer from GPUI's window &
    /// asset caches so closing the lightbox releases RAM and Metal atlas memory immediately.
    pub fn evict_assets(&self, window: &mut Window, cx: &mut Context<Self>) {
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
        images: Arc<Vec<ViewerImage>>,
        index: usize,
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
        cx.new(|cx| {
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
            }
        })
    }

    /// The shown image.
    fn current(&self) -> &ViewerImage {
        &self.images[self.index]
    }

    /// Step prev (-1) / next (+1) through the conversation's images; each image starts
    /// fitted again, like Slack's viewer.
    pub fn step(&mut self, direction: isize, window: &mut Window, cx: &mut Context<Self>) {
        let next = self.index as isize + direction;
        if next < 0 || next >= self.images.len() as isize {
            return;
        }
        self.index = next as usize;
        self.zoom_to(FIT_INDEX, window, cx);
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

    /// Step zoom in (`+1`) or out (`-1`) through `ZOOM_STEPS`.
    pub fn zoom_step(&mut self, direction: isize, window: &mut Window, cx: &mut Context<Self>) {
        let next = (self.zoom_ix as isize + direction).clamp(0, ZOOM_STEPS.len() as isize - 1);
        self.zoom_to(next as usize, window, cx);
    }

    /// Reset zoom back to fit (`100%`).
    pub fn reset_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            // The fitted rendition; prev/next arrows hang at the edges of the free area.
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(ARROW_GUTTER))
                    .py(px(8.))
                    .overflow_hidden()
                    .child(
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
                            ),
                    )
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
                                    Self::arrow_button("viewer-prev", IconName::ArrowLeft, chrome)
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
                                    Self::arrow_button("viewer-next", IconName::ArrowRight, chrome)
                                        .on_click(cx.listener(|viewer, _, window, cx| {
                                            viewer.step(1, window, cx);
                                        }))
                                        .when(!next_enabled, |button| button.opacity(0.35)),
                                ),
                        )
                    }),
            )
            // Controls: zoom out / slider / zoom in bottom-left, Download bottom-right.
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .flex_none()
                    .px(px(12.))
                    .pb(px(12.))
                    .pt(px(4.))
                    .child(
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
                            ),
                    )
                    .child(match image.download.clone() {
                        Some((url, name)) => {
                            Self::tile_button("viewer-download", IconName::Download, chrome)
                                .on_click(move |_, window, cx| {
                                    crate::media::download(url.clone(), name.clone(), window, cx)
                                })
                                .into_any_element()
                        }
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
                    }),
            )
    }
}
