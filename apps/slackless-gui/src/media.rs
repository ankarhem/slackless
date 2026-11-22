//! Remote images (profile photos, bot icons, custom emoji, the workspace icon) render with
//! GPUI's `img(url)`, which fetches through the app's `HttpClient` and decodes and caches the
//! result. This is that client: requests run on the core's reqwest client and tokio runtime,
//! so the app keeps a single HTTP stack. Images that may animate (GIFs, animated custom emoji)
//! render as a [`Picture`].

use std::fs::OpenOptions;
use std::future::Future;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, Instant};

use gpui_kit::gpui::http_client::http::HeaderValue;
use gpui_kit::gpui::http_client::{AsyncBody, HttpClient, Request, Response, Url};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, Corners, EntityId, Global, ImageSource, ImgResourceLoader, IntoElement,
    ObjectFit, Pixels, RenderImage, RenderOnce, Styled as _, StyledImage as _, Window, canvas, img,
    px,
};
use slackless_core::Fetcher;

pub struct CoreHttp(pub Fetcher);

impl HttpClient for CoreHttp {
    fn user_agent(&self) -> Option<&HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        request: Request<AsyncBody>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Response<AsyncBody>>> + Send>> {
        let bytes = self.0.get(request.uri().to_string());
        Box::pin(async move {
            let bytes = bytes.await.map_err(anyhow::Error::msg)?;
            Ok(Response::builder()
                .status(200)
                .body(AsyncBody::from(bytes))?)
        })
    }
}

/// Saves message files into the user's Downloads folder, as Slack's app does, through the
/// core's signed-in client (Slack's file hosts need the session cookie).
struct Downloader(Fetcher);

impl Global for Downloader {}

/// Live mode: let [`download`] fetch through `fetcher`.
pub fn enable_downloads(fetcher: Fetcher, cx: &mut App) {
    cx.set_global(Downloader(fetcher));
}

/// Fetch `url` through the core's client (live mode; `None` with nothing to fetch with).
pub fn fetch(
    url: String,
    cx: &App,
) -> Option<impl Future<Output = Result<Vec<u8>, String>> + Send + 'static + use<>> {
    Some(cx.try_global::<Downloader>()?.0.get(url))
}

/// Download `url` into ~/Downloads as `name` (`name (1).png` when that is taken), then say
/// where it went. Fixture mode has nothing to fetch with and says so.
pub fn download(url: String, name: String, window: &mut Window, cx: &mut App) {
    let Some(fetcher) = cx.try_global::<Downloader>().map(|d| d.0.clone()) else {
        crate::actions::notify(
            "Download",
            "Downloads need a signed-in workspace.",
            window,
            cx,
        );
        return;
    };
    let bytes = fetcher.get(url);
    window
        .spawn(cx, async move |cx| {
            let saved = match bytes.await {
                Ok(bytes) => {
                    cx.background_executor()
                        .spawn(async move { save_download(&name, &bytes) })
                        .await
                }
                Err(e) => Err(e),
            };
            cx.update(|window, cx| match saved {
                Ok(path) => {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    let message = format!("{name} is in your Downloads folder.");
                    crate::actions::notify("Downloaded", message, window, cx);
                }
                Err(e) => crate::actions::notify("Download failed", e, window, cx),
            })
            .ok();
        })
        .detach();
}

/// Writes `bytes` to ~/Downloads/`name`, numbering the name like a browser when it is taken.
fn save_download(name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("no home folder")?;
    let dir = PathBuf::from(home).join("Downloads");
    // A name from Slack never reaches outside the folder.
    let name = Path::new(name)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_owned());
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_owned(), format!(".{ext}")),
        _ => (name.clone(), String::new()),
    };
    let mut path = dir.join(&name);
    for n in 1.. {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(bytes).map_err(|e| e.to_string())?;
                return Ok(path);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                path = dir.join(format!("{stem} ({n}){ext}"));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    unreachable!("the numbering never runs out")
}

/// An image filling its parent; an animated one (a GIF, an animated custom emoji) plays at
/// its own frame delays. GPUI's animated `img` asks for a frame every vsync, redrawing the
/// window about 120 times a second while one shows (68% CPU for one GIF, against 0.6% idle).
/// A `Picture` paints the frame itself from the same cached decode, picks it from a clock
/// every animation shares, and asks for a redraw only when its frame changes, of only the
/// view it is painted in. All animations share one [`TICK`] grid, so however many play, they
/// cost at most 30 redraws a second. A picture that is not painted (scrolled away, or in a
/// covered or minimized window, which macOS does not draw) asks for nothing until it is.
#[derive(IntoElement)]
pub struct Picture {
    source: ImageSource,
    fit: ObjectFit,
    radius: Pixels,
    fallback: Option<Box<dyn Fn() -> AnyElement>>,
}

/// Animation redraws land on this grid: frame changes due within one tick share a redraw.
const TICK: Duration = Duration::from_nanos(1_000_000_000 / 30);

