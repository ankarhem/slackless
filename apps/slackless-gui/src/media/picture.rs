//! Shared-clock animated image element ([`Picture`]) for GIF attachments and custom emoji.

use std::time::{Duration, Instant};

use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, Corners, EntityId, Global, ImageSource, ImgResourceLoader, IntoElement,
    ObjectFit, Pixels, RenderImage, RenderOnce, Styled as _, StyledImage as _, Window, canvas, img,
    px,
};

/// An image filling its parent; an animated one (a GIF, an animated custom emoji) plays at
/// its own frame delays. GPUI's animated `img` asks for a frame every vsync, redrawing the
/// window about 120 times a second while one shows (68% CPU for one GIF, against 0.6% idle).
/// A `Picture` paints the frame itself from the same cached decode, picks it from a clock
/// every animation shares, and asks for a redraw only when its frame changes, of only the
/// view it is painted in. All animations share one tick grid, so however many play, they
/// cost at most 20 redraws a second. A picture that is not painted (scrolled away, or in a
/// covered or minimized window, which macOS does not draw) asks for nothing until it is.
#[derive(IntoElement)]
pub struct Picture {
    source: ImageSource,
    fit: ObjectFit,
    radius: Pixels,
    fallback: Option<Box<dyn Fn() -> AnyElement>>,
}

/// Animation redraw grid for full GIF attachments (20 fps — silky smooth, 33% lighter than 30 fps).
const TICK_GIF: Duration = Duration::from_nanos(1_000_000_000 / 20);
/// Animation redraw grid for tiny inline custom emoji (<= 48px: 10 fps matches GIF's standard
/// 100ms frame delay and cuts full-window relayout CPU by 3x while staying crisp at 20x20 px).
const TICK_EMOJI: Duration = Duration::from_nanos(1_000_000_000 / 10);

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
                // Pause animated GIFs/emojis whenever the window is unfocused (0% background CPU;
                // GPUI repaints automatically on window activation, resuming animations seamlessly).
                let is_small_emoji = bounds.size.width <= px(48.) && bounds.size.height <= px(48.);
                let tick = if is_small_emoji { TICK_EMOJI } else { TICK_GIF };
                let epoch = cx.default_global::<Clock>().epoch;
                let ticks = epoch.elapsed().as_nanos() / tick.as_nanos();
                let at = Duration::from_nanos((ticks * tick.as_nanos()) as u64);
                let (frame, shows_for) = frame_at(&data, at);
                let display = crate::preferences::display_prefs(cx);
                let window_active = window.is_window_active();
                let frozen = !display.animate_emoji
                    || display.reduce_motion
                    || cx.reduce_motion()
                    || !window_active;
                let frame = if frozen { 0 } else { frame };
                let image_bounds = fit.get_bounds(bounds, data.size(frame));
                if let Err(e) = window.paint_image(
                    bounds,
                    image_bounds,
                    Corners::all(radius),
                    data,
                    frame,
                    false,
                ) {
                    tracing::warn!("could not paint an animated image: {e}");
                }
                // Only an active, visible picture schedules its next frame.
                if !frozen && window.content_mask().bounds.intersects(&bounds) {
                    schedule(
                        epoch + at + shows_for.max(tick),
                        tick,
                        window.current_view(),
                        cx,
                    );
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

/// Redraw `view` at `due`, rounded up to `tick`. A redraw already scheduled no
/// later takes the view along; a later one moves up, keeping the views it had.
fn schedule(due: Instant, tick: Duration, view: EntityId, cx: &mut App) {
    let clock = cx.default_global::<Clock>();
    let ticks = due
        .duration_since(clock.epoch)
        .as_nanos()
        .div_ceil(tick.as_nanos());
    let at = clock.epoch + Duration::from_nanos((ticks * tick.as_nanos()) as u64);
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Delay, Frame, RgbaImage};

    #[test]
    fn frame_at_clamps_short_delays_and_loops() {
        let img = RenderImage::new(vec![
            // <=10ms delay is clamped to 100ms (browser/Slack behavior).
            Frame::from_parts(RgbaImage::new(1, 1), 0, 0, Delay::from_numer_denom_ms(5, 1)),
            Frame::from_parts(
                RgbaImage::new(1, 1),
                0,
                0,
                Delay::from_numer_denom_ms(200, 1),
            ),
        ]);
        assert_eq!(
            frame_at(&img, Duration::from_millis(0)),
            (0, Duration::from_millis(100))
        );
        assert_eq!(
            frame_at(&img, Duration::from_millis(40)),
            (0, Duration::from_millis(60))
        );
        assert_eq!(
            frame_at(&img, Duration::from_millis(100)),
            (1, Duration::from_millis(200))
        );
        assert_eq!(
            frame_at(&img, Duration::from_millis(250)),
            (1, Duration::from_millis(50))
        );
        // Wraps modulo 300ms total loop duration.
        assert_eq!(
            frame_at(&img, Duration::from_millis(310)),
            (0, Duration::from_millis(90))
        );
    }
}
