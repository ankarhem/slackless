//! Inline audio player cards, video cards, and expandable clip transcripts (`F-CORE-129`).

use std::collections::HashMap;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, Div, FontWeight, Global, ImageSource, ImgResourceLoader,
    InteractiveElement as _, IntoElement, MouseButton, ObjectFit, ParentElement as _, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::types::{ChannelId, File, FileId, TranscriptCue, Ts, UserId};

use crate::fixtures::Fixtures;
use crate::image_viewer::{ImageClick, ImageOrigin};

use super::gallery::download_button;
use super::meta::{
    CARD_W, audio_wave_bars, clip_vtt_url, download_url, file_ext, file_name, format_duration,
    human_size, inline_clip_transcript, media_duration_ms, parse_webvtt, parse_webvtt_cues,
    resolve_video_preview_size, video_dimensions, video_playback_url, video_poster_url,
};

/// Cached clip transcript state (`F-CORE-129`), stored globally so expanded transcripts
/// survive virtualized message list scrolling.
#[derive(Clone, Default)]
struct ClipTranscriptState {
    expanded: bool,
    loading: bool,
    text: Option<String>,
    cues: Vec<TranscriptCue>,
    error: Option<String>,
}

/// Snapshot of a file's cached WebVTT transcript fetch status shared between inline clip cards
/// and [`crate::image_viewer::ImageViewer`].
#[derive(Clone, Default)]
pub(crate) struct ClipTranscriptStatus {
    pub loading: bool,
    pub text: Option<String>,
    pub cues: Vec<TranscriptCue>,
    pub error: Option<String>,
}

#[derive(Default)]
struct ClipTranscriptStore {
    entries: HashMap<FileId, ClipTranscriptState>,
}

impl Global for ClipTranscriptStore {}

#[cfg(test)]
pub(crate) fn set_clip_transcript_expanded(file_id: FileId, expanded: bool, cx: &mut App) {
    cx.default_global::<ClipTranscriptStore>()
        .entries
        .entry(file_id)
        .or_default()
        .expanded = expanded;
}

/// Read the cached WebVTT transcript fetch status for `file_id`.
pub(crate) fn clip_transcript_status(file_id: &FileId, cx: &App) -> ClipTranscriptStatus {
    cx.try_global::<ClipTranscriptStore>()
        .and_then(|store| store.entries.get(file_id))
        .map(|entry| ClipTranscriptStatus {
            loading: entry.loading,
            text: entry.text.clone(),
            cues: entry.cues.clone(),
            error: entry.error.clone(),
        })
        .unwrap_or_default()
}

/// Lazily fetch and cache a Slack clip's WebVTT transcript if not already loaded or in flight.
pub(crate) fn fetch_clip_vtt_if_needed(
    file_id: FileId,
    vtt_url: String,
    window: &mut Window,
    cx: &mut App,
) {
    let should_fetch = {
        let store = cx.default_global::<ClipTranscriptStore>();
        let entry = store.entries.entry(file_id.clone()).or_default();
        if entry.text.is_none() && entry.cues.is_empty() && !entry.loading {
            entry.loading = true;
            entry.error = None;
            true
        } else {
            false
        }
    };
    if !should_fetch {
        return;
    }
    window.refresh();
    let Some(fut) = crate::media::fetch(vtt_url, cx) else {
        let store = cx.default_global::<ClipTranscriptStore>();
        let entry = store.entries.entry(file_id).or_default();
        entry.loading = false;
        entry.error = Some("Transcript requires a signed-in workspace.".to_owned());
        window.refresh();
        return;
    };
    let win = window.window_handle();
    cx.spawn(async move |cx| {
        let res = fut.await;
        cx.update(|cx| {
            let store = cx.default_global::<ClipTranscriptStore>();
            let entry = store.entries.entry(file_id).or_default();
            entry.loading = false;
            match res {
                Ok(bytes) => {
                    let raw = String::from_utf8_lossy(&bytes);
                    let cues = parse_webvtt_cues(&raw);
                    let parsed = if !cues.is_empty() {
                        TranscriptCue::plain_text(&cues)
                    } else {
                        parse_webvtt(&raw)
                    };
                    entry.cues = cues;
                    entry.text = Some(if parsed.is_empty() {
                        raw.trim().to_owned()
                    } else {
                        parsed
                    });
                }
                Err(err) => entry.error = Some(err),
            }
            let _ = win.update(cx, |_, window, _| window.refresh());
        });
    })
    .detach();
}