/// The clock animated [`Picture`]s pick their frame from, and the next redraw: when it lands
/// and the views it notifies.
struct Clock {
    epoch: Instant,
    next: Option<(Instant, Vec<EntityId>)>,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            next: None,
        }
    }
}

impl Global for Clock {}

impl Picture {
    pub fn new(source: impl Into<ImageSource>) -> Self {
        Self {
            source: source.into(),
            fit: ObjectFit::Contain,
            radius: px(0.),
            fallback: None,
        }
    }

    pub fn fit(mut self, fit: ObjectFit) -> Self {
        self.fit = fit;
        self
    }

    pub fn radius(mut self, radius: Pixels) -> Self {
        self.radius = radius;
        self
    }

    /// What shows when the image fails to load.
    pub fn fallback(mut self, fallback: impl Fn() -> AnyElement + 'static) -> Self {
        self.fallback = Some(Box::new(fallback));
        self
    }
}

impl RenderOnce for Picture {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let loaded = match &self.source {
            ImageSource::Resource(resource) => window
                .use_asset::<ImgResourceLoader>(resource, cx)
                .and_then(Result::ok),
            ImageSource::Render(data) => Some(data.clone()),
            _ => None,
        };
        // Still images, and any image while it loads or fails, stay GPUI's own img.
        let Some(data) = loaded.filter(|data| data.frame_count() > 1) else {
            return img(self.source)
                .size_full()
                .object_fit(self.fit)
                .rounded(self.radius)
                .when_some(self.fallback, |image, fallback| {
                    image.with_fallback(fallback)
                })
                .into_any_element();
        };
        let (fit, radius) = (self.fit, self.radius);
        canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                // Time snapped to the tick grid, so every view drawn within one tick shows
                // the same frame; with Reduce Motion on, the first frame stays.
                let epoch = cx.default_global::<Clock>().epoch;
                let ticks = epoch.elapsed().as_nanos() / TICK.as_nanos();
                let at = Duration::from_nanos((ticks * TICK.as_nanos()) as u64);
                let (frame, shows_for) = frame_at(&data, at);
                let frame = if cx.reduce_motion() { 0 } else { frame };
                let image_bounds = fit.get_bounds(bounds, data.size(frame));
                if let Err(e) = window.paint_image(
                    bounds,
                    image_bounds,
                    Corners::all(radius),
                    data,
                    frame,
                    false,
                ) {
                    eprintln!("slackless: could not paint an animated image ({e})");
                }
                // Only a picture that shows asks for its next frame.
                if !cx.reduce_motion() && window.content_mask().bounds.intersects(&bounds) {
                    schedule(epoch + at + shows_for, window.current_view(), cx);
                }
            },
        )
        .size_full()
        .into_any_element()
    }
}

/// The frame showing `elapsed` into `data`'s loop, and how much longer it shows.
fn frame_at(data: &RenderImage, elapsed: Duration) -> (usize, Duration) {
    // Browsers play a delay of 10ms or less as 100ms; so does Slack.
    let delay = |frame| match Duration::from(data.delay(frame)) {
        delay if delay <= Duration::from_millis(10) => Duration::from_millis(100),
        delay => delay,
    };
    let frames = data.frame_count();
    let total: Duration = (0..frames).map(delay).sum();
    let mut left = Duration::from_nanos((elapsed.as_nanos() % total.as_nanos()) as u64);
    for frame in 0..frames {
        let shows = delay(frame);
        if left < shows {
            return (frame, shows - left);
        }
        left -= shows;
    }
    (0, delay(0))
}

/// Redraw `view` at `due`, rounded up to the [`TICK`] grid. A redraw already scheduled no
/// later takes the view along; a later one moves up, keeping the views it had.
fn schedule(due: Instant, view: EntityId, cx: &mut App) {
    let clock = cx.default_global::<Clock>();
    let ticks = due
        .duration_since(clock.epoch)
        .as_nanos()
        .div_ceil(TICK.as_nanos());
    let at = clock.epoch + Duration::from_nanos((ticks * TICK.as_nanos()) as u64);
    let views = match &mut clock.next {
        Some((next, views)) if *next <= at => {
            if !views.contains(&view) {
                views.push(view);
            }
            return;
        }
        next => {
            let mut views = next.take().map(|(_, views)| views).unwrap_or_default();
            if !views.contains(&view) {
                views.push(view);
            }
            views
        }
    };
    clock.next = Some((at, views));
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(at.saturating_duration_since(Instant::now()))
            .await;
        cx.update(|cx| {
            let clock = cx.global_mut::<Clock>();
            // A redraw moved up meanwhile took this one's views along.
            if clock.next.as_ref().is_some_and(|(next, _)| *next == at) {
                let views = clock
                    .next
                    .take()
                    .map(|(_, views)| views)
                    .unwrap_or_default();
                for view in views {
                    cx.notify(view);
                }
            }
        });
    })
    .detach();
}
