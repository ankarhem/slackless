//! The video transcript drawer: a view of its own that the viewer mounts cached, so the
//! playback tick, which repaints the viewer's transport bar every frame, never rebuilds it. It
//! resolves the transcript when it renders (opened, new fixtures, or a fetched WebVTT or a
//! finished transcription refreshing the window) and repaints when the cue under the playhead
//! changes. The cues are a virtual list: a long recording has hundreds.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FontWeight,
    InteractiveElement as _, IntoElement, ListAlignment, ListState, MouseButton, MouseDownEvent,
    MouseUpEvent, ParentElement as _, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, WeakEntity, Window, div, list,
    px,
};
use slackless_core::types::{FileId, TranscriptCue};

use crate::fixtures::Fixtures;
use crate::theme::{self, ViewerChrome};

use super::{ImageViewer, ViewerMedia, ViewerVideo};

/// Width of the toggleable right-side video transcript drawer.
const TRANSCRIPT_PANEL_WIDTH: f32 = 320.;
/// How far past its edges the transcript's cue list lays rows out ahead of a scroll.
const CUE_LIST_OVERDRAW: f32 = 200.;
/// Disjoint selection document order for the overlay's selectable transcript text.
const VIEWER_TRANSCRIPT_ORDER: u64 = 1u64 << 62;

/// What the transcript drawer asks of the viewer.
enum TranscriptEvent {
    /// Seek the video to a cue's start, in seconds.
    Seek(f64),
    Close,
    Summarize {
        file_id: FileId,
        title: String,
        text: String,
        cues: Vec<TranscriptCue>,
    },
}

impl ImageViewer {
    /// Toggle visibility of the right-side video transcript drawer; lazily fetches WebVTT when
    /// needed.
    pub fn toggle_transcript(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_video() {
            return;
        }
        self.show_transcript = !self.show_transcript;
        self.sync_transcript(window, cx);
        if self.show_transcript {
            self.ensure_transcript_loaded(window, cx);
        }
        cx.notify();
    }

    /// Mount the transcript drawer for the shown video while it is open (a fresh one per
    /// video), and drop it otherwise.
    pub(super) fn sync_transcript(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let video = match &self.current().media {
            ViewerMedia::Video(video) if self.show_transcript => Some(video.clone()),
            ViewerMedia::Video(_) | ViewerMedia::Image | ViewerMedia::Pdf(_) => None,
        };
        let Some(video) = video else {
            self.transcript = None;
            return;
        };
        if self
            .transcript
            .as_ref()
            .is_some_and(|(transcript, _)| transcript.read(cx).video.file_id == video.file_id)
        {
            return;
        }
        let (fixtures, title) = (self.fixtures.clone(), self.current().name.clone());
        let transcript = cx.new(|_| ViewerTranscript::new(fixtures, video, title));
        let subscription = cx.subscribe_in(&transcript, window, Self::on_transcript_event);
        self.transcript = Some((transcript, subscription));
    }

    fn on_transcript_event(
        &mut self,
        _: &Entity<ViewerTranscript>,
        event: &TranscriptEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TranscriptEvent::Seek(secs) => self.seek_video(*secs, window, cx),
            TranscriptEvent::Close => self.toggle_transcript(window, cx),
            TranscriptEvent::Summarize {
                file_id,
                title,
                text,
                cues,
            } => cx.emit(super::ViewerEvent::SummarizeTranscript {
                file_id: file_id.clone(),
                title: title.clone(),
                text: text.clone(),
                cues: cues.clone(),
            }),
        }
    }

    pub(super) fn ensure_transcript_loaded(&self, window: &mut Window, cx: &mut App) {
        let ViewerMedia::Video(video) = &self.current().media else {
            return;
        };
        let fid = FileId::from(video.file_id.clone());
        let has_timed_cues = self.fixtures.file_transcript_cues(&fid).is_some()
            || !crate::files::clip_transcript_status(&fid, cx)
                .cues
                .is_empty();
        // Fetch VTT whenever we do not yet have timed cues (upgrading plain inline previews to
        // clickable timestamped cues).
        if !has_timed_cues && let Some(vtt_url) = video.vtt_url.as_ref() {
            crate::files::fetch_clip_vtt_if_needed(fid, vtt_url.clone(), window, cx);
        }
    }

    /// The drawer's slot at the right of the stage while it is open.
    pub(super) fn render_transcript_panel(&self) -> Option<AnyElement> {
        // The drawer is a cached view of its own: the tick repaints the viewer (this transport)
        // every frame without rebuilding the cue list.
        self.transcript.as_ref().map(|(transcript, _)| {
            div()
                .flex_none()
                .w(px(TRANSCRIPT_PANEL_WIDTH))
                .h_full()
                .mr(px(12.))
                .child(
                    transcript
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                )
                .into_any_element()
        })
    }
}

