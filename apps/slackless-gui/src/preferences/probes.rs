//! Background audio-level and camera preview probe lifecycle for Preferences -> Audio & video.

use gpui_kit::gpui::Context;

use super::{Preferences, Section};

impl Preferences {
    /// Called when the Preferences window is re-opened on its remembered section: resumes the
    /// background audio probe only if currently viewing Audio & video.
    pub fn on_reopen(&mut self, cx: &mut Context<Self>) {
        if self.section == Section::AudioVideo && self.mic_probe.is_none() && !self.audio_loading {
            self.refresh_audio_devices_and_probe(cx);
        }
    }

    /// Release the live microphone & camera preview probes off the UI thread (called when leaving
    /// Audio & video or closing Preferences so hardware teardown never stalls the main thread).
    pub fn stop_mic_probe(&mut self, cx: &mut Context<Self>) {
        self.audio_probe_gen = self.audio_probe_gen.wrapping_add(1);
        self.audio_loading = false;
        if let Some(old) = self.mic_probe.take() {
            std::thread::spawn(move || drop(old));
        }
        self.mic_level = 0.0;
        self.stop_camera_preview(cx);
    }

    pub(super) fn stop_camera_preview(&mut self, cx: &mut Context<Self>) {
        self.camera_probe_gen = self.camera_probe_gen.wrapping_add(1);
        self.camera_preview_on = false;
        if let Some(old) = self.camera_probe.take() {
            std::thread::spawn(move || drop(old));
        }
        if let Some(old) = self.camera_frame.take() {
            cx.drop_image(old, None);
        }
    }

    pub(super) fn start_camera_preview(&mut self, cx: &mut Context<Self>) {
        self.stop_camera_preview(cx);
        #[cfg(test)]
        {
            let _ = cx;
        }
        #[cfg(not(test))]
        {
            self.camera_preview_on = true;
            let probe_gen = self.camera_probe_gen;
            let device_index = self.settings.huddle_camera_device;
            let blur = self.settings.huddle_blur_background;
            cx.spawn(async move |this, cx| {
                let probe = cx
                    .background_executor()
                    .spawn(async move {
                        slackless_core::huddle::CameraPreviewProbe::start(device_index, blur).ok()
                    })
                    .await;

                let started = this
                    .update(cx, |prefs, cx| {
                        if prefs.camera_probe_gen != probe_gen
                            || prefs.section != Section::AudioVideo
                        {
                            if let Some(stale) = probe {
                                std::thread::spawn(move || drop(stale));
                            }
                            return false;
                        }
                        prefs.camera_probe = probe;
                        if prefs.camera_probe.is_none() {
                            prefs.camera_preview_on = false;
                        }
                        cx.notify();
                        prefs.camera_probe.is_some()
                    })
                    .unwrap_or(false);

                if !started {
                    return;
                }
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(80))
                        .await;
                    let keep = this
                        .update(cx, |prefs, cx| {
                            if prefs.camera_probe_gen != probe_gen {
                                return false;
                            }
                            if let Some(probe) = &prefs.camera_probe {
                                probe.set_blur(prefs.settings.huddle_blur_background);
                                if let Some(frame) = probe.latest_frame()
                                    && let Some(img) =
                                        crate::huddle_video::frame_to_render_image(&frame)
                                {
                                    if let Some(old) = prefs.camera_frame.replace(img) {
                                        cx.drop_image(old, None);
                                    }
                                    cx.notify();
                                }
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false);
                    if !keep {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    pub(super) fn refresh_audio_devices_and_probe(&mut self, cx: &mut Context<Self>) {
        self.audio_probe_gen = self.audio_probe_gen.wrapping_add(1);
        if let Some(old) = self.mic_probe.take() {
            std::thread::spawn(move || drop(old));
        }
        #[cfg(test)]
        {
            self.audio_loading = false;
            cx.notify();
        }
        #[cfg(not(test))]
        {
            let probe_gen = self.audio_probe_gen;
            self.audio_loading = true;
            let wanted = self.settings.huddle_input_device.clone();
            cx.spawn(async move |this, cx| {
                let (devices, cameras, probe) = cx
                    .background_executor()
                    .spawn(async move {
                        let devices = slackless_core::huddle::list_audio_devices();
                        let cameras = slackless_core::huddle::list_cameras();
                        let probe =
                            slackless_core::huddle::MicLevelProbe::start(wanted.as_deref()).ok();
                        (devices, cameras, probe)
                    })
                    .await;

                let started = this
                    .update(cx, |prefs, cx| {
                        if prefs.audio_probe_gen != probe_gen
                            || prefs.section != Section::AudioVideo
                        {
                            if let Some(stale) = probe {
                                std::thread::spawn(move || drop(stale));
                            }
                            return false;
                        }
                        prefs.audio_devices = devices;
                        prefs.cameras = cameras;
                        prefs.audio_loading = false;
                        prefs.mic_probe = probe;
                        cx.notify();
                        prefs.mic_probe.is_some()
                    })
                    .unwrap_or(false);

                if !started {
                    return;
                }
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(80))
                        .await;
                    let keep = this
                        .update(cx, |prefs, cx| {
                            if prefs.audio_probe_gen != probe_gen {
                                return false;
                            }
                            if let Some(probe) = &prefs.mic_probe {
                                prefs.mic_level = probe.level();
                                cx.notify();
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false);
                    if !keep {
                        break;
                    }
                }
            })
            .detach();
        }
    }
}