/// Expandable clip transcript pill + selectable transcript panel (`F-CORE-129`).
fn clip_transcript_section(
    fixtures: &Fixtures,
    file: &File,
    order_base: u64,
    cx: &App,
) -> Option<Div> {
    let stored = fixtures.file_transcript(&file.id).map(str::to_owned);
    let inline = stored.or_else(|| inline_clip_transcript(file));
    let vtt_url = clip_vtt_url(file);
    if inline.is_none() && vtt_url.is_none() {
        return None;
    }
    let theme = cx.theme();
    let file_id = file.id.clone();
    let state = cx
        .try_global::<ClipTranscriptStore>()
        .and_then(|store| store.entries.get(&file_id))
        .cloned()
        .unwrap_or_default();
    let expanded = state.expanded;
    let body_text = state.text.clone().or(inline);
    let toggle_id = SharedString::from(format!("clip-transcript-toggle-{}", file.id));
    let click_id = file.id.clone();
    let click_vtt = vtt_url.clone();
    let has_text = body_text.is_some();

    Some(
        v_flex()
            .w(px(CARD_W))
            .max_w_full()
            .gap_1()
            .child(
                h_flex()
                    .id(toggle_id)
                    .cursor_pointer()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .w(px(120.))
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.muted_foreground)
                    .hover(|el| el.bg(theme.secondary_hover).text_color(theme.foreground))
                    .child(
                        Icon::new(IconName::Captions)
                            .size(px(13.))
                            .text_color(theme.muted_foreground),
                    )
                    .child(if expanded {
                        "Hide transcript"
                    } else {
                        "Transcript"
                    })
                    .on_click(move |_, window, cx| {
                        let expanded_now = {
                            let store = cx.default_global::<ClipTranscriptStore>();
                            let entry = store.entries.entry(click_id.clone()).or_default();
                            entry.expanded = !entry.expanded;
                            entry.expanded
                        };
                        window.refresh();
                        if expanded_now
                            && !has_text
                            && let Some(url) = click_vtt.clone()
                        {
                            fetch_clip_vtt_if_needed(click_id.clone(), url, window, cx);
                        }
                    }),
            )
            .when(expanded, |col| {
                let transcript_body: AnyElement = if state.loading {
                    div()
                        .text_color(theme.muted_foreground)
                        .child("Loading transcript\u{2026}")
                        .into_any_element()
                } else if let Some(text) = body_text {
                    crate::messages::selectable_text(
                        &format!("clip-transcript-{}", file.id),
                        0,
                        text,
                        None,
                        order_base,
                        cx,
                    )
                } else if let Some(err) = state.error {
                    div()
                        .text_color(theme.muted_foreground)
                        .child(format!("Could not load transcript: {err}"))
                        .into_any_element()
                } else {
                    div()
                        .text_color(theme.muted_foreground)
                        .child("No transcript available.")
                        .into_any_element()
                };
                col.child(
                    div()
                        .px_2p5()
                        .py_2()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.secondary.opacity(0.45))
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(transcript_body),
                )
            }),
    )
}

