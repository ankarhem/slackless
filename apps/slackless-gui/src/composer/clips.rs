//! Native audio and video clip recording lifecycle inside [`Composer`] (`F-CORE-127`, `128`, `131`, `F-COLLAB-099`).

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::gpui::Context;
use slackless_core::huddle::{
    AudioClipRecorder, ClipVideoSource, VideoClipRecorder, finish_audio_clip,
};

use super::{ActiveClipRecorder, ClipReview, Composer, PendingAttachment};

impl Composer {
    fn clear_clip_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(old) = self.clip_preview.take() {
            cx.drop_image(old, None);
        }
    }

    fn clear_clip_review(&mut self, cx: &mut Context<Self>) -> Option<ClipReview> {
        let review = self.clip_review.take()?;
        for thumb in &review.thumbs {
            cx.drop_image(Arc::clone(thumb), None);
        }
        Some(review)
    }

    /// Start recording a native microphone audio clip (`F-CORE-127`, `F-CORE-131`).
    pub fn start_audio_clip(&mut self, cx: &mut Context<Self>) {
        if self.clip_stopping {
            return;
        }
        if self.clip_recorder.is_some() {
            self.stop_and_attach_clip(cx);
            return;
        }
        self.upload_warning = None;
        match AudioClipRecorder::start() {
            Ok(recorder) => {
                self.clip_recorder = Some(ActiveClipRecorder::Audio(recorder));
                self.clear_clip_preview(cx);
                self.spawn_clip_ticker(cx);
                cx.notify();
            }
            Err(err) => {
                self.upload_warning = Some(format!("Audio clip error: {err}"));
                cx.notify();
            }
        }
    }

    /// Start recording a native camera or screen video clip (`F-CORE-128`, `F-CORE-131`).
    pub fn start_video_clip(&mut self, source: ClipVideoSource, cx: &mut Context<Self>) {
        if self.clip_stopping {
            return;
        }
        if self.clip_recorder.is_some() {
            self.stop_and_attach_clip(cx);
            return;
        }
        self.upload_warning = None;
        let is_screen = matches!(source, ClipVideoSource::Screen(_));
        match VideoClipRecorder::start(source) {
            Ok(recorder) => {
                self.clip_recorder = Some(ActiveClipRecorder::Video {
                    recorder,
                    is_screen,
                });
                self.clear_clip_preview(cx);
                self.spawn_clip_ticker(cx);
                cx.notify();
            }
            Err(err) => {
                self.upload_warning = Some(format!("Video clip error: {err}"));
                cx.notify();
            }
        }
    }

    /// Discard an active clip recording without attaching it.
    pub fn cancel_clip(&mut self, cx: &mut Context<Self>) {
        self.clip_ticker = None;
        self.clear_clip_preview(cx);
        self.clip_stopping = false;
        if let Some(active) = self.clip_recorder.take() {
            match active {
                ActiveClipRecorder::Audio(r) => r.cancel(),
                ActiveClipRecorder::Video { recorder, .. } => recorder.cancel(),
            }
        }
        cx.notify();
    }

    /// Stop the active clip recorder, encode off the UI thread, and stage as a composer attachment.
    pub fn stop_and_attach_clip(&mut self, cx: &mut Context<Self>) {
        let Some(active) = self.clip_recorder.take() else {
            return;
        };
        self.clip_ticker = None;
        self.clear_clip_preview(cx);
        self.clip_stopping = true;
        cx.notify();

        match active {
            ActiveClipRecorder::Audio(recorder) => match recorder.stop_to_pcm() {
                Ok((pcm, elapsed_ms)) => {
                    cx.spawn(async move |this, cx| {
                        let res = cx
                            .background_executor()
                            .spawn(async move { finish_audio_clip(pcm, elapsed_ms) })
                            .await;
                        let _ = this.update(cx, |composer, cx| {
                            composer.clip_stopping = false;
                            match res {
                                Ok(clip) => {
                                    composer.attachments.push(PendingAttachment {
                                        filename: clip.filename,
                                        bytes: clip.bytes,
                                        preview: None,
                                    });
                                }
                                Err(err) => {
                                    composer.upload_warning =
                                        Some(format!("Audio clip encoding failed: {err}"));
                                }
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                }
                Err(err) => {
                    self.clip_stopping = false;
                    self.upload_warning = Some(format!("Audio clip error: {err}"));
                    cx.notify();
                }
            },
            ActiveClipRecorder::Video { recorder, .. } => {
                cx.spawn(async move |this, cx| {
                    let res = cx
                        .background_executor()
                        .spawn(async move { recorder.stop() })
                        .await;
                    let _ = this.update(cx, |composer, cx| {
                        composer.clip_stopping = false;
                        match res {
                            Ok(clip) => {
                                let thumbs = clip
                                    .thumbnail_frames
                                    .iter()
                                    .filter_map(crate::huddle_dock::frame_to_render_image)
                                    .collect::<Vec<_>>();
                                if thumbs.is_empty() {
                                    composer.attachments.push(PendingAttachment {
                                        filename: clip.filename,
                                        bytes: clip.bytes,
                                        preview: None,
                                    });
                                } else {
                                    let _ = composer.clear_clip_review(cx);
                                    composer.clip_review = Some(ClipReview {
                                        filename: clip.filename,
                                        bytes: clip.bytes,
                                        duration_ms: clip.duration_ms,
                                        thumbs,
                                        selected_thumb: 0,
                                    });
                                }
                            }
                            Err(err) => {
                                composer.upload_warning =
                                    Some(format!("Video clip encoding failed: {err}"));
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }

    /// Confirm the reviewed video clip (keeping the chosen thumbnail) and attach it (`F-COLLAB-099`).
    pub fn confirm_clip_review(&mut self, cx: &mut Context<Self>) {
        if let Some(review) = self.clear_clip_review(cx) {
            self.attachments.push(PendingAttachment {
                filename: review.filename,
                bytes: review.bytes,
                preview: None,
            });
            cx.notify();
        }
    }

    /// Discard the pending video clip review without attaching.
    pub fn discard_clip_review(&mut self, cx: &mut Context<Self>) {
        let _ = self.clear_clip_review(cx);
        cx.notify();
    }

    fn spawn_clip_ticker(&mut self, cx: &mut Context<Self>) {
        self.clip_ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let keep_going = this
                    .update(cx, |composer, cx| {
                        let Some(active) = composer.clip_recorder.as_ref() else {
                            return false;
                        };
                        let elapsed = match active {
                            ActiveClipRecorder::Audio(rec) => rec.elapsed_ms(),
                            ActiveClipRecorder::Video { recorder, .. } => recorder.elapsed_ms(),
                        };
                        // Enforce Slack's 5-minute clip cap (`F-COLLAB-107`).
                        if elapsed >= slackless_core::huddle::MAX_CLIP_DURATION_MS {
                            composer.stop_and_attach_clip(cx);
                            return false;
                        }
                        if let ActiveClipRecorder::Video { recorder, .. } = active
                            && let Some(vf) = recorder.latest_preview()
                            && let Some(img) = crate::huddle_dock::frame_into_render_image(vf)
                            && let Some(old) = composer.clip_preview.replace(img)
                        {
                            cx.drop_image(old, None);
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        }));
    }
}
