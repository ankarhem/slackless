//! Huddle stage layout: screen share canvas, local drawing input, video tiles, filmstrip, reactions & captions.

use super::*;

struct ParticipantTileLayout {
    tile_w: Pixels,
    tile_h: Pixels,
    avatar_px: f32,
    full_bleed_avatar: bool,
}

impl PopoutWindow {
    pub(super) fn render_huddle_stage(
        &self,
        state: &crate::huddle_state::HuddleState,
        pal: &HuddlePalette,
        cx: &Context<Self>,
    ) -> Option<gpui_kit::gpui::Div> {
        use slackless_core::huddle::{HuddleDrawMessage, VideoTileKey};

        let mut screens: Vec<(String, String, crate::huddle_state::DisplayVideoFrame)> = Vec::new();
        if let Some(vs) = self.huddle_video.as_ref().and_then(WeakEntity::upgrade) {
            let frames = vs.read(cx).frames();
            for (k, v) in frames {
                if let VideoTileKey::RemoteScreen(uid) = k {
                    screens.push((
                        uid.to_string(),
                        format!("{} is sharing their screen", self.fixtures.user_name(uid)),
                        v.clone(),
                    ));
                }
            }
            screens.sort_by(|a, b| a.0.cmp(&b.0));
            if let Some(v) = frames.get(&VideoTileKey::LocalScreen) {
                let local_item = if !state.session().attendee_id.is_empty() {
                    state.session().attendee_id.clone()
                } else {
                    self.fixtures.self_user.to_string()
                };
                screens.push((
                    local_item,
                    "You are sharing your screen".to_owned(),
                    v.clone(),
                ));
            }
        }
        if screens.is_empty() {
            return None;
        }

        let focused = self.huddle_focused_screen.filter(|&i| i < screens.len());
        let primary_item_id = screens
            .get(focused.unwrap_or(0))
            .map(|s| s.0.clone())
            .unwrap_or_default();
        let header_title = if let Some(ix) = focused {
            screens[ix].1.clone()
        } else if screens.len() > 1 {
            format!("{} screens shared", screens.len())
        } else {
            screens[0].1.clone()
        };

        const HUES: [(i32, &str); 6] = [
            (0, "Red"),
            (35, "Orange"),
            (55, "Yellow"),
            (135, "Green"),
            (210, "Blue"),
            (285, "Purple"),
        ];

        let white = pal.scrim_fg;
        let emerald = pal.success;
        let primary = pal.primary;
        let draw_mode = state.draw_mode();
        let draw_hue = state.draw_hue();
        let draw_btn = {
            let item = primary_item_id.clone();
            h_flex()
                .id("popout-huddle-stage-draw")
                .h(px(26.))
                .px_2p5()
                .gap_1()
                .items_center()
                .rounded_full()
                .cursor_pointer()
                .bg(if draw_mode {
                    primary
                } else {
                    white.opacity(0.11)
                })
                .hover(move |el| {
                    el.bg(if draw_mode {
                        emerald
                    } else {
                        white.opacity(0.18)
                    })
                })
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(white)
                .child(
                    gpui_kit::component::Icon::new(IconName::Pencil)
                        .size(px(12.))
                        .text_color(white),
                )
                .child(if draw_mode { "Drawing" } else { "Draw" })
                .on_click(cx.listener(move |this, _, _, cx| {
                    let next = !draw_mode;
                    this.huddle_draw_mode = next;
                    this.huddle_drawing_active = false;
                    this.huddle_draw_points.clear();
                    this.huddle_draw_local_segments.clear();
                    this.huddle_draw_local_item.clear();
                    if let Some(dock) = this.huddle_state_entity() {
                        dock.update(cx, |d, cx| d.set_draw_mode(next, cx));
                    } else if next {
                        let msg = HuddleDrawMessage::ChangeHue {
                            item_id: item.clone(),
                            hue: draw_hue,
                        };
                        (this.send)(send_huddle_draw(msg.clone()));
                        this.apply_local_draw(msg, cx);
                    }
                    cx.notify();
                }))
        };

        let swatches = draw_mode.then(|| {
            h_flex()
                .items_center()
                .gap_1()
                .px_1()
                .children(HUES.iter().enumerate().map(|(ix, &(hue, _label))| {
                    let selected = draw_hue == hue;
                    let item = primary_item_id.clone();
                    div()
                        .id(("popout-draw-hue", ix))
                        .size(px(15.))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(crate::theme::huddle_draw_hue_color(hue, 0.88, 0.56, 1.0))
                        .border_2()
                        .border_color(if selected { white } else { white.opacity(0.19) })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.huddle_draw_hue = hue;
                            if let Some(dock) = this.huddle_state_entity() {
                                dock.update(cx, |d, cx| d.set_draw_hue(hue, cx));
                            } else {
                                let msg = HuddleDrawMessage::ChangeHue {
                                    item_id: item.clone(),
                                    hue,
                                };
                                (this.send)(send_huddle_draw(msg.clone()));
                                this.apply_local_draw(msg, cx);
                            }
                            cx.notify();
                        }))
                }))
        });

        let screen_tabs = (screens.len() > 1).then(|| {
            h_flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .id("popout-huddle-split-all")
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .cursor_pointer()
                        .bg(if focused.is_none() {
                            white.opacity(0.16)
                        } else {
                            white.opacity(0.06)
                        })
                        .text_xs()
                        .text_color(white)
                        .child("Split")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.huddle_focused_screen = None;
                            cx.notify();
                        })),
                )
                .children(screens.iter().enumerate().map(|(ix, (_, lbl, _))| {
                    let is_f = focused == Some(ix);
                    let short = lbl
                        .split(" is ")
                        .next()
                        .unwrap_or("Screen")
                        .replace("You are sharing your screen", "You");
                    div()
                        .id(("popout-huddle-screen-tab", ix))
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .cursor_pointer()
                        .bg(if is_f {
                            white.opacity(0.16)
                        } else {
                            white.opacity(0.06)
                        })
                        .text_xs()
                        .text_color(white)
                        .child(short)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.huddle_focused_screen = Some(ix);
                            cx.notify();
                        }))
                }))
        });

        let visible_screens: Vec<(String, String, crate::huddle_state::DisplayVideoFrame)> =
            if let Some(ix) = focused {
                vec![screens[ix].clone()]
            } else {
                screens
            };

        Some(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .mt(px(36.))
                .rounded_2xl()
                .overflow_hidden()
                .border_1()
                .border_color(pal.drawer_border)
                .bg(crate::theme::media_backdrop_bg())
                .child(h_flex().size_full().gap_1p5().children(
                    visible_screens.into_iter().enumerate().map(
                        |(ix, (item_id, _lbl, img_frame))| {
                            self.render_huddle_screen_pane(ix, item_id, img_frame, state, cx)
                        },
                    ),
                ))
                .child(
                    h_flex()
                        .absolute()
                        .left(px(12.))
                        .top(px(12.))
                        .px_2p5()
                        .py_1()
                        .gap_2()
                        .items_center()
                        .rounded_full()
                        .bg(pal.scrim_bg)
                        .border_1()
                        .border_color(white.opacity(0.10))
                        .child(
                            gpui_kit::component::Icon::new(IconName::ScreenShare)
                                .size(px(12.))
                                .text_color(emerald),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(white)
                                .child(header_title),
                        )
                        .children(screen_tabs),
                )
                .child(
                    h_flex()
                        .absolute()
                        .right(px(12.))
                        .top(px(12.))
                        .p_1()
                        .gap_1()
                        .items_center()
                        .rounded_full()
                        .bg(pal.scrim_bg)
                        .border_1()
                        .border_color(white.opacity(0.10))
                        .children(swatches)
                        .child(draw_btn)
                        .child(
                            div()
                                .id("popout-huddle-stage-floating-panel")
                                .h(px(26.))
                                .px_2()
                                .rounded_full()
                                .cursor_pointer()
                                .bg(white.opacity(0.08))
                                .hover(move |el| el.bg(white.opacity(0.16)))
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .text_color(white)
                                .child(
                                    gpui_kit::component::Icon::new(IconName::Pin)
                                        .size(px(11.))
                                        .text_color(white),
                                )
                                .child("Controls")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(dock) = this.huddle_state_entity() {
                                        dock.update(cx, |d, cx| {
                                            d.request_open_floating_panel(cx);
                                        });
                                    }
                                })),
                        )
                        .child(
                            div()
                                .id("popout-huddle-stage-expand")
                                .h(px(26.))
                                .px_2()
                                .rounded_full()
                                .cursor_pointer()
                                .bg(white.opacity(0.08))
                                .hover(move |el| el.bg(white.opacity(0.16)))
                                .flex()
                                .items_center()
                                .text_xs()
                                .text_color(white)
                                .child(if self.huddle_stage_expanded {
                                    "Compact"
                                } else {
                                    "Expand"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.huddle_stage_expanded = !this.huddle_stage_expanded;
                                    cx.notify();
                                })),
                        ),
                ),
        )
    }

    fn apply_local_draw(
        &self,
        message: slackless_core::huddle::HuddleDrawMessage,
        cx: &mut Context<Self>,
    ) {
        if let Some(dock) = self.huddle_state_entity() {
            let me = self.fixtures.self_user.clone();
            dock.update(cx, |d, cx| {
                d.apply_data_event(
                    &slackless_core::huddle::HuddleDataEvent::Draw {
                        user: me,
                        message,
                        at_ms: 0,
                    },
                    cx,
                );
            });
        }
    }

    fn render_huddle_screen_pane(
        &self,
        pane_ix: usize,
        item_id: String,
        frame: crate::huddle_state::DisplayVideoFrame,
        state: &crate::huddle_state::HuddleState,
        cx: &Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        use gpui_kit::gpui::{Corners, ObjectFit};
        use slackless_core::huddle::HuddleDrawMessage;

        let filter_id = item_id.clone();
        let strokes: Vec<_> = state
            .draw_strokes()
            .iter()
            .filter(|s| s.item_id.is_empty() || s.item_id == filter_id || pane_ix == 0)
            .cloned()
            .collect();
        let active_hue = state.draw_hue();
        let local_live_segments = if pane_ix == 0
            && self.huddle_drawing_active
            && !self.huddle_draw_local_segments.is_empty()
        {
            Some((active_hue, self.huddle_draw_local_segments.clone()))
        } else {
            None
        };
        let cursors: Vec<_> = state
            .draw_cursors()
            .iter()
            .filter(|(_, c)| c.item_id.is_empty() || c.item_id == filter_id || pane_ix == 0)
            .map(|(uid, c)| {
                let is_me = *uid == self.fixtures.self_user;
                (is_me, c.clone())
            })
            .collect();
        let bounds_cell = Arc::clone(&self.huddle_stage_bounds);
        let (fw, fh) = frame.dimensions();
        #[cfg(target_os = "macos")]
        let is_surface = matches!(frame, crate::huddle_state::DisplayVideoFrame::Surface(_));
        #[cfg(not(target_os = "macos"))]
        let is_surface = false;
        // Render the video frame directly inside a full-size `canvas` with `ObjectFit::Contain`
        // instead of GPUI's `img` element: `Img::request_layout` injects `style.aspect_ratio`,
        // which causes Taffy to expand height from `width: 100%` and clip the bottom of tall
        // windows and 16:10 Retina displays inside `overflow_hidden`.
        // On macOS, `DisplayVideoFrame::Surface` paints zero-copy directly via GPUI's Metal
        // YCbCr shader (`window.paint_surface`) with zero CPU pixel copy or atlas churn!
        let stage_canvas = canvas(
            move |bounds, _, _| {
                let fit_bounds = if is_surface {
                    // Subtle 4px bezel keeps `paint_surface` strictly inside the rounded_2xl
                    // dark stage border with zero corner bleed.
                    let inset = px(4.);
                    Bounds {
                        origin: point(bounds.origin.x + inset, bounds.origin.y + inset),
                        size: size(
                            (bounds.size.width - inset * 2.).max(px(1.)),
                            (bounds.size.height - inset * 2.).max(px(1.)),
                        ),
                    }
                } else {
                    bounds
                };
                let image_bounds = ObjectFit::Contain.get_bounds(
                    fit_bounds,
                    size(
                        gpui_kit::gpui::DevicePixels(fw as i32),
                        gpui_kit::gpui::DevicePixels(fh as i32),
                    ),
                );
                if pane_ix == 0
                    && let Ok(mut guard) = bounds_cell.lock()
                {
                    *guard = image_bounds;
                }
                image_bounds
            },
            move |bounds, image_bounds, window, _| {
                match &frame {
                    #[cfg(target_os = "macos")]
                    crate::huddle_state::DisplayVideoFrame::Surface(surface) => {
                        let ptr = surface.retain_cv_pixel_buffer_ptr()
                            as *mut objc2_core_video::CVPixelBuffer;
                        if let Some(nn) = std::ptr::NonNull::new(ptr) {
                            // SAFETY: `retain_cv_pixel_buffer_ptr` returned a +1 retained
                            // `420f` IOSurface-backed `CVPixelBuffer`; `CFRetained::from_raw`
                            // takes ownership and releases on Drop.
                            let cv_buf = unsafe { objc2_core_foundation::CFRetained::from_raw(nn) };
                            window.paint_surface(image_bounds, cv_buf);
                        }
                    }
                    crate::huddle_state::DisplayVideoFrame::Image(img_frame) => {
                        if img_frame.frame_count() > 0 {
                            let _ = window.paint_image(
                                bounds,
                                image_bounds,
                                Corners::all(px(15.)),
                                Arc::clone(img_frame),
                                0,
                                false,
                            );
                        }
                    }
                }
                let left: f32 = image_bounds.origin.x.into();
                let top: f32 = image_bounds.origin.y.into();
                let width: f32 = image_bounds.size.width.into();
                let height: f32 = image_bounds.size.height.into();
                if width <= 1.0 || height <= 1.0 {
                    return;
                }
                let now_ms = current_time_ms();
                let mut needs_anim_frame = false;

                for stroke in &strokes {
                    if stroke.segments.is_empty() {
                        continue;
                    }
                    let (alpha, width_scale, fading) =
                        stroke_fade_curve(stroke.ended_at_ms, now_ms);
                    if fading {
                        needs_anim_frame = true;
                    }
                    if alpha <= 0.005 {
                        continue;
                    }
                    paint_smooth_stroke(
                        window,
                        &stroke.segments,
                        StrokePaintParams {
                            hue: stroke.hue,
                            alpha,
                            width_scale,
                            left,
                            top,
                            width,
                            height,
                        },
                    );
                }
                if let Some((hue, ref live_segs)) = local_live_segments {
                    paint_smooth_stroke(
                        window,
                        live_segs,
                        StrokePaintParams {
                            hue,
                            alpha: 1.0,
                            width_scale: 1.0,
                            left,
                            top,
                            width,
                            height,
                        },
                    );
                }
                for (is_me, cur) in &cursors {
                    if *is_me {
                        continue;
                    }
                    let cx_px = left + cur.x as f32 * width;
                    let cy_px = top + cur.y as f32 * height;
                    if let Ok(halo) = build_smooth_circle(cx_px, cy_px, 11.0).build() {
                        window.paint_path(
                            halo,
                            crate::theme::huddle_draw_hue_color(cur.hue, 0.90, 0.56, 0.22),
                        );
                    }
                    if let Ok(ring) = build_smooth_circle(cx_px, cy_px, 6.5).build() {
                        window.paint_path(ring, crate::theme::huddle_draw_cursor_ring());
                    }
                    if let Ok(dot) = build_smooth_circle(cx_px, cy_px, 4.5).build() {
                        window.paint_path(
                            dot,
                            crate::theme::huddle_draw_hue_color(cur.hue, 0.92, 0.54, 0.98),
                        );
                    }
                }
                if needs_anim_frame {
                    window.request_animation_frame();
                }
            },
        )
        .size_full();

        let item_down = item_id.clone();
        let item_move = item_id.clone();
        let item_up = item_id.clone();
        let item_up_out = item_id;

        let mut pane = div()
            .id(("popout-huddle-stage-pane", pane_ix))
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .rounded(px(15.))
            .overflow_hidden()
            .child(stage_canvas);

        if state.draw_mode() && pane_ix == 0 {
            pane = pane
                .cursor_crosshair()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        // Never merge into a stale un-ended stroke if a prior mouse-up was missed.
                        if this.huddle_drawing_active {
                            this.finish_local_draw_stroke(&item_down, cx);
                        }
                        let Some(pt) = this.normalize_stage_point(ev.position) else {
                            return;
                        };
                        this.huddle_drawing_active = true;
                        this.huddle_draw_points.clear();
                        this.huddle_draw_points.push(pt);
                        this.huddle_draw_local_segments.clear();
                        this.huddle_draw_local_item = item_down.clone();
                        // Emit an initial dot segment so single-click highlights work cleanly.
                        let dot_seg = make_catmull_rom_segment(pt, pt, pt, pt);
                        this.huddle_draw_local_segments.push(dot_seg);
                        let hue = this
                            .huddle_state_entity()
                            .map_or(this.huddle_draw_hue, |d| d.read(cx).draw_hue());
                        let hue_msg = HuddleDrawMessage::ChangeHue {
                            item_id: item_down.clone(),
                            hue,
                        };
                        (this.send)(send_huddle_draw(hue_msg));
                        cx.notify();
                    }),
                )
                .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                    // If the left button was released anywhere (e.g. over an overlay pill or
                    // outside the window), finalize the previous stroke immediately so moving
                    // to write the next letter never bridges them with a connecting line.
                    if this.huddle_drawing_active && ev.pressed_button != Some(MouseButton::Left) {
                        this.finish_local_draw_stroke(&item_move, cx);
                    }
                    let Some(raw_pt) = this.normalize_stage_point(ev.position) else {
                        return;
                    };
                    if this.huddle_drawing_active {
                        if let Some(&prev) = this.huddle_draw_points.last() {
                            // Measure distance in actual stage pixels (aspect-corrected) so
                            // curves & handwriting have crisp uniform density.
                            let (stage_w, stage_h) = this.stage_pixel_size();
                            let dx_px = (raw_pt.0 - prev.0) * stage_w;
                            let dy_px = (raw_pt.1 - prev.1) * stage_h;
                            if dx_px * dx_px + dy_px * dy_px < 2.5 * 2.5 {
                                return;
                            }
                            let pt = (
                                prev.0 * 0.18 + raw_pt.0 * 0.82,
                                prev.1 * 0.18 + raw_pt.1 * 0.82,
                            );
                            this.huddle_draw_points.push(pt);
                            let n = this.huddle_draw_points.len();
                            if n == 2 {
                                let p0 = this.huddle_draw_points[0];
                                let p1 = this.huddle_draw_points[1];
                                let seg = make_catmull_rom_segment(p0, p0, p1, p1);
                                this.huddle_draw_local_segments.clear();
                                this.huddle_draw_local_segments.push(seg);
                                (this.send)(send_huddle_draw(HuddleDrawMessage::New {
                                    item_id: item_move.clone(),
                                    segment: seg,
                                }));
                            } else {
                                let p_prev2 = if n >= 4 {
                                    this.huddle_draw_points[n - 4]
                                } else {
                                    this.huddle_draw_points[0]
                                };
                                let p_prev1 = this.huddle_draw_points[n - 3];
                                let p_curr = this.huddle_draw_points[n - 2];
                                let p_next = this.huddle_draw_points[n - 1];
                                let refined =
                                    make_catmull_rom_segment(p_prev2, p_prev1, p_curr, p_next);
                                if let Some(last) = this.huddle_draw_local_segments.last_mut() {
                                    *last = refined;
                                }
                                (this.send)(send_huddle_draw(HuddleDrawMessage::Edit {
                                    item_id: item_move.clone(),
                                    segment: refined,
                                }));
                                let new_seg =
                                    make_catmull_rom_segment(p_prev1, p_curr, p_next, p_next);
                                this.huddle_draw_local_segments.push(new_seg);
                                (this.send)(send_huddle_draw(HuddleDrawMessage::New {
                                    item_id: item_move.clone(),
                                    segment: new_seg,
                                }));
                            }
                            cx.notify();
                        }
                    } else {
                        let now = current_time_ms();
                        if now.saturating_sub(this.huddle_draw_last_cursor_ms) >= 33 {
                            this.huddle_draw_last_cursor_ms = now;
                            let pos_msg = HuddleDrawMessage::Position {
                                item_id: item_move.clone(),
                                x: raw_pt.0,
                                y: raw_pt.1,
                            };
                            (this.send)(send_huddle_draw(pos_msg));
                        }
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseUpEvent, _, cx| {
                        this.finish_local_draw_stroke(&item_up, cx);
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseUpEvent, _, cx| {
                        this.finish_local_draw_stroke(&item_up_out, cx);
                    }),
                );
        }
        pane
    }

    fn finish_local_draw_stroke(&mut self, fallback_item: &str, cx: &mut Context<Self>) {
        use slackless_core::huddle::HuddleDrawMessage;
        if !self.huddle_drawing_active {
            return;
        }
        self.huddle_drawing_active = false;
        let segs = std::mem::take(&mut self.huddle_draw_local_segments);
        let item = if self.huddle_draw_local_item.is_empty() {
            fallback_item.to_owned()
        } else {
            std::mem::take(&mut self.huddle_draw_local_item)
        };
        if self.huddle_draw_points.len() == 1
            && let Some(&dot) = segs.first()
        {
            (self.send)(send_huddle_draw(HuddleDrawMessage::New {
                item_id: item.clone(),
                segment: dot,
            }));
        }
        self.huddle_draw_points.clear();
        let end_msg = HuddleDrawMessage::End {
            item_id: item.clone(),
        };
        (self.send)(send_huddle_draw(end_msg));
        if let Some(dock) = self.huddle_state_entity() {
            let hue = dock.read(cx).draw_hue();
            dock.update(cx, |d, cx| {
                d.commit_local_draw_stroke(item, hue, segs, cx);
            });
        }
        cx.notify();
    }

    fn stage_pixel_size(&self) -> (f64, f64) {
        self.huddle_stage_bounds
            .lock()
            .ok()
            .map(|b| {
                let w: f32 = b.size.width.into();
                let h: f32 = b.size.height.into();
                (w.max(1.0) as f64, h.max(1.0) as f64)
            })
            .unwrap_or((1280.0, 720.0))
    }

    fn normalize_stage_point(&self, pos: Point<Pixels>) -> Option<(f64, f64)> {
        let bounds = *self.huddle_stage_bounds.lock().ok()?;
        let w: f32 = bounds.size.width.into();
        let h: f32 = bounds.size.height.into();
        if w <= 1.0 || h <= 1.0 {
            return None;
        }
        let x: f32 = (pos.x - bounds.origin.x).into();
        let y: f32 = (pos.y - bounds.origin.y).into();
        Some((
            (x / w).clamp(0.0, 1.0) as f64,
            (y / h).clamp(0.0, 1.0) as f64,
        ))
    }

    pub(super) fn render_huddle_tiles(
        &self,
        state: &crate::huddle_state::HuddleState,
        people: &[slackless_core::types::UserId],
        pal: &HuddlePalette,
        cx: &Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let solo = people.len() <= 1;
        // Official Slack renders a single participant as a crisp centered rounded-square portrait
        // card, and scales smoothly into balanced square/4:3 cards as more teammates join.
        let (tile_w, tile_h, avatar_px, grid_max_w) = match people.len() {
            0..=1 => (px(276.), px(276.), 272.0, px(360.)),
            2 => (px(272.), px(252.), 112.0, px(640.)),
            3 => (px(236.), px(216.), 88.0, px(780.)),
            4 => (px(252.), px(208.), 84.0, px(560.)),
            _ => (px(212.), px(164.), 66.0, px(760.)),
        };
        h_flex()
            .max_w(grid_max_w)
            .flex_wrap()
            .items_center()
            .justify_center()
            .gap_4()
            .children(people.iter().map(|uid| {
                self.render_huddle_participant_tile(
                    state,
                    uid,
                    pal,
                    ParticipantTileLayout {
                        tile_w,
                        tile_h,
                        avatar_px,
                        full_bleed_avatar: solo,
                    },
                    cx,
                )
            }))
    }

    pub(super) fn render_huddle_filmstrip(
        &self,
        state: &crate::huddle_state::HuddleState,
        people: &[slackless_core::types::UserId],
        pal: &HuddlePalette,
        cx: &Context<Self>,
    ) -> gpui_kit::gpui::Div {
        if self.huddle_stage_expanded {
            return div();
        }
        h_flex()
            .flex_none()
            .w_full()
            .items_center()
            .justify_center()
            .gap_2p5()
            .children(people.iter().take(6).map(|uid| {
                self.render_huddle_participant_tile(
                    state,
                    uid,
                    pal,
                    ParticipantTileLayout {
                        tile_w: px(148.),
                        tile_h: px(96.),
                        avatar_px: 44.0,
                        full_bleed_avatar: false,
                    },
                    cx,
                )
            }))
    }

    fn render_huddle_participant_tile(
        &self,
        state: &crate::huddle_state::HuddleState,
        uid: &slackless_core::types::UserId,
        pal: &HuddlePalette,
        layout: ParticipantTileLayout,
        cx: &Context<Self>,
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;
        use gpui_kit::gpui::ObjectFit;
        use slackless_core::huddle::VideoTileKey;

        let ParticipantTileLayout {
            tile_w,
            tile_h,
            avatar_px,
            full_bleed_avatar,
        } = layout;
        let snap = state.session();
        let is_self = *uid == self.fixtures.self_user;
        let speaking =
            crate::huddle_video::is_user_speaking(is_self, snap.muted, state.levels(), uid);
        let is_muted = if is_self {
            snap.muted
        } else {
            state.levels().muted_peers.contains(uid)
        };
        let badge_glyph = state
            .sticky_emojis()
            .get(uid)
            .map(|s| sticky_emoji_glyph(s).to_owned())
            .or_else(|| state.raised_hands().contains(uid).then(|| "✋".to_owned()));
        let video_frame = self
            .huddle_video
            .as_ref()
            .and_then(WeakEntity::upgrade)
            .and_then(|vs| {
                let frames = vs.read(cx).frames();
                let key = if is_self {
                    VideoTileKey::LocalCamera
                } else {
                    VideoTileKey::RemoteCamera(uid.clone())
                };
                frames.get(&key).and_then(|f| f.as_render_image().cloned())
            });
        let has_video = video_frame.is_some();
        let name = self.fixtures.user_name(uid).to_owned();
        let display_name = if is_self {
            format!("{name} (You)")
        } else {
            name
        };

        let pill_bg = if has_video {
            pal.scrim_bg
        } else {
            pal.tile_pill_bg
        };
        let pill_fg = if has_video {
            pal.scrim_fg
        } else {
            pal.tile_pill_fg
        };

        div()
            .relative()
            .w(tile_w)
            .h(tile_h)
            .rounded_2xl()
            .overflow_hidden()
            .bg(pal.tile_bg)
            .shadow_xl()
            .border(px(if speaking { 3. } else { 2. }))
            .border_color(if speaking {
                pal.success
            } else {
                pal.tile_border
            })
            .child(if let Some(frame) = video_frame {
                let inner_radius = px(if speaking { 13. } else { 14. });
                let frame_layout = Arc::clone(&frame);
                canvas(
                    move |bounds, _, _| ObjectFit::Cover.get_bounds(bounds, frame_layout.size(0)),
                    move |bounds, img_bounds, window, _| {
                        if frame.frame_count() > 0 {
                            let _ = window.paint_image(
                                bounds,
                                img_bounds,
                                gpui_kit::gpui::Corners::all(inner_radius),
                                Arc::clone(&frame),
                                0,
                                false,
                            );
                        }
                    },
                )
                .size_full()
                .into_any_element()
            } else if full_bleed_avatar {
                div()
                    .size_full()
                    .rounded(px(14.))
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(crate::avatars::user_chip(&self.fixtures, uid, avatar_px))
                    .into_any_element()
            } else {
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .p(px(3.))
                            .rounded_2xl()
                            .border_2()
                            .border_color(if speaking {
                                pal.success.opacity(0.65)
                            } else {
                                gpui_kit::gpui::transparent_black()
                            })
                            .child(crate::avatars::user_chip(&self.fixtures, uid, avatar_px)),
                    )
                    .into_any_element()
            })
            // Top-left sticky/hand-raise pill
            .when_some(badge_glyph, |el, glyph| {
                el.child(
                    div()
                        .absolute()
                        .left(px(10.))
                        .top(px(10.))
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .bg(pill_bg)
                        .border_1()
                        .border_color(pal.drawer_border)
                        .shadow_sm()
                        .text_sm()
                        .child(glyph),
                )
            })
            // Bottom-left compact audio/name badge (matches official Slack's clean white pill)
            .child(
                h_flex()
                    .absolute()
                    .left(px(10.))
                    .bottom(px(10.))
                    .max_w(tile_w - px(20.))
                    .px_2()
                    .py_1()
                    .gap_1p5()
                    .items_center()
                    .rounded_full()
                    .bg(pill_bg)
                    .shadow_sm()
                    .when(is_muted, |pill| {
                        pill.child(
                            Icon::new(IconName::MicOff)
                                .size(px(11.))
                                .text_color(pal.danger),
                        )
                    })
                    .when(!is_muted, |pill| {
                        pill.child(div().size(px(7.)).rounded_full().bg(if speaking {
                            pal.success
                        } else {
                            pal.drawer_muted
                        }))
                    })
                    .when(!full_bleed_avatar, |pill| {
                        pill.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(pill_fg)
                                .child(display_name),
                        )
                    }),
            )
    }

    pub(super) fn render_huddle_floating_reactions(
        &self,
        state: &crate::huddle_state::HuddleState,
        pal: &HuddlePalette,
    ) -> Option<gpui_kit::gpui::Div> {
        if state.reactions().is_empty() {
            return None;
        }
        let mut stacked: Vec<(String, usize, Vec<String>)> = Vec::new();
        for entry in state.reactions().iter() {
            let who = self.fixtures.user_name(&entry.user).to_owned();
            if let Some(slot) = stacked.iter_mut().find(|(em, _, _)| *em == entry.emoji) {
                slot.1 += 1;
                if !slot.2.contains(&who) {
                    slot.2.push(who);
                }
            } else {
                stacked.push((entry.emoji.clone(), 1, vec![who]));
            }
        }
        Some(
            h_flex()
                .absolute()
                .left(px(18.))
                .bottom(px(14.))
                .flex_wrap()
                .gap_1p5()
                .children(stacked.into_iter().map(|(emoji, count, names)| {
                    let glyph = sticky_emoji_glyph(&emoji);
                    let label = if count > 1 {
                        format!("{glyph} ×{count}")
                    } else {
                        glyph.to_owned()
                    };
                    h_flex()
                        .px_2p5()
                        .py_1()
                        .gap_1p5()
                        .items_center()
                        .rounded_full()
                        .bg(pal.popover_bg)
                        .border_1()
                        .border_color(pal.popover_border)
                        .shadow_lg()
                        .text_xs()
                        .text_color(pal.popover_fg)
                        .child(label)
                        .child(div().text_color(pal.popover_muted).child(names.join(", ")))
                })),
        )
    }

    pub(super) fn render_huddle_floating_captions(
        &self,
        state: &crate::huddle_state::HuddleState,
        pal: &HuddlePalette,
    ) -> Option<gpui_kit::gpui::Div> {
        let show_captions = state.session().captions_enabled || !state.captions().is_empty();
        if !show_captions {
            return None;
        }
        let latest_lang = state
            .captions()
            .iter()
            .rev()
            .find_map(|line| line.lang.as_deref())
            .map(|l| l.to_ascii_uppercase());

        let header = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .pb_1()
            .border_b_1()
            .border_color(pal.scrim_fg.opacity(0.07))
            .child(
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .child(div().size(px(6.)).rounded_full().bg(pal.success))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(pal.scrim_muted)
                            .child("Live captions"),
                    ),
            )
            .when_some(latest_lang, |row, code| {
                row.child(
                    div()
                        .px_1p5()
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(pal.scrim_fg.opacity(0.08))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(pal.scrim_fg)
                        .child(code),
                )
            });

        let mut cap_box = v_flex()
            .w_full()
            .max_w(px(680.))
            .min_w_0()
            .overflow_hidden()
            .px_4()
            .py_3()
            .gap_2()
            .rounded_2xl()
            .bg(pal.scrim_bg)
            .border_1()
            .border_color(pal.scrim_fg.opacity(0.12))
            .shadow_xl()
            .child(header);

        if state.captions().is_empty() {
            cap_box = cap_box.child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_sm()
                    .text_color(pal.scrim_muted)
                    .child("Listening for live speech transcripts…"),
            );
        } else {
            // Group consecutive utterances from the same speaker so their name & avatar appear
            // once per turn instead of repeating on every line, and fade older lines subtly.
            struct CaptionTurn<'a> {
                uid: &'a slackless_core::types::UserId,
                lines: Vec<(usize, &'a str, bool)>,
            }
            let total = state.captions().len();
            let mut groups: Vec<CaptionTurn<'_>> = Vec::new();
            for (
                idx,
                CaptionLine {
                    user: uid,
                    text,
                    partial: is_partial,
                    ..
                },
            ) in state.captions().iter().enumerate()
            {
                if let Some(prev) = groups.last_mut()
                    && prev.uid == uid
                {
                    prev.lines.push((idx, text.as_str(), *is_partial));
                } else {
                    groups.push(CaptionTurn {
                        uid,
                        lines: vec![(idx, text.as_str(), *is_partial)],
                    });
                }
            }

            let mut turns = v_flex().w_full().min_w_0().gap_2();
            for CaptionTurn { uid, lines } in groups {
                let who = self.fixtures.user_name(uid).to_owned();
                let mut turn_body = v_flex().flex_1().min_w_0().gap_0p5().child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(pal.scrim_muted)
                        .child(who),
                );
                for (idx, text, is_partial) in lines {
                    let is_latest = idx + 1 == total;
                    let text_color = if is_partial {
                        pal.scrim_muted
                    } else if is_latest {
                        pal.scrim_fg
                    } else {
                        pal.scrim_fg.opacity(0.72)
                    };
                    turn_body = turn_body.child(
                        div()
                            .w_full()
                            .min_w_0()
                            .text_sm()
                            .line_height(px(20.))
                            .text_color(text_color)
                            .when(is_partial, |el| el.italic())
                            .child(text.to_owned()),
                    );
                }
                turns = turns.child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .items_start()
                        .gap_2p5()
                        .child(
                            div()
                                .flex_none()
                                .mt(px(2.))
                                .child(crate::avatars::user_chip(&self.fixtures, uid, 22.)),
                        )
                        .child(turn_body),
                );
            }
            cap_box = cap_box.child(turns);
        }

        Some(
            h_flex()
                .absolute()
                .bottom(px(16.))
                .left_0()
                .right_0()
                .justify_center()
                .px_6()
                .child(cap_box),
        )
    }
}