/// Subtle header pill button triggering local on-device transcription when no transcript exists yet.
fn transcribe_pill_button(
    fixtures: &Fixtures,
    file: &File,
    play_url: Option<&str>,
    ext: String,
    cx: &App,
) -> Option<gpui_kit::gpui::Stateful<Div>> {
    let has_cached_vtt = cx
        .try_global::<ClipTranscriptStore>()
        .and_then(|store| store.entries.get(&file.id))
        .is_some_and(|entry| entry.text.is_some());
    let has_transcript = fixtures.file_transcript(&file.id).is_some()
        || inline_clip_transcript(file).is_some()
        || has_cached_vtt;
    if has_transcript {
        return None;
    }
    let url = play_url
        .filter(|u| !u.is_empty() && !crate::media::is_hls_url(u))?
        .to_owned();
    let theme = cx.theme();
    let transcribing = crate::media::is_transcribing(&file.id, cx);
    let fid = file.id.clone();
    Some(
        h_flex()
            .id(SharedString::from(format!("media-transcribe-{}", file.id)))
            .flex_none()
            .items_center()
            .gap_1()
            .px_1p5()
            .py(px(1.))
            .rounded_sm()
            .border_1()
            .border_color(theme.border)
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.muted_foreground)
            .when(!transcribing, |el| {
                el.cursor_pointer()
                    .hover(|h| h.bg(theme.secondary_hover).text_color(theme.foreground))
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Icon::new(IconName::Captions)
                    .size(px(11.))
                    .text_color(theme.muted_foreground),
            )
            .child(if transcribing {
                "Transcribing\u{2026}"
            } else {
                "Transcribe"
            })
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                if !transcribing {
                    crate::media::transcribe_media(
                        fid.clone(),
                        url.clone(),
                        ext.clone(),
                        false,
                        window,
                        cx,
                    );
                }
            }),
    )
}

/// Slack-style inline audio player card for uploaded audio files and voice clips.
pub(super) fn audio_card(fixtures: &Fixtures, file: &File, order_base: u64, cx: &App) -> Div {
    let theme = cx.theme();
    let id_str = file.id.to_string();
    let state = crate::media::audio_state(&id_str, cx);
    let playing = state.as_ref().is_some_and(|s| s.playing);
    let paused = state.as_ref().is_some_and(|s| s.paused);
    let loading = state.as_ref().is_some_and(|s| s.loading);
    let active = playing || paused;
    let rate = state.as_ref().map_or(1.0, |s| s.rate);
    let muted = state.as_ref().is_some_and(|s| s.muted);
    let duration_ms = state
        .as_ref()
        .and_then(|s| s.duration_ms)
        .or_else(|| media_duration_ms(file));
    let duration_secs = state
        .as_ref()
        .map(|s| s.duration_secs)
        .filter(|&d| d > 0.0)
        .unwrap_or_else(|| duration_ms.map_or(0.0, |ms| ms as f64 / 1000.0));
    let position_secs = state.as_ref().map_or(0.0, |s| s.position_secs);
    let progress = if active {
        crate::media::progress_fraction(position_secs, duration_secs)
    } else {
        0.0
    };
    let status_text = if loading {
        "Loading audio\u{2026}".to_owned()
    } else if active {
        let prefix = if paused { "Paused \u{b7} " } else { "" };
        if duration_secs > 0.0 {
            format!(
                "{prefix}{} / {}",
                crate::media::format_video_time(position_secs),
                crate::media::format_video_time(duration_secs)
            )
        } else {
            format!("{prefix}{}", crate::media::format_video_time(position_secs))
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
    let transcribe_btn =
        transcribe_pill_button(fixtures, file, play_url.as_deref(), ext.clone(), cx);
    let play_icon = if playing {
        IconName::Pause
    } else {
        IconName::Play
    };
    let id_seek = id_str.clone();
    let id_rate = id_str.clone();
    let id_mute = id_str.clone();
    let rate_btn = active.then(|| {
        div()
            .id(SharedString::from(format!("audio-rate-{}", file.id)))
            .flex_none()
            .px_1p5()
            .h(px(22.))
            .rounded_sm()
            .border_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(10.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.foreground)
            .cursor_pointer()
            .hover(|b| b.bg(theme.secondary_hover))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(crate::media::format_playback_rate(rate))
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                crate::media::cycle_audio_rate(&id_rate, window, cx);
            })
    });
    let mute_btn = active.then(|| {
        div()
            .id(SharedString::from(format!("audio-mute-{}", file.id)))
            .size(px(24.))
            .flex_none()
            .rounded_sm()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|b| b.bg(theme.secondary_hover))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Icon::new(if muted {
                    IconName::VolumeX
                } else {
                    IconName::Volume2
                })
                .text_color(theme.muted_foreground)
                .size(px(14.)),
            )
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                crate::media::toggle_audio_mute(&id_mute, window, cx);
            })
    });
    let stop_btn = active.then(|| {
        div()
            .id(SharedString::from(format!("audio-stop-{}", file.id)))
            .size(px(24.))
            .flex_none()
            .rounded_sm()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|b| b.bg(theme.secondary_hover))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Icon::new(IconName::Square)
                    .text_color(theme.muted_foreground)
                    .size(px(12.)),
            )
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
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
    let wave = audio_wave_bars(file);
    let has_wave = !wave.is_empty();
    let transcript = clip_transcript_section(fixtures, file, order_base, cx);
    let card = h_flex()
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
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_1p5()
                        .min_w_0()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(file_name(file)),
                        )
                        .children(transcribe_btn),
                )
                .when(has_wave && !active, |col| {
                    let total = wave.len();
                    col.child(h_flex().items_center().gap(px(2.)).h(px(18.)).children(
                        wave.into_iter().enumerate().map(|(ix, amp)| {
                            let filled = (ix as f32 / total.max(1) as f32) <= progress;
                            let h = (4. + amp * 13.).clamp(4., 17.);
                            div()
                                .w(px(3.))
                                .h(px(h))
                                .rounded_full()
                                .bg(if filled && active {
                                    theme.primary
                                } else {
                                    theme.foreground.opacity(0.28)
                                })
                        }),
                    ))
                })
                .when(active, |col| {
                    col.child(h_flex().w_full().items_center().child(
                        crate::media::video_scrub_bar(
                            SharedString::from(format!("audio-scrub-{}", file.id)),
                            progress,
                            duration_secs,
                            theme.border,
                            theme.primary,
                            move |secs, window, cx| {
                                crate::media::seek_audio(&id_seek, secs, window, cx);
                            },
                        ),
                    ))
                })
                .when(!active && !has_wave && duration_ms.is_some(), |col| {
                    col.child(
                        div()
                            .w_full()
                            .h(px(4.))
                            .rounded_full()
                            .bg(theme.border)
                            .overflow_hidden(),
                    )
                })
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_1p5()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(status_text),
                        )
                        .when(active, |row| {
                            row.child(
                                h_flex()
                                    .flex_none()
                                    .items_center()
                                    .gap_1()
                                    .children(rate_btn)
                                    .children(mute_btn)
                                    .children(stop_btn),
                            )
                        }),
                ),
        )
        .children(dl_btn);
    v_flex().gap_1().child(card).children(transcript)
}

