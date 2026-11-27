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
//!
//! [`item`] is what the viewer can show and how a transcript click maps onto it,
//! [`playback`] (with [`video`] and [`pdf`]) owns the shown item's live playback, [`stage`],
//! [`controls`] and [`render`] paint the overlay, and [`transcript`] is the video transcript
//! drawer.

mod controls;
mod item;
mod pdf;
mod playback;
mod render;
mod stage;
#[cfg(test)]
mod tests;
mod transcript;
mod video;

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::gpui::{App, AppContext as _, Context, Entity, EventEmitter, Subscription, Window};
use slackless_core::types::{FileId, TranscriptCue};

use crate::fixtures::Fixtures;
use playback::Playback;
use transcript::ViewerTranscript;

pub(crate) use item::relative_stamp;
pub use item::{
    ImageClick, ImageOrigin, ViewerImage, ViewerMedia, ViewerVideo, collect_viewer_images,
};

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

/// What the overlay tells the root: the close X was pressed (Escape is intercepted by the
/// root itself).
pub enum ViewerEvent {
    Close,
    SummarizeTranscript {
        file_id: FileId,
        title: String,
        text: String,
        cues: Vec<TranscriptCue>,
    },
}

/// Result of routing a keystroke to an open [`ImageViewer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerKeyOutcome {
    /// Caller should close the viewer and stop propagation.
    Close,
    /// Viewer handled the keystroke; caller should stop propagation.
    Handled,
    /// Keystroke was not consumed by the viewer (e.g., `Cmd+C` to copy transcript text).
    Ignored,
}

/// The viewer overlay: an entity so the root can mount it like the other overlays and
/// drop it (and the large rendition / video player it holds) on close.
pub struct ImageViewer {
    fixtures: Arc<Fixtures>,
    /// The conversation label the header shows ("# design").
    channel: String,
    /// Every image/video/PDF in the conversation (transcript order); only the shown one is painted,
    /// so only it loads.
    images: Rc<Vec<ViewerImage>>,
    index: usize,
    zoom_ix: usize,
    slider: Entity<SliderState>,
    _subscription: Subscription,
    /// Counts every playback start and stop, so a load or tick task captured under an older
    /// generation lands nowhere.
    media_gen: u64,
    /// The shown item's live playback: a video's player or a PDF's page, nothing for a still image.
    playback: Option<Playback>,
    /// Whether the transcript drawer is open for the shown video (kept across items).
    show_transcript: bool,
    /// The shown video's transcript drawer while it is open.
    transcript: Option<(Entity<ViewerTranscript>, Subscription)>,
}

impl Drop for ImageViewer {
    fn drop(&mut self) {
        self.stop_playback();
    }
}

impl EventEmitter<ViewerEvent> for ImageViewer {}

