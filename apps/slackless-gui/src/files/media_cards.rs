//! Inline audio player cards, video cards, and expandable clip transcripts (`F-CORE-129`).

use std::collections::HashMap;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, Div, FontWeight, Global, ImageSource, InteractiveElement as _, MouseButton, ObjectFit,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use slackless_core::types::{File, FileId};

use crate::image_viewer::{ImageClick, ImageOrigin};

use super::gallery::download_button;
use super::meta::{
    CARD_W, audio_wave_bars, clip_vtt_url, download_url, file_ext, file_name, fit, format_duration,
    human_size, inline_clip_transcript, media_duration_ms, parse_webvtt, video_playback_url,
    video_poster_url,
};

/// Cached clip transcript state (`F-CORE-129`), stored globally so expanded transcripts
/// survive virtualized message list scrolling.
#[derive(Clone, Default)]
struct ClipTranscriptState {
    expanded: bool,
    loading: bool,
    text: Option<String>,
    error: Option<String>,
}

#[derive(Default)]
struct ClipTranscriptStore {
    entries: HashMap<FileId, ClipTranscriptState>,
}

impl Global for ClipTranscriptStore {}

/// Expandable clip transcript pill + transcript panel (`F-CORE-129`).
fn clip_transcript_section(file: &File, cx: &App) -> Option<Div> {
    let inline = inline_clip_transcript(file);
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
                        let store = cx.default_global::<ClipTranscriptStore>();
                        let entry = store.entries.entry(click_id.clone()).or_default();
                        entry.expanded = !entry.expanded;
                        let should_fetch = entry.expanded
                            && !has_text
                            && entry.text.is_none()
                            && !entry.loading
                            && click_vtt.is_some();
                        if should_fetch {
                            entry.loading = true;
                            entry.error = None;
                        }
                        window.refresh();
                        if should_fetch
                            && let (Some(url), Some(fut)) = (
                                click_vtt.clone(),
                                click_vtt.clone().and_then(|u| crate::media::fetch(u, cx)),
                            )
                        {
                            let _ = url;
                            let fid = click_id.clone();
                            let win = window.window_handle();
                            cx.spawn(async move |cx| {
                                let res = fut.await;
                                cx.update(|cx| {
                                    let store = cx.default_global::<ClipTranscriptStore>();
                                    let entry = store.entries.entry(fid).or_default();
                                    entry.loading = false;
                                    match res {
                                        Ok(bytes) => {
                                            let raw = String::from_utf8_lossy(&bytes);
                                            let parsed = parse_webvtt(&raw);
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
                    }),
            )
            .when(expanded, |col| {
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
                        .child(if state.loading {
                            "Loading transcript\u{2026}".to_owned()
                        } else if let Some(err) = state.error {
                            format!("Could not load transcript: {err}")
                        } else {
                            body_text.unwrap_or_else(|| "No transcript available.".to_owned())
                        }),
                )
            }),
    )
}

/// Slack-style inline audio player card for uploaded audio files and voice clips.
pub(super) fn audio_card(file: &File, cx: &App) -> Div {
    let theme = cx.theme();
    let id_str = file.id.to_string();
    let state = crate::media::audio_state(&id_str, cx);
    let playing = state.as_ref().is_some_and(|s| s.playing);
    let paused = state.as_ref().is_some_and(|s| s.paused);
    let loading = state.as_ref().is_some_and(|s| s.loading);
    let duration_ms = state
        .as_ref()
        .and_then(|s| s.duration_ms)
        .or_else(|| media_duration_ms(file));
    let elapsed_ms = state.as_ref().map(|s| s.elapsed_ms).unwrap_or(0);
    let progress = match duration_ms {
        Some(total) if total > 0 && (playing || paused) => {
            (elapsed_ms as f32 / total as f32).clamp(0., 1.)
        }
        _ => 0.,
    };
    let status_text = if loading {
        "Loading audio\u{2026}".to_owned()
    } else if playing || paused {
        let prefix = if paused { "Paused \u{b7} " } else { "" };
        match duration_ms {
            Some(total) => format!(
                "{prefix}{} / {}",
                format_duration(elapsed_ms),
                format_duration(total)
            ),
            None => format!("{prefix}{}", format_duration(elapsed_ms)),
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
    let play_icon = if playing {
        IconName::Pause
    } else {
        IconName::Play
    };
    let stop_btn = (playing || paused).then(|| {
        div()
            .id(SharedString::from(format!("audio-stop-{}", file.id)))
            .size(px(28.))
            .flex_none()
            .rounded_sm()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|b| b.bg(theme.secondary_hover))
            .child(
                Icon::new(IconName::Square)
                    .text_color(theme.muted_foreground)
                    .size(px(13.)),
            )
            .on_click(move |_, window, cx| {
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
    let transcript = clip_transcript_section(file, cx);
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
                    div()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(file_name(file)),
                )
                .when(has_wave, |col| {
                    let total = wave.len();
                    col.child(h_flex().items_center().gap(px(2.)).h(px(18.)).children(
                        wave.into_iter().enumerate().map(|(ix, amp)| {
                            let filled = (ix as f32 / total.max(1) as f32) <= progress;
                            let h = (4. + amp * 13.).clamp(4., 17.);
                            div().w(px(3.)).h(px(h)).rounded_full().bg(
                                if filled && (playing || paused) {
                                    theme.primary
                                } else {
                                    theme.foreground.opacity(0.28)
                                },
                            )
                        }),
                    ))
                })
                .when(!has_wave && (playing || duration_ms.is_some()), |col| {
                    col.child(
                        div()
                            .w_full()
                            .h(px(4.))
                            .rounded_full()
                            .bg(theme.border)
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(px((220. * progress).max(if playing { 6. } else { 0. })))
                                    .rounded_full()
                                    .bg(theme.primary),
                            ),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(status_text),
                ),
        )
        .children(stop_btn)
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

/// Slack-style inline video card: clicking the centered Play button plays natively right inside
/// the chat card with compact transport controls; clicking anywhere else on the poster preview
/// (or clicking Expand while playing) opens the video inside [`crate::image_viewer::ImageViewer`].
pub(super) fn video_card(file: &File, ix: usize, on_open: Option<&ImageClick>, cx: &App) -> Div {
    let theme = cx.theme();
    let id_str = file.id.to_string();
    let snap = crate::media::inline_video_snapshot(&id_str, cx);
    let (w, h) = match (file.original_w, file.original_h) {
        (Some(_), Some(_)) => fit(file.original_w, file.original_h),
        _ => match snap.as_ref().filter(|s| s.width > 0 && s.height > 0) {
            Some(s) => fit(Some(s.width as i64), Some(s.height as i64)),
            None => fit(file.original_w, file.original_h),
        },
    };
    let poster_url = video_poster_url(file);
    let poster_source = poster_url
        .clone()
        .map(|p| ImageSource::from(SharedString::from(p)));
    let video_url = video_playback_url(file);
    let ext = file_ext(file, "mp4");
    let meta_duration_ms = media_duration_ms(file);
    let duration_label = meta_duration_ms.map(format_duration);
    let group = SharedString::from(format!("video-tile-{}", file.id));

    let frame = if let Some(snap) = snap {
        let fg = crate::theme::media_overlay_foreground();
        let dur_secs = if snap.duration_secs > 0.0 {
            snap.duration_secs
        } else {
            meta_duration_ms.map_or(0.0, |ms| ms as f64 / 1000.0)
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

        let expand_btn = on_open.cloned().map(|on_open| {
            let origin = ImageOrigin::File(ix);
            inline_transport_button(
                SharedString::from(format!("video-inline-expand-{}", file.id)),
                IconName::Maximize2,
                13.,
            )
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                crate::media::stop_inline_video_for_expand(&id_expand, window, cx);
                on_open(origin.clone(), window, cx);
            })
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

        let mut transport = h_flex()
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
            .child(mute_btn)
            .children(expand_btn)
            .child(close_btn);
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

        div()
            .id(SharedString::from(format!("video-active-{}", file.id)))
            .group(group.clone())
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
                poster_source,
                px(8.),
            ))
            .children(center_overlay)
            .child(transport)
            .children(download_button(file, group, cx))
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
        if let Some(on_open) = on_open.cloned() {
            let origin = ImageOrigin::File(ix);
            frame = frame
                .cursor_pointer()
                .on_click(move |_, window, cx| on_open(origin.clone(), window, cx));
        }
        if let Some(poster) = poster_source {
            frame = frame.child(
                crate::media::Picture::new(poster)
                    .fit(ObjectFit::Cover)
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
        if let Some(url) = video_url {
            play_circle = play_circle
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    crate::media::play_inline_video(
                        id_str.clone(),
                        url.clone(),
                        ext.clone(),
                        None,
                        window,
                        cx,
                    );
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
            .children(download_button(file, group, cx))
    };

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
        .child(frame)
        .children(clip_transcript_section(file, cx))
}