/// Compact icon button inside the inline video player's bottom transport overlay.
fn inline_transport_button(
    id: SharedString,
    icon: IconName,
    icon_size: f32,
) -> gpui_kit::gpui::Stateful<Div> {
    let fg = crate::theme::media_overlay_foreground();
    div()
        .id(id)
        .size(px(24.))
        .flex_none()
        .rounded_sm()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|b| b.bg(gpui_kit::gpui::hsla(0., 0., 1., 0.16)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(Icon::new(icon).size(px(icon_size)).text_color(fg))
}

/// Decoded image dimensions from GPUI's asset cache when available (lets video previews without
/// Slack dimension metadata size to the poster's true aspect ratio as soon as it loads).
fn loaded_image_dims(
    source: &ImageSource,
    window: &mut Window,
    cx: &mut App,
) -> Option<(i64, i64)> {
    match source {
        ImageSource::Resource(resource) => {
            let data = window
                .use_asset::<ImgResourceLoader>(resource, cx)
                .and_then(Result::ok)?;
            let sz = data.size(0);
            let (w, h) = (i64::from(sz.width.0), i64::from(sz.height.0));
            (w > 0 && h > 0).then_some((w, h))
        }
        ImageSource::Render(data) => {
            let sz = data.size(0);
            let (w, h) = (i64::from(sz.width.0), i64::from(sz.height.0));
            (w > 0 && h > 0).then_some((w, h))
        }
        ImageSource::Image(img) if img.format == gpui_kit::gpui::ImageFormat::Png => {
            let (w, h) = image::ImageReader::with_format(
                std::io::Cursor::new(&img.bytes),
                image::ImageFormat::Png,
            )
            .into_dimensions()
            .ok()?;
            (w > 0 && h > 0).then_some((i64::from(w), i64::from(h)))
        }
        _ => None,
    }
}

/// Top-right overlay icon button (matching [`download_button`]) used in active inline video frames.
fn top_overlay_action_button(
    id: SharedString,
    icon: IconName,
    cx: &App,
) -> gpui_kit::gpui::Stateful<Div> {
    let theme = cx.theme();
    div()
        .id(id)
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .bg(theme.background)
        .border_1()
        .border_color(theme.border)
        .cursor_pointer()
        .hover(|button| button.bg(theme.secondary))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(Icon::new(icon).size(px(15.)).text_color(theme.foreground))
}

/// Width threshold below which the inline video transport switches to a compact two-row layout
/// so portrait videos never clip transport buttons.
const COMPACT_TRANSPORT_MAX_W: f32 = 260.;

/// Aspect-ratio-aware video preview / active inline player element rendered via `RenderOnce` so
/// it can inspect decoded poster dimensions from `Window` when Slack omits video dimension fields.
#[derive(IntoElement)]
struct VideoFrame {
    file: File,
    ix: usize,
    channel: Option<ChannelId>,
    message_ts: Option<Ts>,
    message_user: Option<UserId>,
    message_taken_at: Option<i64>,
    poster_source: Option<ImageSource>,
    video_url: Option<String>,
    ext: String,
    meta_duration_ms: Option<u64>,
    on_open: Option<ImageClick>,
}

impl RenderOnce for VideoFrame {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let file = self.file;
        let id_str = file.id.to_string();
        let poster_dims = self
            .poster_source
            .as_ref()
            .and_then(|src| loaded_image_dims(src, window, cx));
        let snap = crate::media::inline_video_snapshot(&id_str, cx);
        let theme = cx.theme();
        let (w, h) = resolve_video_preview_size(
            video_dimensions(&file),
            poster_dims,
            snap.as_ref().map(|s| (s.width, s.height)),
        );
        let duration_label = self.meta_duration_ms.map(format_duration);
        let group = SharedString::from(format!("video-tile-{}", file.id));

        if let Some(snap) = snap {
            let fg = crate::theme::media_overlay_foreground();
            let dur_secs = if snap.duration_secs > 0.0 {
                snap.duration_secs
            } else {
                self.meta_duration_ms.map_or(0.0, |ms| ms as f64 / 1000.0)
            };
            let progress = crate::media::progress_fraction(snap.position_secs, dur_secs);
            let time_text = if dur_secs > 0.0 {
                format!(
                    "{} / {}",
                    crate::media::format_video_time(snap.position_secs),
                    crate::media::format_video_time(dur_secs)
                )
            } else {
                crate::media::format_video_time(snap.position_secs)
            };
            let keep_controls_visible = snap.loading || snap.failed || !snap.playing || snap.ended;

            let id_surface = id_str.clone();
            let id_play = id_str.clone();
            let id_seek = id_str.clone();
            let id_rate = id_str.clone();
            let id_mute = id_str.clone();
            let id_expand = id_str.clone();
            let id_close = id_str.clone();

            let play_pause_btn = inline_transport_button(
                SharedString::from(format!("video-inline-play-{}", file.id)),
                if snap.playing {
                    IconName::Pause
                } else {
                    IconName::Play
                },
                14.,
            )
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                crate::media::toggle_inline_video_pause(&id_play, window, cx);
            });

            let scrub = crate::media::video_scrub_bar(
                SharedString::from(format!("video-inline-scrub-{}", file.id)),
                progress,
                dur_secs,
                gpui_kit::gpui::hsla(0., 0., 1., 0.28),
                fg,
                move |secs, window, cx| {
                    crate::media::seek_inline_video(&id_seek, secs, window, cx);
                },
            );

            let rate_btn = div()
                .id(SharedString::from(format!("video-inline-rate-{}", file.id)))
                .flex_none()
                .px_1()
                .h(px(22.))
                .rounded_sm()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg)
                .cursor_pointer()
                .hover(|b| b.bg(gpui_kit::gpui::hsla(0., 0., 1., 0.16)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(crate::media::format_playback_rate(snap.rate))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    crate::media::cycle_inline_video_rate(&id_rate, window, cx);
                });

            let mute_btn = inline_transport_button(
                SharedString::from(format!("video-inline-mute-{}", file.id)),
                if snap.muted {
                    IconName::VolumeX
                } else {
                    IconName::Volume2
                },
                14.,
            )
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                crate::media::toggle_inline_video_mute(&id_mute, window, cx);
            });

            let close_btn = inline_transport_button(
                SharedString::from(format!("video-inline-close-{}", file.id)),
                IconName::X,
                13.,
            )
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                crate::media::stop_inline_video(&id_close, window, cx);
            });

            let expand_top_btn = self.on_open.clone().map(|on_open| {
                let origin = ImageOrigin::File(self.ix);
                top_overlay_action_button(
                    SharedString::from(format!("video-inline-expand-{}", file.id)),
                    IconName::Maximize2,
                    cx,
                )
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    crate::media::stop_inline_video_for_expand(&id_expand, window, cx);
                    on_open(origin.clone(), window, cx);
                })
            });

            let download_top_btn = download_url(&file).map(|url| {
                let name = file_name(&file);
                top_overlay_action_button(
                    SharedString::from(format!("file-download-{}", file.id)),
                    IconName::Download,
                    cx,
                )
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    crate::media::download(url.clone(), name.clone(), window, cx);
                })
            });

            let top_actions = (expand_top_btn.is_some() || download_top_btn.is_some()).then(|| {
                let mut cluster = h_flex()
                    .absolute()
                    .top(px(8.))
                    .right(px(8.))
                    .items_center()
                    .gap_1()
                    .children(expand_top_btn)
                    .children(download_top_btn);
                if !keep_controls_visible {
                    cluster = cluster
                        .opacity(0.)
                        .group_hover(group.clone(), |bar| bar.opacity(1.));
                }
                cluster
            });

            let mut transport = if w < COMPACT_TRANSPORT_MAX_W {
                v_flex()
                    .id(SharedString::from(format!("video-inline-bar-{}", file.id)))
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .px_1p5()
                    .py_1()
                    .bg(crate::theme::media_overlay_scrim_strong())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(h_flex().w_full().items_center().child(scrub))
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_0p5()
                            .child(play_pause_btn)
                            .child(rate_btn)
                            .child(mute_btn)
                            .child(close_btn),
                    )
            } else {
                h_flex()
                    .id(SharedString::from(format!("video-inline-bar-{}", file.id)))
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .bg(crate::theme::media_overlay_scrim_strong())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(play_pause_btn)
                    .child(scrub)
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(fg)
                            .child(time_text),
                    )
                    .child(rate_btn)
                    .child(mute_btn)
                    .child(close_btn)
            };
            if !keep_controls_visible {
                transport = transport
                    .opacity(0.)
                    .group_hover(group.clone(), |bar| bar.opacity(1.));
            }

            let center_overlay = if snap.loading {
                Some(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .px_2p5()
                                .py_1()
                                .rounded_full()
                                .bg(crate::theme::media_overlay_scrim_strong())
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg)
                                .child("Loading\u{2026}"),
                        ),
                )
            } else if snap.failed {
                Some(
                    div()
                        .absolute()
                        .inset_0()
                        .pb(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .px_2p5()
                                .py_1()
                                .rounded_full()
                                .bg(crate::theme::media_overlay_scrim_strong())
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg)
                                .child("Playback failed"),
                        ),
                )
            } else if !snap.playing || snap.ended {
                Some(
                    div()
                        .absolute()
                        .inset_0()
                        .pb(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .size(px(40.))
                                .rounded_full()
                                .bg(crate::theme::media_overlay_scrim())
                                .border_1()
                                .border_color(crate::theme::media_overlay_border())
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Icon::new(IconName::Play).size(px(18.)).text_color(fg)),
                        ),
                )
            } else {
                None
            };

            let visibility_probe = crate::media::inline_video_visibility_probe(id_str.clone());

            div()
                .id(SharedString::from(format!("video-active-{}", file.id)))
                .group(group)
                .relative()
                .w(px(w))
                .h(px(h))
                .rounded_lg()
                .overflow_hidden()
                .border_1()
                .border_color(theme.foreground.opacity(0.13))
                .bg(theme.secondary)
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    crate::media::toggle_inline_video_pause(&id_surface, window, cx);
                })
                .child(crate::media::render_video_surface(
                    snap.surface,
                    self.poster_source,
                    px(8.),
                ))
                .child(visibility_probe)
                .children(center_overlay)
                .child(transport)
                .children(top_actions)
        } else {
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
            if let Some(on_open) = self.on_open {
                let origin = ImageOrigin::File(self.ix);
                frame = frame
                    .cursor_pointer()
                    .on_click(move |_, window, cx| on_open(origin.clone(), window, cx));
            }
            if let Some(poster) = self.poster_source {
                frame = frame.child(
                    crate::media::Picture::new(poster)
                        .fit(ObjectFit::Contain)
                        .radius(px(8.)),
                );
            }
            let mut play_circle = div()
                .id(SharedString::from(format!("video-play-btn-{}", file.id)))
                .size(px(48.))
                .rounded_full()
                .bg(crate::theme::media_overlay_scrim())
                .border_1()
                .border_color(crate::theme::media_overlay_border())
                .flex()
                .items_center()
                .justify_center()
                .hover(|b| b.bg(crate::theme::media_overlay_scrim_strong()))
                .child(
                    Icon::new(IconName::Play)
                        .size(px(22.))
                        .text_color(crate::theme::media_overlay_foreground()),
                );
            if let Some(url) = self.video_url {
                let ext = self.ext;
                let title = file_name(&file);
                let meta = crate::media::InlineVideoMeta {
                    file_id: id_str.clone(),
                    title: title.clone(),
                    channel: self.channel,
                    message_ts: self.message_ts,
                    url,
                    ext,
                    duration_ms: self.meta_duration_ms,
                    poster_url: video_poster_url(&file),
                    inline_transcript: inline_clip_transcript(&file),
                    vtt_url: clip_vtt_url(&file),
                    download: download_url(&file).map(|dl| (dl, title)),
                    user: file.user.clone().or(self.message_user),
                    taken_at: file.timestamp.or(self.message_taken_at),
                };
                play_circle = play_circle
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        crate::media::play_inline_video_with_meta(meta.clone(), None, window, cx);
                    });
            }
            let play_badge = div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(play_circle);

            frame
                .child(play_badge)
                .when_some(duration_label, |el, dur| {
                    el.child(
                        div()
                            .absolute()
                            .bottom(px(8.))
                            .right(px(8.))
                            .px_1p5()
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(crate::theme::media_overlay_scrim_strong())
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(crate::theme::media_overlay_foreground())
                            .child(dur),
                    )
                })
                .children(download_button(&file, group, cx))
        }
    }
}

