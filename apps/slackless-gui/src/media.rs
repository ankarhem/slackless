//! Remote images (profile photos, bot icons, custom emoji, the workspace icon) render with
//! GPUI's `img(url)`, which fetches through the app's `HttpClient` and decodes and caches the
//! result. This is that client: requests run on the core's reqwest client and tokio runtime,
//! so the app keeps a single HTTP stack. Images that may animate (GIFs, animated custom emoji)
//! render as a [`Picture`].

use std::future::Future;
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