impl ImageViewer {
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
                media_gen: 0,
                playback: None,
                show_transcript: false,
                transcript: None,
            }
        });
        viewer.update(cx, |v, cx| v.sync_current_media(window, cx));
        viewer
    }

    /// The shown item.
    fn current(&self) -> &ViewerImage {
        &self.images[self.index]
    }

    /// Total number of items in the viewer's gallery list.
    pub fn item_count(&self) -> usize {
        self.images.len()
    }

    /// Update workspace fixtures (e.g. when a newly transcribed video's transcript lands live).
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        if let Some((transcript, _)) = &self.transcript {
            transcript.update(cx, |transcript, cx| {
                transcript.set_fixtures(fixtures.clone(), cx);
            });
        }
        self.fixtures = fixtures;
        cx.notify();
    }

    /// Whether the currently shown item is a playable video.
    pub fn is_video(&self) -> bool {
        matches!(self.current().media, ViewerMedia::Video(_))
    }

    /// Whether the currently shown item is a multi-page PDF document.
    pub fn is_pdf(&self) -> bool {
        matches!(self.current().media, ViewerMedia::Pdf(_))
    }

    /// Handle keyboard shortcuts while the viewer is open, isolating image/video/PDF controls from
    /// the underlying transcript and leaving system chords (such as `Cmd+C`) untouched.
    pub fn handle_keystroke(
        &mut self,
        keystroke: &gpui_kit::gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ViewerKeyOutcome {
        let mods = keystroke.modifiers;
        let no_cmd = !mods.platform && !mods.control && !mods.alt;
        match keystroke.key.as_str() {
            "escape" => ViewerKeyOutcome::Close,
            "left" if mods.shift && no_cmd => {
                if self.is_video() {
                    self.seek_video_relative(-5.0, window, cx);
                    ViewerKeyOutcome::Handled
                } else if self.is_pdf() {
                    let _ = self.step_pdf_page(-1, window, cx);
                    ViewerKeyOutcome::Handled
                } else {
                    ViewerKeyOutcome::Ignored
                }
            }
            "right" if mods.shift && no_cmd => {
                if self.is_video() {
                    self.seek_video_relative(5.0, window, cx);
                    ViewerKeyOutcome::Handled
                } else if self.is_pdf() {
                    let _ = self.step_pdf_page(1, window, cx);
                    ViewerKeyOutcome::Handled
                } else {
                    ViewerKeyOutcome::Ignored
                }
            }
            "left" if no_cmd && !mods.shift => {
                if self.is_pdf() && self.item_count() == 1 {
                    let _ = self.step_pdf_page(-1, window, cx);
                } else {
                    self.step(-1, window, cx);
                }
                ViewerKeyOutcome::Handled
            }
            "right" if no_cmd && !mods.shift => {
                if self.is_pdf() && self.item_count() == 1 {
                    let _ = self.step_pdf_page(1, window, cx);
                } else {
                    self.step(1, window, cx);
                }
                ViewerKeyOutcome::Handled
            }
            "pageup" | "[" if no_cmd => {
                if self.is_pdf() {
                    let _ = self.step_pdf_page(-1, window, cx);
                    ViewerKeyOutcome::Handled
                } else {
                    ViewerKeyOutcome::Ignored
                }
            }
            "pagedown" | "]" if no_cmd => {
                if self.is_pdf() {
                    let _ = self.step_pdf_page(1, window, cx);
                    ViewerKeyOutcome::Handled
                } else {
                    ViewerKeyOutcome::Ignored
                }
            }
            "up" if no_cmd && !mods.shift => {
                if self.is_video() {
                    self.adjust_video_volume(0.1, cx);
                }
                ViewerKeyOutcome::Handled
            }
            "down" if no_cmd && !mods.shift => {
                if self.is_video() {
                    self.adjust_video_volume(-0.1, cx);
                }
                ViewerKeyOutcome::Handled
            }
            "+" | "=" if !mods.control && !mods.alt => {
                self.zoom_step(1, window, cx);
                ViewerKeyOutcome::Handled
            }
            "-" | "_" if !mods.control && !mods.alt => {
                self.zoom_step(-1, window, cx);
                ViewerKeyOutcome::Handled
            }
            "0" if !mods.control && !mods.alt => {
                self.reset_zoom(window, cx);
                ViewerKeyOutcome::Handled
            }
            "space" if no_cmd && !mods.shift => {
                if self.is_video() {
                    self.toggle_video_play(window, cx);
                }
                ViewerKeyOutcome::Handled
            }
            "c" if no_cmd && !mods.shift => {
                if self.is_video() {
                    self.toggle_transcript(window, cx);
                    ViewerKeyOutcome::Handled
                } else {
                    ViewerKeyOutcome::Ignored
                }
            }
            "m" if no_cmd && !mods.shift => {
                if self.is_video() {
                    self.toggle_video_mute(cx);
                    ViewerKeyOutcome::Handled
                } else {
                    ViewerKeyOutcome::Ignored
                }
            }
            "j" if no_cmd && !mods.shift => {
                if self.is_video() {
                    self.seek_video_relative(-5.0, window, cx);
                    ViewerKeyOutcome::Handled
                } else {
                    ViewerKeyOutcome::Ignored
                }
            }
            "l" if no_cmd && !mods.shift => {
                if self.is_video() {
                    self.seek_video_relative(5.0, window, cx);
                    ViewerKeyOutcome::Handled
                } else {
                    ViewerKeyOutcome::Ignored
                }
            }
            _ => ViewerKeyOutcome::Ignored,
        }
    }

    /// Step prev (-1) / next (+1) through the conversation's images/videos; each item starts
    /// fitted again, like Slack's viewer.
    pub fn step(&mut self, direction: isize, window: &mut Window, cx: &mut Context<Self>) {
        let next = self.index as isize + direction;
        if next < 0 || next >= self.images.len() as isize {
            return;
        }
        self.index = next as usize;
        self.zoom_to(FIT_INDEX, window, cx);
        self.sync_current_media(window, cx);
        self.sync_transcript(window, cx);
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
}