/// Slack-style inline video card: clicking the centered Play button plays natively right inside
/// the chat card with compact transport controls; clicking anywhere else on the poster preview
/// (or clicking the top-right Expand button while playing) opens the video inside
/// [`crate::image_viewer::ImageViewer`].
#[allow(clippy::too_many_arguments)]
pub(super) fn video_card(
    fixtures: &Fixtures,
    file: &File,
    ix: usize,
    channel: Option<ChannelId>,
    message_ts: Option<Ts>,
    message_user: Option<UserId>,
    message_taken_at: Option<i64>,
    order_base: u64,
    on_open: Option<&ImageClick>,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let poster_source = match fixtures.thumbnails.get(&file.id) {
        Some(image) => Some(ImageSource::Image(image.clone())),
        None => video_poster_url(file).map(|p| ImageSource::from(SharedString::from(p))),
    };
    let video_url = video_playback_url(file);
    let ext = file_ext(file, "mp4");
    let transcribe_btn =
        transcribe_pill_button(fixtures, file, video_url.as_deref(), ext.clone(), cx);
    let meta_duration_ms = media_duration_ms(file);

    v_flex()
        .gap_1()
        .child(
            h_flex()
                .gap_1p5()
                .items_center()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(file_name(file))
                .child(
                    Icon::new(IconName::ChevronDown)
                        .text_color(theme.muted_foreground)
                        .size(px(12.)),
                )
                .children(transcribe_btn),
        )
        .child(VideoFrame {
            file: file.clone(),
            ix,
            channel,
            message_ts,
            message_user,
            message_taken_at,
            poster_source,
            video_url,
            ext,
            meta_duration_ms,
            on_open: on_open.cloned(),
        })
        .children(clip_transcript_section(fixtures, file, order_base, cx))
}