/// A video's transcript drawer: the plain transcript, or its virtual list of timestamped cues.
pub(super) struct ViewerTranscript {
    fixtures: Arc<Fixtures>,
    video: ViewerVideo,
    /// The video's name, for Summarize.
    title: String,
    /// The transcript as the last render resolved it.
    pub(super) text: Option<SharedString>,
    pub(super) cues: Rc<[TranscriptCue]>,
    list: ListState,
    pub(super) active_cue: Option<usize>,
    copied: bool,
    /// A pending single-click mouse-down `(cue_index, position)`: a click on a cue seeks, a drag
    /// selecting its text does not.
    cue_mouse_down: Option<(usize, Point<Pixels>)>,
}

impl EventEmitter<TranscriptEvent> for ViewerTranscript {}

impl ViewerTranscript {
    fn new(fixtures: Arc<Fixtures>, video: ViewerVideo, title: String) -> Self {
        Self {
            fixtures,
            video,
            title,
            text: None,
            cues: Vec::new().into(),
            list: ListState::new(0, ListAlignment::Top, px(CUE_LIST_OVERDRAW)),
            active_cue: None,
            copied: false,
            cue_mouse_down: None,
        }
    }

    /// New workspace fixtures (a transcript landing live), resolved on the next render.
    pub(super) fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        cx.notify();
    }

    /// Follow the playhead, repainting only when the cue under it changes.
    pub(super) fn set_playhead(&mut self, position: f64, playing: bool, cx: &mut Context<Self>) {
        let active = (playing || position > 0.0)
            .then(|| TranscriptCue::active_index(&self.cues, position))
            .flatten();
        if active != self.active_cue {
            self.active_cue = active;
            cx.notify();
        }
    }

    /// The transcript the fixtures, a fetched WebVTT or the clip's inline preview hold, whether
    /// a WebVTT is loading, and why it failed.
    fn resolve(&self, cx: &App) -> (Option<String>, Vec<TranscriptCue>, bool, Option<String>) {
        let fid = FileId::from(self.video.file_id.clone());
        let cached = crate::files::clip_transcript_status(&fid, cx);
        let cues = self
            .fixtures
            .file_transcript_cues(&fid)
            .map(<[TranscriptCue]>::to_vec)
            .unwrap_or(cached.cues);
        let text = self
            .fixtures
            .file_transcript(&fid)
            .map(str::to_owned)
            .or(cached.text)
            .or_else(|| (!cues.is_empty()).then(|| TranscriptCue::plain_text(&cues)))
            .or_else(|| self.video.inline_transcript.clone());
        (text, cues, cached.loading, cached.error)
    }

    /// Record a cue mouse-down; a double or triple click (selecting a word or the line) is no
    /// click on the cue.
    pub(super) fn record_cue_mouse_down(
        &mut self,
        cue_ix: usize,
        click_count: usize,
        pos: Point<Pixels>,
    ) {
        self.cue_mouse_down = (click_count == 1).then_some((cue_ix, pos));
    }

    /// Whether a mouse-up on cue `cue_ix` ends a click on it, within the click threshold of its
    /// mouse-down, rather than a drag selecting text.
    pub(super) fn take_cue_click(&mut self, cue_ix: usize, pos: Point<Pixels>) -> bool {
        let Some((down_ix, origin)) = self.cue_mouse_down.take() else {
            return false;
        };
        let dx: f32 = (pos.x - origin.x).into();
        let dy: f32 = (pos.y - origin.y).into();
        down_ix == cue_ix && dx.abs() < 4.0 && dy.abs() < 4.0
    }

    fn cue_released(&mut self, cue_ix: usize, pos: Point<Pixels>, cx: &mut Context<Self>) {
        if self.take_cue_click(cue_ix, pos)
            && let Some(cue) = self.cues.get(cue_ix)
        {
            cx.emit(TranscriptEvent::Seek(cue.start_secs));
        }
    }

    fn summarize(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = &self.text {
            cx.emit(TranscriptEvent::Summarize {
                file_id: FileId::from(self.video.file_id.clone()),
                title: self.title.clone(),
                text: text.to_string(),
                cues: self.cues.to_vec(),
            });
        }
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = &self.text {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
            self.copied = true;
            cx.notify();
        }
    }

    fn header(&self, chrome: ViewerChrome, cx: &mut Context<Self>) -> impl IntoElement {
        let copied = self.copied;
        let has_text = self.text.is_some();
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
                    .when(has_text, |row| {
                        row.child(
                            h_flex()
                                .id("viewer-transcript-summarize")
                                .items_center()
                                .gap_1()
                                .px_1p5()
                                .py(px(2.))
                                .rounded_sm()
                                .text_xs()
                                .text_color(chrome.text)
                                .cursor_pointer()
                                .hover(move |b| b.bg(chrome.button_hover))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(
                                    Icon::new(IconName::Sparkles)
                                        .size(px(12.))
                                        .text_color(chrome.text),
                                )
                                .child("Summarize")
                                .on_click(cx.listener(|transcript, _, _, cx| {
                                    transcript.summarize(cx);
                                })),
                        )
                        .child(
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
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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
                                .on_click(cx.listener(|transcript, _, _, cx| transcript.copy(cx))),
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
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                Icon::new(IconName::X)
                                    .size(px(13.))
                                    .text_color(chrome.text_muted),
                            )
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(TranscriptEvent::Close))),
                    ),
            )
    }

    /// What shows without timed cues: the plain transcript, its loading or transcription state,
    /// the error, or the Transcribe button.
    fn message(&self, state: (bool, Option<String>), chrome: ViewerChrome, cx: &App) -> AnyElement {
        let (loading_vtt, vtt_error) = state;
        let fid = FileId::from(self.video.file_id.clone());
        let transcribing = crate::media::is_transcribing(&fid, cx);
        if let Some(text) = &self.text {
            return crate::messages::selectable_text(
                &format!("viewer-transcript-{}", self.video.file_id),
                0,
                text.to_string(),
                Some(chrome.text),
                VIEWER_TRANSCRIPT_ORDER,
                cx,
            );
        }
        let status = if loading_vtt {
            Some("Loading transcript\u{2026}".to_owned())
        } else if transcribing {
            Some("Transcribing\u{2026}".to_owned())
        } else {
            vtt_error.map(|err| format!("Could not load transcript: {err}"))
        };
        if let Some(status) = status {
            return div()
                .text_color(chrome.text_muted)
                .child(status)
                .into_any_element();
        }
        let can_transcribe =
            !self.video.url.is_empty() && !crate::media::is_hls_url(&self.video.url);
        let (url, ext) = (self.video.url.clone(), self.video.ext.clone());
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
                                fid.clone(),
                                url.clone(),
                                ext.clone(),
                                false,
                                window,
                                cx,
                            );
                        }),
                )
            })
            .into_any_element()
    }
}

