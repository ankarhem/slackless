//! Anchored popovers and modals for Huddles (More, Camera, Screen Share, Reactions, Themes, Invite, Devices).

use super::*;

impl PopoutWindow {
    pub(super) fn render_huddle_active_popover(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        channel: &ChannelId,
        people: &[slackless_core::types::UserId],
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::gpui::Div> {
        if self.huddle_camera_open {
            return Some(self.render_huddle_camera_panel(
                snap.video,
                snap.blur_background,
                pal,
                cx,
            ));
        }
        if self.huddle_reactions_open {
            return Some(self.render_huddle_reactions_panel(snap, pal, cx));
        }
        if self.huddle_theme_open {
            return Some(self.render_huddle_theme_panel(snap.background_id.as_deref(), pal, cx));
        }
        if self.huddle_invite_open {
            return Some(self.render_huddle_invite_panel(channel, people, pal, cx));
        }
        if self.huddle_devices_open {
            return Some(self.render_huddle_devices_panel(
                snap.input_device.clone(),
                snap.output_device.clone(),
                pal,
                cx,
            ));
        }
        if self.huddle_more_open {
            return Some(self.render_huddle_more_panel(snap, pal, cx));
        }
        None
    }

    fn render_huddle_more_panel(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;
        use slackless_core::huddle::list_audio_devices;

        let hover = pal.popover_hover;
        let captions_now = snap.captions_enabled;
        let music_on = snap.jukebox_song.is_some();
        let notes_open = self.huddle_drawer == Some(HuddleDrawer::Notes);
        let channel = snap.channel.clone();
        let browser_url = snap.huddle_link.clone().unwrap_or_else(|| {
            let team = self
                .fixtures
                .workspaces
                .first()
                .map(|t| t.id.to_string())
                .unwrap_or_default();
            format!("https://app.slack.com/huddle/{team}/{channel}")
        });
        let send_cc = self.send.clone();
        let send_music = self.send.clone();
        let send_copy = self.send.clone();
        let copy_ch = channel.clone();

        let item = |id: &'static str, icon: IconName, label: String, active: bool| {
            h_flex()
                .id(id)
                .px_2p5()
                .py_1p5()
                .gap_2()
                .items_center()
                .rounded_lg()
                .cursor_pointer()
                .hover(move |el| el.bg(hover))
                .text_xs()
                .font_weight(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(if active { pal.link } else { pal.popover_fg })
                .child(Icon::new(icon).size(px(14.)).text_color(if active {
                    pal.link
                } else {
                    pal.popover_muted
                }))
                .child(label)
        };

        v_flex()
            .w(px(248.))
            .p_2()
            .gap_0p5()
            .rounded_xl()
            .border_1()
            .border_color(pal.popover_border)
            .bg(pal.popover_bg)
            .shadow_2xl()
            .text_color(pal.popover_fg)
            .child(
                item(
                    "popout-huddle-canvas",
                    IconName::FileText,
                    if notes_open {
                        "Hide huddle notes".to_owned()
                    } else {
                        "Open huddle notes".to_owned()
                    },
                    notes_open,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.huddle_more_open = false;
                    this.huddle_drawer = if this.huddle_drawer == Some(HuddleDrawer::Notes) {
                        None
                    } else {
                        Some(HuddleDrawer::Notes)
                    };
                    cx.notify();
                })),
            )
            .child(
                item(
                    "popout-huddle-cc",
                    IconName::Captions,
                    if captions_now {
                        "Turn off live captions".to_owned()
                    } else {
                        "Turn on live captions".to_owned()
                    },
                    captions_now,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let next = !captions_now;
                    (send_cc)(Command::SetHuddleCaptions(next));
                    if this.fixtures.sample
                        && let Some(dock) = this.huddle_dock_entity()
                    {
                        dock.update(cx, |d, cx| d.set_captions_local(next, cx));
                    }
                    this.huddle_more_open = false;
                    cx.notify();
                })),
            )
            .child(
                item(
                    "popout-huddle-music",
                    IconName::Music,
                    if music_on {
                        "Stop background music".to_owned()
                    } else {
                        "Play Smooth Jazz".to_owned()
                    },
                    music_on,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let next_song = (!music_on).then(|| "jazz".to_owned());
                    (send_music)(Command::SetHuddleJukebox(next_song.clone()));
                    if this.fixtures.sample
                        && let Some(dock) = this.huddle_dock_entity()
                    {
                        dock.update(cx, |d, cx| d.set_jukebox_local(next_song.clone(), cx));
                    }
                    this.huddle_more_open = false;
                    cx.notify();
                })),
            )
            .child(
                item(
                    "popout-huddle-devices-menu",
                    IconName::SlidersHorizontal,
                    "Audio devices…".to_owned(),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.close_huddle_popovers();
                    this.huddle_devices_open = true;
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
                    cx.notify();
                })),
            )
            .child(div().h(px(1.)).my_1().bg(pal.popover_border))
            .child(
                item(
                    "popout-huddle-copy-link",
                    IconName::Link,
                    "Copy huddle link".to_owned(),
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    (send_copy)(Command::CopyHuddleLink {
                        channel: copy_ch.clone(),
                    });
                    this.huddle_more_open = false;
                    cx.notify();
                })),
            )
            .child(
                item(
                    "popout-huddle-browser",
                    IconName::ExternalLink,
                    "Open in browser".to_owned(),
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.open_url(&browser_url);
                    this.huddle_more_open = false;
                    cx.notify();
                })),
            )
    }

    fn render_huddle_camera_panel(
        &self,
        video_now: bool,
        blur_now: bool,
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;

        let hover = pal.popover_hover;
        let link = pal.link;
        let border = pal.popover_border;
        let selected_idx = self
            .huddle_selected_camera
            .or_else(|| self.huddle_cameras.first().map(|c| c.index));
        let send_cam = self.send.clone();
        let send_blur = self.send.clone();

        v_flex()
            .w(px(248.))
            .p_2()
            .gap_0p5()
            .rounded_xl()
            .border_1()
            .border_color(border)
            .bg(pal.popover_bg)
            .shadow_2xl()
            .text_color(pal.popover_fg)
            .child(
                div()
                    .px_2p5()
                    .py_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(pal.popover_muted)
                    .child("Video"),
            )
            .child(if self.huddle_cameras.is_empty() {
                div()
                    .px_2p5()
                    .py_1p5()
                    .text_xs()
                    .text_color(pal.popover_muted)
                    .child("No cameras detected")
                    .into_any_element()
            } else {
                v_flex()
                    .gap_0p5()
                    .children(self.huddle_cameras.iter().enumerate().map(|(ix, cam)| {
                        let is_selected = selected_idx == Some(cam.index);
                        let cam_idx = cam.index;
                        let send = send_cam.clone();
                        h_flex()
                            .id(("popout-huddle-cam-opt", ix))
                            .px_2p5()
                            .py_1p5()
                            .gap_2()
                            .items_center()
                            .rounded_lg()
                            .cursor_pointer()
                            .hover(move |el| el.bg(hover))
                            .text_xs()
                            .font_weight(if is_selected {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .text_color(if is_selected { link } else { pal.popover_fg })
                            .child(if is_selected {
                                Icon::new(IconName::Check)
                                    .size(px(14.))
                                    .text_color(link)
                                    .into_any_element()
                            } else {
                                div().size(px(14.)).into_any_element()
                            })
                            .child(div().flex_1().min_w_0().truncate().child(cam.name.clone()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.huddle_selected_camera = Some(cam_idx);
                                if video_now {
                                    (send)(Command::SetHuddleCamera {
                                        enabled: true,
                                        device_index: Some(cam_idx),
                                        blur: blur_now,
                                    });
                                }
                                this.huddle_camera_open = false;
                                cx.notify();
                            }))
                    }))
                    .into_any_element()
            })
            .child(div().my_1().h(px(1.)).bg(border))
            .child(
                h_flex()
                    .id("popout-huddle-cam-blur")
                    .px_2p5()
                    .py_1p5()
                    .gap_2()
                    .items_center()
                    .rounded_lg()
                    .cursor_pointer()
                    .hover(move |el| el.bg(hover))
                    .text_xs()
                    .font_weight(if blur_now {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if blur_now { link } else { pal.popover_fg })
                    .child(if blur_now {
                        Icon::new(IconName::Check)
                            .size(px(14.))
                            .text_color(link)
                            .into_any_element()
                    } else {
                        Icon::new(IconName::Video)
                            .size(px(14.))
                            .text_color(pal.popover_muted)
                            .into_any_element()
                    })
                    .child("Blur background")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let next_blur = !blur_now;
                        (send_blur)(Command::SetHuddleBlurBackground(next_blur));
                        if this.fixtures.sample
                            && let Some(dock) = this.huddle_dock_entity()
                        {
                            dock.update(cx, |d, cx| d.set_blur_local(next_blur, cx));
                        }
                        this.huddle_camera_open = false;
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .id("popout-huddle-cam-bg")
                    .px_2p5()
                    .py_1p5()
                    .gap_2()
                    .items_center()
                    .rounded_lg()
                    .cursor_pointer()
                    .hover(move |el| el.bg(hover))
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(pal.popover_fg)
                    .child(
                        Icon::new(IconName::Sparkles)
                            .size(px(14.))
                            .text_color(pal.popover_muted),
                    )
                    .child("Video background")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.close_huddle_popovers();
                        this.huddle_theme_open = true;
                        cx.notify();
                    })),
            )
            .child(div().my_1().h(px(1.)).bg(border))
            .child(
                h_flex()
                    .id("popout-huddle-cam-prefs")
                    .px_2p5()
                    .py_1p5()
                    .gap_2()
                    .items_center()
                    .rounded_lg()
                    .cursor_pointer()
                    .hover(move |el| el.bg(hover))
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(pal.popover_fg)
                    .child(
                        Icon::new(IconName::Settings)
                            .size(px(14.))
                            .text_color(pal.popover_muted),
                    )
                    .child("Audio & video preferences")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_audio_video_preferences(cx);
                    })),
            )
    }

    pub(super) fn render_huddle_share_modal(
        &self,
        active_source: Option<&str>,
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        use gpui_kit::component::Icon;
        use gpui_kit::gpui::{Corners, ObjectFit};

        let send = self.send.clone();
        let loopback = slackless_core::huddle::system_audio_source_label();
        let has_screen_perm = slackless_core::huddle::has_screen_capture_permission();
        let hover = pal.popover_hover;
        let primary = pal.primary;
        let primary_fg = pal.primary_fg;
        let surface = pal.drawer_surface;
        let border = pal.popover_border;
        let active_tab = self.huddle_share_tab;
        let selected_id = self.huddle_share_selected.as_deref();
        let share_audio = self.huddle_share_audio;

        let monitors: Vec<_> = self
            .huddle_screens
            .iter()
            .filter(|s| s.is_monitor)
            .cloned()
            .collect();
        let windows: Vec<_> = self
            .huddle_screens
            .iter()
            .filter(|s| !s.is_monitor)
            .cloned()
            .collect();

        let render_thumb = |thumb: Option<Arc<gpui_kit::gpui::RenderImage>>,
                            is_monitor: bool,
                            res_label: String,
                            w: Pixels,
                            h: Pixels| {
            div()
                .w(w)
                .h(h)
                .rounded_lg()
                .overflow_hidden()
                .border_1()
                .border_color(border)
                .bg(rgb(0x14171C))
                .flex()
                .items_center()
                .justify_center()
                .child(if let Some(frame) = thumb {
                    let frame_layout = Arc::clone(&frame);
                    canvas(
                        move |bounds, _, _| {
                            ObjectFit::Contain.get_bounds(bounds, frame_layout.size(0))
                        },
                        move |bounds, img_bounds, window, _| {
                            if frame.frame_count() > 0 {
                                let _ = window.paint_image(
                                    bounds,
                                    img_bounds,
                                    Corners::all(px(7.)),
                                    Arc::clone(&frame),
                                    0,
                                    false,
                                );
                            }
                        },
                    )
                    .size_full()
                    .into_any_element()
                } else {
                    v_flex()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .child(
                            Icon::new(if is_monitor {
                                IconName::ScreenShare
                            } else {
                                IconName::AppWindow
                            })
                            .size(px(22.))
                            .text_color(pal.scrim_muted),
                        )
                        .child(div().text_xs().text_color(pal.scrim_muted).child(res_label))
                        .into_any_element()
                })
        };

        let header = h_flex()
            .relative()
            .w_full()
            .px_5()
            .pt_4()
            .pb_3()
            .items_center()
            .justify_center()
            .child(
                h_flex()
                    .p(px(3.))
                    .rounded_xl()
                    .bg(surface)
                    .border_1()
                    .border_color(border)
                    .gap_1()
                    .child(
                        div()
                            .id("popout-share-tab-screen")
                            .px_4()
                            .py_1p5()
                            .rounded_lg()
                            .cursor_pointer()
                            .text_xs()
                            .font_weight(if active_tab == HuddleShareTab::Screen {
                                FontWeight::BOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .bg(if active_tab == HuddleShareTab::Screen {
                                pal.popover_bg
                            } else {
                                Hsla::from(rgba(0x00000000))
                            })
                            .border_1()
                            .border_color(if active_tab == HuddleShareTab::Screen {
                                border
                            } else {
                                Hsla::from(rgba(0x00000000))
                            })
                            .text_color(if active_tab == HuddleShareTab::Screen {
                                pal.popover_fg
                            } else {
                                pal.popover_muted
                            })
                            .child("Share entire screen")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.huddle_share_tab != HuddleShareTab::Screen {
                                    this.huddle_share_tab = HuddleShareTab::Screen;
                                    this.huddle_share_selected = None;
                                    this.refresh_share_thumbnails(true, cx);
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        div()
                            .id("popout-share-tab-window")
                            .px_5()
                            .py_1p5()
                            .rounded_lg()
                            .cursor_pointer()
                            .text_xs()
                            .font_weight(if active_tab == HuddleShareTab::Window {
                                FontWeight::BOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .bg(if active_tab == HuddleShareTab::Window {
                                pal.popover_bg
                            } else {
                                Hsla::from(rgba(0x00000000))
                            })
                            .border_1()
                            .border_color(if active_tab == HuddleShareTab::Window {
                                border
                            } else {
                                Hsla::from(rgba(0x00000000))
                            })
                            .text_color(if active_tab == HuddleShareTab::Window {
                                pal.popover_fg
                            } else {
                                pal.popover_muted
                            })
                            .child("Window")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.huddle_share_tab != HuddleShareTab::Window {
                                    this.huddle_share_tab = HuddleShareTab::Window;
                                    this.huddle_share_selected = None;
                                    this.refresh_share_thumbnails(false, cx);
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .id("popout-share-modal-close")
                    .absolute()
                    .right_4()
                    .top_4()
                    .size(px(28.))
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(move |el| el.bg(hover))
                    .child(
                        Icon::new(IconName::X)
                            .size(px(15.))
                            .text_color(pal.popover_muted),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.close_share_modal(cx);
                        cx.notify();
                    })),
            );

        let body: gpui_kit::gpui::AnyElement = match active_tab {
            HuddleShareTab::Screen => {
                if monitors.is_empty() {
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(pal.popover_muted)
                        .child("Scanning screens…")
                        .into_any_element()
                } else {
                    h_flex()
                        .flex_1()
                        .w_full()
                        .px_6()
                        .py_4()
                        .flex_wrap()
                        .items_center()
                        .justify_center()
                        .gap_6()
                        .children(monitors.iter().enumerate().map(|(ix, src)| {
                            let is_selected = selected_id == Some(src.id.as_str());
                            let src_id = src.id.clone();
                            let label = if monitors.len() > 1 {
                                format!("Screen {} — {}", ix + 1, src.name)
                            } else {
                                format!("Screen 1 — {}", src.name)
                            };
                            let thumb = self.huddle_screen_thumbs.get(&src.id).cloned();
                            let res = format!("{}×{}", src.width, src.height);
                            v_flex()
                                .id(("popout-share-screen-card", ix))
                                .w(px(268.))
                                .p_2p5()
                                .gap_2()
                                .items_center()
                                .rounded_xl()
                                .cursor_pointer()
                                .border_2()
                                .border_color(if is_selected {
                                    primary
                                } else {
                                    Hsla::from(rgba(0x00000000))
                                })
                                .bg(if is_selected {
                                    primary.opacity(0.08)
                                } else {
                                    Hsla::from(rgba(0x00000000))
                                })
                                .hover(move |el| if is_selected { el } else { el.bg(hover) })
                                .child(render_thumb(thumb, true, res, px(244.), px(138.)))
                                .child(
                                    div()
                                        .w_full()
                                        .text_center()
                                        .truncate()
                                        .text_xs()
                                        .font_weight(if is_selected {
                                            FontWeight::SEMIBOLD
                                        } else {
                                            FontWeight::MEDIUM
                                        })
                                        .text_color(if is_selected {
                                            primary
                                        } else {
                                            pal.popover_fg
                                        })
                                        .child(label),
                                )
                                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                    if ev.click_count() >= 2 {
                                        this.start_selected_screen_share(src_id.clone(), cx);
                                    } else {
                                        this.huddle_share_selected = Some(src_id.clone());
                                        cx.notify();
                                    }
                                }))
                        }))
                        .into_any_element()
                }
            }
            HuddleShareTab::Window => {
                if windows.is_empty() {
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(pal.popover_muted)
                        .child("No shareable windows found (check Screen Recording permission)")
                        .into_any_element()
                } else {
                    div()
                        .id("popout-share-windows-scroll")
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .overflow_y_scroll()
                        .px_5()
                        .py_3()
                        .child(h_flex().flex_wrap().justify_start().gap_3().children(
                            windows.iter().enumerate().map(|(ix, src)| {
                                let is_selected = selected_id == Some(src.id.as_str());
                                let src_id = src.id.clone();
                                let thumb = self.huddle_screen_thumbs.get(&src.id).cloned();
                                let res = format!("{}×{}", src.width, src.height);
                                v_flex()
                                    .id(("popout-share-window-card", ix))
                                    .w(px(200.))
                                    .p_2()
                                    .gap_1p5()
                                    .items_center()
                                    .rounded_xl()
                                    .cursor_pointer()
                                    .border_2()
                                    .border_color(if is_selected {
                                        primary
                                    } else {
                                        Hsla::from(rgba(0x00000000))
                                    })
                                    .bg(if is_selected {
                                        primary.opacity(0.08)
                                    } else {
                                        Hsla::from(rgba(0x00000000))
                                    })
                                    .hover(move |el| if is_selected { el } else { el.bg(hover) })
                                    .child(render_thumb(thumb, false, res, px(180.), px(102.)))
                                    .child(
                                        div()
                                            .w(px(180.))
                                            .text_center()
                                            .truncate()
                                            .text_xs()
                                            .font_weight(if is_selected {
                                                FontWeight::SEMIBOLD
                                            } else {
                                                FontWeight::MEDIUM
                                            })
                                            .text_color(if is_selected {
                                                primary
                                            } else {
                                                pal.popover_fg
                                            })
                                            .child(src.name.clone()),
                                    )
                                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                        if ev.click_count() >= 2 {
                                            this.start_selected_screen_share(src_id.clone(), cx);
                                        } else {
                                            this.huddle_share_selected = Some(src_id.clone());
                                            cx.notify();
                                        }
                                    }))
                            }),
                        ))
                        .into_any_element()
                }
            }
        };

        let can_share = self.huddle_share_selected.is_some();
        let selected_to_share = self.huddle_share_selected.clone();
        let success = pal.success;

        let footer = h_flex()
            .flex_none()
            .w_full()
            .px_5()
            .py_3p5()
            .border_t_1()
            .border_color(border)
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                h_flex()
                    .id("popout-share-audio-toggle")
                    .gap_2()
                    .items_center()
                    .cursor_pointer()
                    .child(
                        div()
                            .size(px(16.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(if share_audio { primary } else { border })
                            .bg(if share_audio { primary } else { pal.popover_bg })
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(primary_fg)
                            .child(if share_audio { "✓" } else { "" }),
                    )
                    .child(div().text_xs().text_color(pal.popover_fg).child(
                        if loopback.is_some() {
                            "Share system audio"
                        } else {
                            "Share system audio (requires BlackHole 2ch)"
                        },
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.huddle_share_audio = !this.huddle_share_audio;
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .when(active_source.is_some(), |row| {
                        let send_stop = send.clone();
                        row.child(
                            div()
                                .id("popout-huddle-share-stop")
                                .px_3()
                                .py_1p5()
                                .rounded_lg()
                                .bg(pal.danger)
                                .cursor_pointer()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(pal.danger_fg)
                                .child("Stop sharing")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    (send_stop)(Command::SetHuddleScreenShare {
                                        source_id: None,
                                        share_audio: false,
                                    });
                                    if this.fixtures.sample
                                        && let Some(dock) = this.huddle_dock_entity()
                                    {
                                        dock.update(cx, |d, cx| d.set_sharing_local(None, cx));
                                    }
                                    this.close_share_modal(cx);
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        div()
                            .id("popout-share-modal-cancel")
                            .px_4()
                            .py_1p5()
                            .rounded_lg()
                            .border_1()
                            .border_color(border)
                            .bg(pal.popover_bg)
                            .hover(move |el| el.bg(hover))
                            .cursor_pointer()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(pal.popover_fg)
                            .child("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_share_modal(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("popout-share-modal-confirm")
                            .px_4()
                            .py_1p5()
                            .rounded_lg()
                            .bg(if can_share { success } else { surface })
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if can_share {
                                rgb(0xFFFFFF).into()
                            } else {
                                pal.popover_muted
                            })
                            .when(can_share, |btn| btn.cursor_pointer())
                            .child("Share")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(src_id) = selected_to_share.clone() {
                                    this.start_selected_screen_share(src_id, cx);
                                }
                            })),
                    ),
            );

        div()
            .id("popout-huddle-share-backdrop")
            .absolute()
            .inset_0()
            .bg(rgba(0x00000078))
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_share_modal(cx);
                cx.notify();
            }))
            .child(
                v_flex()
                    .id("popout-huddle-share-card")
                    .w(px(680.))
                    .max_w_full()
                    .h(px(470.))
                    .max_h_full()
                    .rounded_2xl()
                    .border_1()
                    .border_color(border)
                    .bg(pal.popover_bg)
                    .shadow_2xl()
                    .text_color(pal.popover_fg)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(header)
                    .when(!has_screen_perm, |col| {
                        col.child(
                            h_flex()
                                .mx_5()
                                .mb_2()
                                .px_3()
                                .py_2()
                                .gap_3()
                                .items_center()
                                .justify_between()
                                .rounded_lg()
                                .bg(pal.warning.opacity(0.14))
                                .border_1()
                                .border_color(pal.warning.opacity(0.55))
                                .child(
                                    div()
                                        .flex_1()
                                        .text_xs()
                                        .text_color(pal.popover_fg)
                                        .child("Grant Screen Recording permission in macOS System Settings for full window previews & hardware capture."),
                                )
                                .child(
                                    div()
                                        .id("popout-huddle-open-screen-perm")
                                        .flex_none()
                                        .px_2p5()
                                        .py_1()
                                        .rounded_md()
                                        .bg(primary)
                                        .cursor_pointer()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(primary_fg)
                                        .child("Open Settings")
                                        .on_click(|_, _, _| {
                                            slackless_core::huddle::request_screen_capture_permission();
                                            slackless_core::huddle::open_screen_capture_settings();
                                        }),
                                ),
                        )
                    })
                    .child(body)
                    .child(footer),
            )
    }

    fn render_huddle_reactions_panel(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        use slackless_core::huddle::{HuddleDataEvent, HuddleReactionKind};

        const QUICK: [(&str, &str); 8] = [
            ("👍", "+1"),
            ("🎉", "tada"),
            ("❤️", "heart"),
            ("😂", "joy"),
            ("👏", "clap"),
            ("🔥", "fire"),
            ("👀", "eyes"),
            ("🙌", "raised_hands"),
        ];
        const EFFECTS: [(&str, &str, &str); 6] = [
            ("🎉", "Confetti", "confetti"),
            ("💖", "Hearts", "hearts"),
            ("👏", "Applause", "clap"),
            ("✨", "Sparkles", "sparkles"),
            ("🚀", "Rocket", "rocket"),
            ("🏆", "Trophy", "trophy"),
        ];
        const STICKY: [(&str, &str, &str); 4] = [
            ("✋", "Hand", "raised_hand"),
            ("☕", "Coffee", "coffee"),
            ("🎧", "BRB", "headphones"),
            ("✅", "Ready", "white_check_mark"),
        ];

        let send = self.send.clone();
        let active_sticky = snap.sticky_emoji.clone();
        let hover = pal.popover_hover;
        let surface = pal.drawer_surface;
        let primary = pal.primary;
        let primary_fg = pal.primary_fg;

        v_flex()
            .p_3()
            .gap_2p5()
            .rounded_2xl()
            .border_1()
            .border_color(pal.popover_border)
            .bg(pal.popover_bg)
            .shadow_2xl()
            .text_color(pal.popover_fg)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .w(px(52.))
                            .text_xs()
                            .text_color(pal.popover_muted)
                            .child("React"),
                    )
                    .children(QUICK.iter().enumerate().map(|(ix, &(glyph, code))| {
                        let em = code.to_owned();
                        let send_em = send.clone();
                        div()
                            .id(("popout-huddle-react", ix))
                            .size(px(32.))
                            .rounded_lg()
                            .cursor_pointer()
                            .hover(move |el| el.bg(hover))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_base()
                            .child(glyph)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                (send_em)(Command::SendHuddleReaction {
                                    emoji: em.clone(),
                                    kind: HuddleReactionKind::Standard,
                                    emoji_url: None,
                                });
                                if this.fixtures.sample
                                    && let Some(dock) = this.huddle_dock_entity()
                                {
                                    let me = this.fixtures.self_user.clone();
                                    dock.update(cx, |d, cx| {
                                        d.apply_data_event(
                                            &HuddleDataEvent::Reaction {
                                                user: me,
                                                emoji: em.clone(),
                                                kind: HuddleReactionKind::Standard,
                                                emoji_url: None,
                                                at_ms: 0,
                                            },
                                            cx,
                                        );
                                    });
                                }
                                cx.notify();
                            }))
                    })),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .items_center()
                    .child(
                        div()
                            .w(px(52.))
                            .text_xs()
                            .text_color(pal.popover_muted)
                            .child("Effects"),
                    )
                    .children(
                        EFFECTS
                            .iter()
                            .enumerate()
                            .map(|(ix, &(glyph, label, code))| {
                                let em = code.to_owned();
                                let send_ef = send.clone();
                                div()
                                    .id(("popout-huddle-effect", ix))
                                    .px_2p5()
                                    .py_1()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .bg(surface)
                                    .hover(move |el| el.bg(hover))
                                    .text_xs()
                                    .text_color(pal.popover_fg)
                                    .child(format!("{glyph} {label}"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        (send_ef)(Command::SendHuddleReaction {
                                            emoji: em.clone(),
                                            kind: HuddleReactionKind::Effect,
                                            emoji_url: None,
                                        });
                                        if this.fixtures.sample
                                            && let Some(dock) = this.huddle_dock_entity()
                                        {
                                            let me = this.fixtures.self_user.clone();
                                            dock.update(cx, |d, cx| {
                                                d.apply_data_event(
                                                    &HuddleDataEvent::Reaction {
                                                        user: me,
                                                        emoji: em.clone(),
                                                        kind: HuddleReactionKind::Effect,
                                                        emoji_url: None,
                                                        at_ms: 0,
                                                    },
                                                    cx,
                                                );
                                            });
                                        }
                                        this.huddle_reactions_open = false;
                                        cx.notify();
                                    }))
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .items_center()
                    .child(
                        div()
                            .w(px(52.))
                            .text_xs()
                            .text_color(pal.popover_muted)
                            .child("Sticky"),
                    )
                    .children(
                        STICKY
                            .iter()
                            .enumerate()
                            .map(|(ix, &(glyph, label, code))| {
                                let selected = active_sticky.as_deref() == Some(code);
                                let next = (!selected).then(|| code.to_owned());
                                let send_st = send.clone();
                                div()
                                    .id(("popout-huddle-sticky", ix))
                                    .px_2p5()
                                    .py_1()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .bg(if selected { primary } else { surface })
                                    .hover(move |el| el.bg(if selected { primary } else { hover }))
                                    .text_xs()
                                    .text_color(if selected { primary_fg } else { pal.popover_fg })
                                    .child(format!("{glyph} {label}"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        (send_st)(Command::SetHuddleStickyEmoji(next.clone()));
                                        if this.fixtures.sample
                                            && let Some(dock) = this.huddle_dock_entity()
                                        {
                                            let me = this.fixtures.self_user.clone();
                                            dock.update(cx, |d, cx| {
                                                d.apply_data_event(
                                                    &HuddleDataEvent::StickyEmoji {
                                                        user: me,
                                                        emoji: next.clone(),
                                                    },
                                                    cx,
                                                );
                                            });
                                        }
                                        cx.notify();
                                    }))
                            }),
                    ),
            )
    }

    fn render_huddle_theme_panel(
        &self,
        active_bg: Option<&str>,
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        const THEMES: [(&str, &str, u32); 7] = [
            ("default", "Match theme", 0xCBEAD7),
            ("meadow", "Meadow", 0xBCE3CC),
            ("sky", "Daylight", 0xC5E5FA),
            ("midnight", "Midnight", 0x10172A),
            ("forest", "Forest", 0x11221B),
            ("sunset", "Sunset", 0x2B1818),
            ("plum", "Aubergine", 0x24142B),
        ];
        let send = self.send.clone();
        let hover = pal.popover_hover;
        let surface = pal.drawer_surface;
        h_flex()
            .p_3()
            .flex_wrap()
            .gap_2()
            .items_center()
            .rounded_2xl()
            .border_1()
            .border_color(pal.popover_border)
            .bg(pal.popover_bg)
            .shadow_2xl()
            .text_color(pal.popover_fg)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(pal.popover_muted)
                    .mr_1()
                    .child("Background"),
            )
            .children(THEMES.iter().enumerate().map(|(ix, &(id, label, hex))| {
                let selected = active_bg == Some(id) || (active_bg.is_none() && id == "default");
                let bg_id = id.to_owned();
                let send_bg = send.clone();
                h_flex()
                    .id(("popout-huddle-theme", ix))
                    .px_2p5()
                    .py_1()
                    .gap_1p5()
                    .items_center()
                    .rounded_full()
                    .cursor_pointer()
                    .bg(if selected { hover } else { surface })
                    .border_1()
                    .border_color(if selected {
                        pal.success
                    } else {
                        pal.popover_border
                    })
                    .hover(move |el| el.bg(hover))
                    .text_xs()
                    .text_color(pal.popover_fg)
                    .child(
                        div()
                            .size(px(12.))
                            .rounded_full()
                            .bg(rgb(hex))
                            .border_1()
                            .border_color(pal.popover_border),
                    )
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        (send_bg)(Command::SetHuddleBackground(bg_id.clone()));
                        if let Some(dock) = this.huddle_dock_entity() {
                            dock.update(cx, |d, cx| d.set_background_local(bg_id.clone(), cx));
                        }
                        this.huddle_theme_open = false;
                        cx.notify();
                    }))
            }))
    }

    fn render_huddle_invite_panel(
        &self,
        channel: &ChannelId,
        people: &[slackless_core::types::UserId],
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let candidates: Vec<_> = self
            .fixtures
            .users
            .iter()
            .filter(|u| !u.is_bot && !u.deleted && !people.contains(&u.id))
            .take(12)
            .cloned()
            .collect();
        let inv_ch = channel.clone();
        let send_inv = self.send.clone();
        let hover = pal.popover_hover;
        let surface = pal.drawer_surface;

        v_flex()
            .max_w(px(520.))
            .p_3()
            .gap_2()
            .rounded_2xl()
            .border_1()
            .border_color(pal.popover_border)
            .bg(pal.popover_bg)
            .shadow_2xl()
            .text_color(pal.popover_fg)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(pal.popover_muted)
                    .child("Invite people to this huddle"),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .children(candidates.into_iter().enumerate().map(|(ix, u)| {
                        let uid = u.id.clone();
                        let ch = inv_ch.clone();
                        let send = send_inv.clone();
                        let label = self.fixtures.user_name(&uid).to_owned();
                        h_flex()
                            .id(("popout-huddle-inv", ix))
                            .px_2p5()
                            .py_1()
                            .gap_1p5()
                            .items_center()
                            .rounded_full()
                            .cursor_pointer()
                            .bg(surface)
                            .hover(move |el| el.bg(hover))
                            .text_xs()
                            .text_color(pal.popover_fg)
                            .child(
                                gpui_kit::component::Icon::new(IconName::UserPlus)
                                    .size(px(12.))
                                    .text_color(pal.popover_muted),
                            )
                            .child(label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                (send)(Command::InviteToHuddle {
                                    channel: ch.clone(),
                                    users: vec![uid.clone()],
                                });
                                actions::notify(
                                    "Invitation sent",
                                    "Notified member to join this huddle.",
                                    window,
                                    cx,
                                );
                                this.huddle_invite_open = false;
                                cx.notify();
                            }))
                    })),
            )
    }

    fn render_huddle_devices_panel(
        &self,
        cur_in: Option<String>,
        cur_out: Option<String>,
        pal: &HuddlePalette,
        cx: &Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let send_in = self.send.clone();
        let send_out = self.send.clone();
        let hover = pal.popover_hover;
        let surface = pal.drawer_surface;
        let primary = pal.primary;
        let primary_fg = pal.primary_fg;

        v_flex()
            .max_w(px(540.))
            .p_3()
            .gap_2()
            .rounded_2xl()
            .border_1()
            .border_color(pal.popover_border)
            .bg(pal.popover_bg)
            .shadow_2xl()
            .text_color(pal.popover_fg)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(pal.popover_muted)
                    .child("Microphone"),
            )
            .child(
                h_flex().flex_wrap().gap_1p5().children(
                    self.huddle_devices
                        .inputs
                        .iter()
                        .enumerate()
                        .map(|(ix, d)| {
                            let selected = cur_in.as_deref() == Some(d.name.as_str())
                                || (cur_in.is_none() && d.is_default);
                            let name = d.name.clone();
                            let out = cur_out.clone();
                            let send = send_in.clone();
                            div()
                                .id(("popout-in-dev", ix))
                                .px_2p5()
                                .py_1()
                                .rounded_lg()
                                .cursor_pointer()
                                .bg(if selected { primary } else { surface })
                                .hover(move |el| el.bg(if selected { primary } else { hover }))
                                .text_xs()
                                .text_color(if selected { primary_fg } else { pal.popover_fg })
                                .child(if selected {
                                    format!("✓ {}", d.name)
                                } else {
                                    d.name.clone()
                                })
                                .on_click(move |_, _, _| {
                                    (send)(Command::SetHuddleAudioDevices {
                                        input: Some(name.clone()),
                                        output: out.clone(),
                                    });
                                })
                        }),
                ),
            )
            .child(
                div()
                    .mt_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(pal.popover_muted)
                    .child("Speaker"),
            )
            .child(
                h_flex().flex_wrap().gap_1p5().children(
                    self.huddle_devices
                        .outputs
                        .iter()
                        .enumerate()
                        .map(|(ix, d)| {
                            let selected = cur_out.as_deref() == Some(d.name.as_str())
                                || (cur_out.is_none() && d.is_default);
                            let name = d.name.clone();
                            let inp = cur_in.clone();
                            let send = send_out.clone();
                            div()
                                .id(("popout-out-dev", ix))
                                .px_2p5()
                                .py_1()
                                .rounded_lg()
                                .cursor_pointer()
                                .bg(if selected { primary } else { surface })
                                .hover(move |el| el.bg(if selected { primary } else { hover }))
                                .text_xs()
                                .text_color(if selected { primary_fg } else { pal.popover_fg })
                                .child(if selected {
                                    format!("✓ {}", d.name)
                                } else {
                                    d.name.clone()
                                })
                                .on_click(move |_, _, _| {
                                    (send)(Command::SetHuddleAudioDevices {
                                        input: inp.clone(),
                                        output: Some(name.clone()),
                                    });
                                })
                        }),
                ),
            )
            .child(div().my_1().h(px(1.)).bg(pal.popover_border))
            .child(
                h_flex()
                    .id("popout-audio-prefs")
                    .px_2p5()
                    .py_1p5()
                    .gap_2()
                    .items_center()
                    .rounded_lg()
                    .cursor_pointer()
                    .hover(move |el| el.bg(hover))
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(pal.popover_fg)
                    .child(
                        gpui_kit::component::Icon::new(IconName::Settings)
                            .size(px(14.))
                            .text_color(pal.popover_muted),
                    )
                    .child("Audio & video preferences")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_audio_video_preferences(cx);
                    })),
            )
    }
}
