//! Preferences > Audio & video section view.

use super::super::*;

impl Preferences {
    fn device_choice_button(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        selected: bool,
    ) -> Button {
        let btn = Button::new(id).with_size(Size::Small).label(label);
        if selected {
            btn.primary().icon(IconName::Check)
        } else {
            btn.outline()
        }
    }

    pub(super) fn audio_video(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (border, muted, success, card_bg) = (
            theme.border,
            theme.muted_foreground,
            theme.success,
            theme.secondary.opacity(0.35),
        );
        let active_in = self.settings.huddle_input_device.clone();
        let active_out = self.settings.huddle_output_device.clone();
        let meter_pct = (self.mic_level * 3.5).clamp(0.0, 1.0);
        let mic_active = self.mic_probe.is_some();

        let mic_picker = v_flex()
            .gap_2p5()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .child(
                        Self::device_choice_button(
                            "prefs-mic-default",
                            "System default",
                            active_in.is_none(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.huddle_input_device = None;
                            this.refresh_audio_devices_and_probe(cx);
                            this.changed(cx);
                        })),
                    )
                    .children(
                        self.audio_devices
                            .inputs
                            .iter()
                            .enumerate()
                            .map(|(ix, dev)| {
                                let selected = active_in.as_deref() == Some(dev.name.as_str());
                                let name = dev.name.clone();
                                Self::device_choice_button(
                                    ("prefs-mic", ix),
                                    dev.name.clone(),
                                    selected,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.settings.huddle_input_device = Some(name.clone());
                                        this.refresh_audio_devices_and_probe(cx);
                                        this.changed(cx);
                                    },
                                ))
                            }),
                    ),
            )
            .when(
                self.audio_loading && self.audio_devices.inputs.is_empty(),
                |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Detecting audio devices…"),
                    )
                },
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .bg(card_bg)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(div().size(px(7.)).rounded_full().bg(if mic_active {
                                success
                            } else {
                                muted
                            }))
                            .child(div().text_xs().text_color(muted).child("Input level")),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(6.))
                            .rounded_full()
                            .bg(border.opacity(0.6))
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui_kit::gpui::relative(meter_pct))
                                    .rounded_full()
                                    .bg(success),
                            ),
                    ),
            );

        let speaker_picker =
            v_flex()
                .gap_2p5()
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1p5()
                        .child(
                            Self::device_choice_button(
                                "prefs-spk-default",
                                "System default",
                                active_out.is_none(),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings.huddle_output_device = None;
                                this.changed(cx);
                            })),
                        )
                        .children(self.audio_devices.outputs.iter().enumerate().map(
                            |(ix, dev)| {
                                let selected = active_out.as_deref() == Some(dev.name.as_str());
                                let name = dev.name.clone();
                                Self::device_choice_button(
                                    ("prefs-spk", ix),
                                    dev.name.clone(),
                                    selected,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.settings.huddle_output_device = Some(name.clone());
                                        this.changed(cx);
                                    },
                                ))
                            },
                        )),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("prefs-test-speaker")
                                .outline()
                                .with_size(Size::Small)
                                .icon(IconName::Play)
                                .label("Test speaker")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.emit(PreferencesEvent::TestSpeaker(
                                        this.settings.huddle_output_device.clone(),
                                    ));
                                })),
                        )
                        .child(
                            Button::new("prefs-refresh-audio")
                                .ghost()
                                .with_size(Size::Small)
                                .icon(IconName::RefreshCw)
                                .label("Refresh devices")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.refresh_audio_devices_and_probe(cx);
                                    cx.notify();
                                })),
                        ),
                );

        let dsp = v_flex()
            .gap_1p5()
            .child(self.checkbox(
                "prefs-huddle-aec",
                "Echo cancellation (WebRTC AEC3)",
                |s| &mut s.huddle_aec,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-ns",
                "Background noise suppression",
                |s| &mut s.huddle_ns,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-agc",
                "Automatic microphone gain control",
                |s| &mut s.huddle_agc,
                cx,
            ));

        let behavior = v_flex()
            .gap_1p5()
            .child(self.checkbox(
                "prefs-huddle-join-muted",
                "Mute my microphone when joining a huddle",
                |s| &mut s.huddle_join_muted,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-leave-lock",
                "Automatically leave a huddle when my computer locks",
                |s| &mut s.huddle_leave_on_lock,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-auto-popout",
                "Automatically open the Huddle window when joining",
                |s| &mut s.huddle_auto_open_window,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-sounds",
                "Play join, leave, and incoming huddle ring chimes",
                |s| &mut s.huddle_sounds,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-reactions",
                "Show floating emoji reaction animations in huddles",
                |s| &mut s.huddle_reaction_animations,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-captions",
                "Automatically enable live captions when joining",
                |s| &mut s.huddle_auto_captions,
                cx,
            ))
            .child(self.checkbox(
                "prefs-huddle-music",
                "Play ambient music while waiting alone in a huddle",
                |s| &mut s.huddle_music_when_alone,
                cx,
            ));

        let camera_group = self.audio_video_camera(cx);

        v_flex()
            .child(camera_group)
            .child(self.group(
                "Microphone",
                Some("Choose which microphone Slackless uses for native huddles."),
                mic_picker,
                cx,
            ))
            .child(self.group(
                "Speaker",
                Some("Choose where huddle audio plays."),
                speaker_picker,
                cx,
            ))
            .child(self.group(
                "Audio processing",
                Some("Pure-Rust real-time DSP applied to captured microphone audio."),
                dsp,
                cx,
            ))
            .child(self.group("When joining a huddle", None, behavior, cx))
    }

    fn audio_video_camera(&mut self, cx: &mut Context<Self>) -> Div {
        use gpui_kit::gpui::{ObjectFit, StyledImage as _, img};

        let theme = cx.theme();
        let (border, muted, card_bg) = (theme.border, theme.muted_foreground, theme.muted);
        let active_cam = self.settings.huddle_camera_device;
        let preview_on = self.camera_preview_on;

        let picker = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .child(
                        Self::device_choice_button(
                            "prefs-cam-default",
                            "System default",
                            active_cam.is_none(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.huddle_camera_device = None;
                            if this.camera_preview_on {
                                this.start_camera_preview(cx);
                            }
                            this.changed(cx);
                        })),
                    )
                    .children(self.cameras.iter().enumerate().map(|(ix, cam)| {
                        let selected = active_cam == Some(cam.index);
                        let cam_idx = cam.index;
                        Self::device_choice_button(("prefs-cam", ix), cam.name.clone(), selected)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.settings.huddle_camera_device = Some(cam_idx);
                                if this.camera_preview_on {
                                    this.start_camera_preview(cx);
                                }
                                this.changed(cx);
                            }))
                    })),
            )
            .child(self.checkbox(
                "prefs-huddle-blur",
                "Blur my background by default when turning on camera",
                |s| &mut s.huddle_blur_background,
                cx,
            ))
            .child(
                h_flex().gap_2().child(
                    Button::new("prefs-cam-preview-toggle")
                        .outline()
                        .with_size(Size::Small)
                        .icon(if preview_on {
                            IconName::VideoOff
                        } else {
                            IconName::Video
                        })
                        .label(if preview_on {
                            "Stop camera preview"
                        } else {
                            "Preview camera"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.camera_preview_on {
                                this.stop_camera_preview(cx);
                                cx.notify();
                            } else {
                                this.start_camera_preview(cx);
                            }
                        })),
                ),
            )
            .when(preview_on, |el| {
                let preview = div()
                    .w(px(280.))
                    .h(px(158.))
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(card_bg)
                    .border_1()
                    .border_color(border)
                    .flex()
                    .items_center()
                    .justify_center();
                el.child(if let Some(frame) = self.camera_frame.clone() {
                    preview.child(img(frame).w_full().h_full().object_fit(ObjectFit::Cover))
                } else {
                    preview.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Starting camera preview…"),
                    )
                })
            });

        self.group(
            "Camera",
            Some("Choose your webcam and optional portrait background blur for native huddles."),
            picker,
            cx,
        )
    }
}