impl Render for ViewerTranscript {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = theme::viewer_chrome();
        let (text, cues, loading_vtt, vtt_error) = self.resolve(cx);
        if *self.cues != *cues {
            self.list.reset(cues.len());
            self.cues = cues.into();
            self.active_cue = None;
        }
        self.text = text.map(SharedString::from);
        let body = if self.cues.is_empty() {
            div()
                .id("viewer-transcript-text")
                .size_full()
                .overflow_y_scroll()
                .px_3()
                .py_2p5()
                .child(self.message((loading_vtt, vtt_error), chrome, cx))
                .into_any_element()
        } else {
            let rows = CueRows {
                cues: self.cues.clone(),
                active: self.active_cue,
                key: SharedString::from(self.video.file_id.clone()),
                transcript: cx.weak_entity(),
                chrome,
            };
            list(self.list.clone(), move |ix, _, cx| rows.row(ix, cx))
                .size_full()
                .px_3()
                .py_2p5()
                .into_any_element()
        };
        v_flex()
            .id("viewer-transcript-panel")
            .size_full()
            .rounded_lg()
            .bg(chrome.glass)
            .border_1()
            .border_color(chrome.border)
            .overflow_hidden()
            .child(self.header(chrome, cx))
            .child(
                div()
                    .id("viewer-transcript-body")
                    .flex_1()
                    .min_h_0()
                    .text_xs()
                    .line_height(px(18.))
                    .text_color(chrome.text)
                    .on_scroll(|_, _, cx| cx.stop_propagation())
                    .child(body),
            )
    }
}

