//! Bottom Huddle control bar (diagnostics, invite, mic/camera/share/hand/reactions/theme/thread/more, Leave).

use super::*;

impl PopoutWindow {
    pub(super) fn render_huddle_controls(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        status_color: Hsla,
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;
        use slackless_core::huddle::{list_audio_devices, list_cameras, list_screen_sources};

        let (muted_now, video_now, blur_now, sharing_now, hand_now) = (
            snap.muted,
            snap.video,
            snap.blur_background,
            snap.sharing,
            snap.hand_raised,
        );
        let thread_open = self.huddle_drawer == Some(HuddleDrawer::Thread);
        let diag_open = self.huddle_drawer == Some(HuddleDrawer::Diagnostics);
        let ctrl_bg = pal.ctrl_bg;
        let ctrl_hover = pal.ctrl_hover;
        let ctrl_fg = pal.chrome_fg;
        let active_bg = pal.ctrl_active_bg;
        let active_fg = pal.ctrl_active_fg;

        // Left utility cluster: signal/diagnostics + invite (matches official Slack).
        let left_cluster = h_flex()
            .items_center()
            .gap_1p5()
            .child(
                div()
                    .id("popout-huddle-diag")
                    .size(px(34.))
                    .rounded_lg()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if diag_open { ctrl_hover } else { ctrl_bg })
                    .hover(move |el| el.bg(ctrl_hover))
                    .child(
                        Icon::new(IconName::Activity)
                            .size(px(15.))
                            .text_color(status_color),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.huddle_drawer =
                            if this.huddle_drawer == Some(HuddleDrawer::Diagnostics) {
                                None
                            } else {
                                Some(HuddleDrawer::Diagnostics)
                            };
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("popout-huddle-header-invite")
                    .size(px(34.))
                    .rounded_lg()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if self.huddle_popover == Some(HuddlePopover::Invite) {
                        ctrl_hover
                    } else {
                        ctrl_bg
                    })
                    .hover(move |el| el.bg(ctrl_hover))
                    .child(
                        Icon::new(IconName::UserPlus)
                            .size(px(15.))
                            .text_color(ctrl_fg),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_huddle_popover(HuddlePopover::Invite);
                        cx.notify();
                    })),
            );

        // Center primary cluster (matches official Slack's exact order:
        // Mic + caret | Camera (+ Blur) | Screen Share | Hand | Reactions | Theme | Thread | More).
        let send_mute = self.send.clone();
        let next_muted = !muted_now;
        let mic_btn = h_flex()
            .h(px(36.))
            .rounded_lg()
            .overflow_hidden()
            .bg(if muted_now { pal.danger } else { active_bg })
            .child(
                div()
                    .id("popout-huddle-mic")
                    .h_full()
                    .px_2p5()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(if muted_now {
                            IconName::MicOff
                        } else {
                            IconName::Mic
                        })
                        .size(px(16.))
                        .text_color(if muted_now {
                            pal.danger_fg
                        } else {
                            active_fg
                        }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        (send_mute)(HuddleControlCommand::SetMuted(next_muted).into());
                        if this.fixtures.sample
                            && let Some(state) = this.huddle_state_entity()
                        {
                            state.update(cx, |s, cx| s.set_muted_local(next_muted, cx));
                        }
                    })),
            )
            .child(
                div()
                    .id("popout-huddle-devices-btn")
                    .h_full()
                    .px_1p5()
                    .cursor_pointer()
                    .border_l_1()
                    .border_color(if muted_now {
                        pal.danger_fg.opacity(0.2)
                    } else {
                        active_fg.opacity(0.18)
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::ChevronUp)
                            .size(px(12.))
                            .text_color(if muted_now { pal.danger_fg } else { active_fg }),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.toggle_huddle_popover(HuddlePopover::Devices) {
                            cx.spawn(async move |win, cx| {
                                let devices = cx
                                    .background_executor()
                                    .spawn(async move { list_audio_devices() })
                                    .await;
                                let _ = win.update(cx, |win: &mut PopoutWindow, cx| {
                                    win.huddle_devices = devices;
                                    cx.notify();
                                });
                            })
                            .detach();
                        }
                        cx.notify();
                    })),
            );

        let send_cam = self.send.clone();
        let next_cam = !video_now;
        let selected_cam_idx = self.huddle_selected_camera;
        let cam_open = self.huddle_popover == Some(HuddlePopover::Camera);
        let cam_btn = h_flex()
            .h(px(36.))
            .rounded_lg()
            .overflow_hidden()
            .bg(if video_now { active_bg } else { ctrl_bg })
            .child(
                div()
                    .id("popout-huddle-video")
                    .h_full()
                    .px_2p5()
                    .rounded_l_lg()
                    .cursor_pointer()
                    .hover(move |el| if video_now { el } else { el.bg(ctrl_hover) })
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(if video_now {
                            IconName::Video
                        } else {
                            IconName::VideoOff
                        })
                        .size(px(16.))
                        .text_color(if video_now {
                            active_fg
                        } else {
                            ctrl_fg
                        }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        (send_cam)(
                            HuddleControlCommand::SetCamera {
                                enabled: next_cam,
                                device_index: selected_cam_idx,
                                blur: blur_now,
                            }
                            .into(),
                        );
                        if this.fixtures.sample
                            && let Some(state) = this.huddle_state_entity()
                        {
                            state.update(cx, |s, cx| s.set_camera_local(next_cam, cx));
                        }
                    })),
            )
            .child(
                div()
                    .id("popout-huddle-camera-btn")
                    .h_full()
                    .px_1p5()
                    .rounded_r_lg()
                    .cursor_pointer()
                    .when(cam_open && !video_now, |el| el.bg(active_bg))
                    .hover(move |el| {
                        if video_now || cam_open {
                            el
                        } else {
                            el.bg(ctrl_hover)
                        }
                    })
                    .border_l_1()
                    .border_color(if video_now || cam_open {
                        active_fg.opacity(0.18)
                    } else {
                        pal.chrome_border
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(if cam_open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronUp
                        })
                        .size(px(12.))
                        .text_color(if video_now || cam_open {
                            active_fg
                        } else {
                            ctrl_fg
                        }),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.toggle_huddle_popover(HuddlePopover::Camera) {
                            cx.spawn(async move |win, cx| {
                                let cameras = cx
                                    .background_executor()
                                    .spawn(async move { list_cameras() })
                                    .await;
                                let _ = win.update(cx, |win: &mut PopoutWindow, cx| {
                                    win.huddle_cameras = cameras;
                                    cx.notify();
                                });
                            })
                            .detach();
                        }
                        cx.notify();
                    })),
            );

        let primary = pal.primary;
        let primary_hover = pal.primary_hover;
        let primary_fg = pal.primary_fg;
        let active_share_id = snap.share_source_id.clone();
        let share_btn = div()
            .id("popout-huddle-share")
            .size(px(36.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if sharing_now { primary } else { ctrl_bg })
            .hover(move |el| {
                el.bg(if sharing_now {
                    primary_hover
                } else {
                    ctrl_hover
                })
            })
            .child(
                Icon::new(IconName::ScreenShare)
                    .size(px(16.))
                    .text_color(if sharing_now { primary_fg } else { ctrl_fg }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let was_open = this.huddle_popover == Some(HuddlePopover::Share);
                this.close_share_modal(cx);
                this.close_huddle_popovers();
                if !was_open {
                    this.huddle_popover = Some(HuddlePopover::Share);
                    this.huddle_share_selected = active_share_id.clone();
                    this.huddle_share_tab = if active_share_id
                        .as_deref()
                        .is_some_and(|id| id.starts_with("window:"))
                    {
                        HuddleShareTab::Window
                    } else {
                        HuddleShareTab::Screen
                    };
                    if !slackless_core::huddle::has_screen_capture_permission() {
                        slackless_core::huddle::request_screen_capture_permission();
                    }
                    let monitors_first = this.huddle_share_tab == HuddleShareTab::Screen;
                    this.refresh_share_thumbnails(monitors_first, cx);
                    cx.spawn(async move |win, cx| {
                        let screens = cx
                            .background_executor()
                            .spawn(async move { list_screen_sources() })
                            .await;
                        let _ = win.update(cx, |win: &mut PopoutWindow, cx| {
                            win.huddle_screens = screens;
                            cx.notify();
                        });
                    })
                    .detach();
                }
                cx.notify();
            }));

        let send_hand = self.send.clone();
        let next_hand = !hand_now;
        let warn = pal.warning;
        let warn_fg = pal.warning_fg;
        let hand_btn = div()
            .id("popout-huddle-hand")
            .size(px(36.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if hand_now { warn } else { ctrl_bg })
            .hover(move |el| el.bg(if hand_now { warn } else { ctrl_hover }))
            .child(
                Icon::new(IconName::Hand)
                    .size(px(16.))
                    .text_color(if hand_now { warn_fg } else { ctrl_fg }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                (send_hand)(HuddleControlCommand::SetHandRaised(next_hand).into());
                if this.fixtures.sample
                    && let Some(dock) = this.huddle_state_entity()
                {
                    let me = this.fixtures.self_user.clone();
                    dock.update(cx, |d, cx| {
                        d.apply_data_event(
                            &slackless_core::huddle::HuddleDataEvent::HandRaise {
                                user: me,
                                raised: next_hand,
                            },
                            cx,
                        );
                    });
                }
            }));

        let react_btn = div()
            .id("popout-huddle-react-btn")
            .size(px(36.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if self.huddle_popover == Some(HuddlePopover::Reactions) {
                ctrl_hover
            } else {
                ctrl_bg
            })
            .hover(move |el| el.bg(ctrl_hover))
            .child(
                Icon::new(IconName::FaceSlightlySmiling)
                    .size(px(16.))
                    .text_color(ctrl_fg),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.toggle_huddle_popover(HuddlePopover::Reactions);
                cx.notify();
            }));

        let theme_btn = div()
            .id("popout-huddle-theme-btn")
            .size(px(36.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if self.huddle_popover == Some(HuddlePopover::Theme) {
                ctrl_hover
            } else {
                ctrl_bg
            })
            .hover(move |el| el.bg(ctrl_hover))
            .child(
                Icon::new(IconName::Sparkles)
                    .size(px(16.))
                    .text_color(ctrl_fg),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.toggle_huddle_popover(HuddlePopover::Theme);
                cx.notify();
            }));

        let thread_btn = div()
            .id("popout-huddle-thread")
            .size(px(36.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if thread_open { active_bg } else { ctrl_bg })
            .hover(move |el| el.bg(if thread_open { active_bg } else { ctrl_hover }))
            .child(
                Icon::new(IconName::MessageSquare)
                    .size(px(16.))
                    .text_color(if thread_open { active_fg } else { ctrl_fg }),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.huddle_drawer = if this.huddle_drawer == Some(HuddleDrawer::Thread) {
                    None
                } else {
                    Some(HuddleDrawer::Thread)
                };
                this.refresh_huddle_thread_async(cx);
                this.maybe_fetch_huddle_thread();
                cx.notify();
            }));

        let more_btn = div()
            .id("popout-huddle-more-btn")
            .size(px(36.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if self.huddle_popover == Some(HuddlePopover::More) {
                ctrl_hover
            } else {
                ctrl_bg
            })
            .hover(move |el| el.bg(ctrl_hover))
            .child(
                Icon::new(IconName::EllipsisVertical)
                    .size(px(16.))
                    .text_color(ctrl_fg),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.toggle_huddle_popover(HuddlePopover::More);
                cx.notify();
            }));

        // Far-right crimson "Leave" button (matches official Slack).
        let send_leave = self.send.clone();
        let danger_hover = pal.danger_hover;
        let leave_btn = h_flex()
            .id("popout-huddle-leave")
            .h(px(36.))
            .px_4()
            .items_center()
            .justify_center()
            .rounded_lg()
            .bg(pal.danger)
            .hover(move |el| el.bg(danger_hover))
            .cursor_pointer()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(pal.danger_fg)
            .child("Leave")
            .on_click(move |_, window, _| {
                (send_leave)(HuddleCommand::Leave.into());
                window.remove_window();
            });

        h_flex()
            .flex_none()
            .h(px(56.))
            .px_3p5()
            .items_center()
            .justify_between()
            .bg(pal.chrome_bg)
            .border_t_1()
            .border_color(pal.chrome_border)
            .child(left_cluster)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(mic_btn)
                    .child(cam_btn)
                    .child(share_btn)
                    .child(hand_btn)
                    .child(react_btn)
                    .child(theme_btn)
                    .child(thread_btn)
                    .child(more_btn),
            )
            .child(leave_btn)
    }
}