/// What the cue list's rows render from, built per drawer render and owned by the list.
struct CueRows {
    cues: Rc<[TranscriptCue]>,
    active: Option<usize>,
    /// The video's file id, keying each cue's selectable text.
    key: SharedString,
    transcript: WeakEntity<ViewerTranscript>,
    chrome: ViewerChrome,
}

impl CueRows {
    fn row(&self, ix: usize, cx: &App) -> AnyElement {
        let Some(cue) = self.cues.get(ix) else {
            return div().into_any_element();
        };
        let chrome = self.chrome;
        let is_active = self.active == Some(ix);
        let start_secs = cue.start_secs;
        let text = crate::messages::selectable_text(
            &format!("viewer-transcript-{}-{ix}", self.key),
            0,
            cue.text.clone(),
            Some(if is_active {
                chrome.text
            } else {
                chrome.text_muted
            }),
            VIEWER_TRANSCRIPT_ORDER + (ix as u64 + 1) * 16,
            cx,
        );
        let (pressed, released, seek) = (
            self.transcript.clone(),
            self.transcript.clone(),
            self.transcript.clone(),
        );
        h_flex()
            .id(("viewer-cue", ix))
            .items_start()
            .gap_2()
            .mb_1()
            .px_2()
            .py_1()
            .rounded_md()
            .border_l_2()
            .border_color(if is_active {
                chrome.text
            } else {
                gpui_kit::gpui::transparent_black()
            })
            .when(is_active, |row| row.bg(chrome.button_hover))
            .hover(move |row| row.bg(chrome.button))
            .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, _, cx| {
                let _ = pressed.update(cx, |transcript, _| {
                    transcript.record_cue_mouse_down(ix, ev.click_count, ev.position);
                });
            })
            .on_mouse_up(MouseButton::Left, move |ev: &MouseUpEvent, _, cx| {
                let _ = released.update(cx, |transcript, cx| {
                    transcript.cue_released(ix, ev.position, cx);
                });
            })
            .child(
                div()
                    .id(("viewer-cue-ts", ix))
                    .flex_none()
                    .px_1p5()
                    .py(px(1.))
                    .rounded_sm()
                    .bg(if is_active {
                        chrome.button
                    } else {
                        gpui_kit::gpui::transparent_black()
                    })
                    .text_size(px(11.))
                    .font_weight(if is_active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if is_active {
                        chrome.text
                    } else {
                        chrome.text_dim
                    })
                    .cursor_pointer()
                    .hover(move |b| b.bg(chrome.button_hover).text_color(chrome.text))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        let _ = seek.update(cx, |_, cx| cx.emit(TranscriptEvent::Seek(start_secs)));
                    })
                    .child(TranscriptCue::format_timestamp(start_secs)),
            )
            .child(div().flex_1().min_w_0().child(text))
            .into_any_element()
    }
}
