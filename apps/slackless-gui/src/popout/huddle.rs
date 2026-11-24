//! Huddle pop-out stage, video tiles, drawing, captions, drawers, and popovers.
//!
//! Styled to match official Slack's Huddle window across Light, Dark, and tinted workspace themes:
//! theme-matched top title bar & bottom control bar (left: diagnostics + invite; center: mic,
//! camera, share, hand, reactions, background/theme, thread, more; right: red Leave button),
//! luminous rounded-square participant cards on the stage, and a crisp theme-matched side drawer.

use gpui_kit::gpui::{Background, ClickEvent, Hsla, linear_color_stop, linear_gradient};

use super::*;

#[derive(Clone, Copy)]
struct HuddlePalette {
    /// Top titlebar and bottom control bar chrome.
    chrome_bg: Hsla,
    chrome_fg: Hsla,
    chrome_muted: Hsla,
    chrome_border: Hsla,
    /// Neutral control button fill & hover on the chrome bar.
    ctrl_bg: Hsla,
    ctrl_hover: Hsla,
    /// Emphasized active control button (e.g., unmuted mic / open thread toggle, like Slack).
    ctrl_active_bg: Hsla,
    ctrl_active_fg: Hsla,
    /// Stage backdrop gradient and participant card surface.
    stage_bg: Background,
    stage_solid: Hsla,
    tile_bg: Hsla,
    tile_border: Hsla,
    tile_pill_bg: Hsla,
    tile_pill_fg: Hsla,
    /// Floating scrim overlays on top of video / stage.
    scrim_bg: Hsla,
    scrim_fg: Hsla,
    scrim_muted: Hsla,
    /// Right-hand companion drawer (Thread / Notes / Diagnostics).
    drawer_bg: Hsla,
    drawer_fg: Hsla,
    drawer_muted: Hsla,
    drawer_border: Hsla,
    drawer_surface: Hsla,
    drawer_hover: Hsla,
    /// Popovers anchored above the bottom bar.
    popover_bg: Hsla,
    popover_fg: Hsla,
    popover_muted: Hsla,
    popover_border: Hsla,
    popover_hover: Hsla,
    /// Semantic accents.
    primary: Hsla,
    primary_hover: Hsla,
    primary_fg: Hsla,
    danger: Hsla,
    danger_hover: Hsla,
    danger_fg: Hsla,
    success: Hsla,
    warning: Hsla,
    warning_fg: Hsla,
    link: Hsla,
}

impl HuddlePalette {
    fn for_theme(bg_id: Option<&str>, cx: &App) -> Self {
        let theme = cx.theme();
        let dark_mode = theme.mode.is_dark();
        let chrome_dark = crate::theme::sidebar_is_dark(cx);

        let (
            chrome_bg,
            chrome_fg,
            chrome_muted,
            chrome_border,
            ctrl_bg,
            ctrl_hover,
            ctrl_active_bg,
            ctrl_active_fg,
        ) = if chrome_dark {
            let fg = theme.sidebar_foreground;
            (
                theme.title_bar,
                fg,
                fg.opacity(0.68),
                theme.sidebar_border,
                fg.opacity(0.12),
                fg.opacity(0.22),
                fg.opacity(0.92),
                theme.title_bar,
            )
        } else {
            // Light default Slack huddle chrome: cool pale sky tint matching `theme.title_bar`
            // blended gently toward soft mist, crisp dark icons, dark charcoal active pills!
            let bar = theme.title_bar.blend(theme.background.opacity(0.55));
            (
                bar,
                theme.foreground,
                theme.muted_foreground,
                theme.border,
                theme.foreground.opacity(0.08),
                theme.foreground.opacity(0.15),
                Hsla::from(rgb(0x2B2D31)),
                Hsla::from(rgb(0xFFFFFF)),
            )
        };

        let (stage_a, stage_b, stage_solid, tile_bg, stage_is_dark) = match bg_id {
            Some("meadow") => (
                Hsla::from(rgb(0xBCE3CC)),
                Hsla::from(rgb(0xDCF2E4)),
                Hsla::from(rgb(0xCBEAD7)),
                Hsla::from(rgb(0xFFFFFF)),
                false,
            ),
            Some("sky") => (
                Hsla::from(rgb(0xC5E5FA)),
                Hsla::from(rgb(0xE8F5FE)),
                Hsla::from(rgb(0xD6EEFC)),
                Hsla::from(rgb(0xFFFFFF)),
                false,
            ),
            Some("midnight") => (
                Hsla::from(rgb(0x0E1629)),
                Hsla::from(rgb(0x1B294A)),
                Hsla::from(rgb(0x10172A)),
                Hsla::from(rgb(0x1B2440)),
                true,
            ),
            Some("sunset") => (
                Hsla::from(rgb(0x2B1818)),
                Hsla::from(rgb(0x472329)),
                Hsla::from(rgb(0x2B1818)),
                Hsla::from(rgb(0x3B2222)),
                true,
            ),
            Some("forest") => (
                Hsla::from(rgb(0x10241C)),
                Hsla::from(rgb(0x1C3D30)),
                Hsla::from(rgb(0x11221B)),
                Hsla::from(rgb(0x1B3328)),
                true,
            ),
            Some("ocean") => (
                Hsla::from(rgb(0x0D2230)),
                Hsla::from(rgb(0x183A52)),
                Hsla::from(rgb(0x0F212E)),
                Hsla::from(rgb(0x183244)),
                true,
            ),
            Some("plum") => (
                Hsla::from(rgb(0x24142B)),
                Hsla::from(rgb(0x3D1D47)),
                Hsla::from(rgb(0x24142B)),
                Hsla::from(rgb(0x351F3E)),
                true,
            ),
            _ if dark_mode => {
                let top = theme.background.blend(theme.title_bar.opacity(0.35));
                let bot = theme.background;
                (top, bot, bot, theme.muted, true)
            }
            _ => {
                // Light default: serene pastel sage wash reminiscent of official Slack's
                // sunlit botanical huddle canvas, bright and airy instead of black.
                (
                    Hsla::from(rgb(0xC2E6CE)),
                    Hsla::from(rgb(0xE1F4E8)),
                    Hsla::from(rgb(0xD2EEDC)),
                    Hsla::from(rgb(0xFFFFFF)),
                    false,
                )
            }
        };

        let stage_bg = linear_gradient(
            145.,
            linear_color_stop(stage_a, 0.),
            linear_color_stop(stage_b, 1.),
        );
        let (tile_border, tile_pill_bg, tile_pill_fg) = if stage_is_dark {
            (
                Hsla::from(rgba(0xFFFFFF22)),
                Hsla::from(rgba(0x0E1014CC)),
                Hsla::from(rgb(0xFFFFFF)),
            )
        } else {
            (
                Hsla::from(rgba(0xFFFFFFEE)),
                Hsla::from(rgba(0xFFFFFFE6)),
                Hsla::from(rgb(0x1D1C1D)),
            )
        };

        Self {
            chrome_bg,
            chrome_fg,
            chrome_muted,
            chrome_border,
            ctrl_bg,
            ctrl_hover,
            ctrl_active_bg,
            ctrl_active_fg,
            stage_bg,
            stage_solid,
            tile_bg,
            tile_border,
            tile_pill_bg,
            tile_pill_fg,
            scrim_bg: Hsla::from(rgba(0x14171CE2)),
            scrim_fg: Hsla::from(rgb(0xFFFFFF)),
            scrim_muted: Hsla::from(rgb(0xD1D2D3)),
            drawer_bg: theme.background,
            drawer_fg: theme.foreground,
            drawer_muted: theme.muted_foreground,
            drawer_border: theme.border,
            drawer_surface: theme.muted,
            drawer_hover: theme.accent,
            popover_bg: theme.popover,
            popover_fg: theme.popover_foreground,
            popover_muted: theme.muted_foreground,
            popover_border: theme.border,
            popover_hover: theme.accent,
            primary: theme.primary,
            primary_hover: theme.primary_hover,
            primary_fg: theme.primary_foreground,
            danger: theme.danger,
            danger_hover: theme.danger_hover,
            danger_fg: theme.danger_foreground,
            success: theme.success,
            warning: theme.warning,
            warning_fg: theme.warning_foreground,
            link: theme.link,
        }
    }
}

fn parse_ts_secs(ts: &str) -> Option<f64> {
    ts.trim().parse::<f64>().ok()
}

impl PopoutWindow {
    fn huddle_dock_entity(&self) -> Option<Entity<crate::huddle_dock::HuddleDock>> {
        match &self.kind {
            PopoutKind::Huddle { source, .. } => source.upgrade(),
            _ => None,
        }
    }

    fn close_huddle_popovers(&mut self) {
        self.huddle_invite_open = false;
        self.huddle_devices_open = false;
        self.huddle_camera_open = false;
        self.huddle_share_open = false;
        self.huddle_reactions_open = false;
        self.huddle_theme_open = false;
        self.huddle_more_open = false;
        self.huddle_music_open = false;
    }

    fn close_share_modal(&mut self, cx: &mut Context<Self>) {
        self.huddle_share_open = false;
        for (_, img) in self.huddle_screen_thumbs.drain() {
            cx.drop_image(img, None);
        }
    }

    fn open_audio_video_preferences(&mut self, cx: &mut Context<Self>) {
        self.close_huddle_popovers();
        if let Some(dock) = self.huddle_dock_entity() {
            dock.update(cx, |d, cx| d.open_audio_settings(cx));
        }
        cx.notify();
    }

    fn refresh_share_thumbnails(&self, monitors_only: bool, cx: &mut Context<Self>) {
        use slackless_core::huddle::capture_screen_thumbnails;
        cx.spawn(async move |win, cx| {
            let frames = cx
                .background_executor()
                .spawn(async move { capture_screen_thumbnails(monitors_only, 360, 224) })
                .await;
            let _ = win.update(cx, |win: &mut PopoutWindow, cx| {
                if !win.huddle_share_open {
                    return;
                }
                for (id, frame) in frames {
                    if let Some(img) = crate::huddle_dock::frame_to_render_image(&frame) {
                        if let Some(old) = win.huddle_screen_thumbs.insert(id, img) {
                            cx.drop_image(old, None);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_selected_screen_share(&mut self, source_id: String, cx: &mut Context<Self>) {
        let share_audio = self.huddle_share_audio;
        (self.send)(Command::SetHuddleScreenShare {
            source_id: Some(source_id.clone()),
            share_audio,
        });
        if self.fixtures.sample {
            if let Some(dock) = self.huddle_dock_entity() {
                dock.update(cx, |d, cx| d.set_sharing_local(Some(source_id), cx));
            }
        }
        self.close_share_modal(cx);
        cx.notify();
    }

    pub(super) fn render_huddle_window(
        &self,
        on_jump: JumpHandler,
        on_open_canvas: CanvasOpenHandler,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        use gpui_kit::component::Icon;
        use slackless_core::huddle::{HuddlePhase, VideoTileKey};

        let bg_id = self
            .huddle_state
            .as_ref()
            .and_then(|s| s.session.background_id.as_deref());
        let pal = HuddlePalette::for_theme(bg_id, cx);

        let Some(state) = self.huddle_state.clone() else {
            return v_flex()
                .id("popout-huddle-ended")
                .size_full()
                .bg(pal.drawer_bg)
                .text_color(pal.drawer_fg)
                .child(
                    TitleBar::new()
                        .h(px(38.))
                        .bg(pal.chrome_bg)
                        .border_b_1()
                        .border_color(pal.chrome_border)
                        .child(
                            h_flex()
                                .flex_1()
                                .h_full()
                                .items_center()
                                .justify_center()
                                .gap_1p5()
                                .child(
                                    Icon::new(IconName::Headphones)
                                        .size(px(13.))
                                        .text_color(pal.chrome_muted),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(pal.chrome_fg)
                                        .child(self.title.clone()),
                                ),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::Headphones)
                                .size(px(32.))
                                .text_color(pal.drawer_muted),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(pal.drawer_muted)
                                .child("This huddle has ended."),
                        ),
                );
        };

        let snap = state.session.clone();
        let channel = snap.channel.clone();
        let mut people = state.participants.clone();
        if !people.contains(&self.fixtures.self_user) {
            people.push(self.fixtures.self_user.clone());
        }
        let (status_color, status_text) = match snap.phase {
            HuddlePhase::Joining | HuddlePhase::Connecting => {
                (pal.warning, "Connecting…".to_owned())
            }
            HuddlePhase::Connected => (
                pal.success,
                crate::huddle_dock::format_elapsed(state.elapsed_secs),
            ),
            HuddlePhase::Reconnecting => (pal.warning, "Reconnecting…".to_owned()),
            HuddlePhase::Leaving => (pal.chrome_muted, "Leaving…".to_owned()),
            HuddlePhase::Failed => (pal.danger, "Failed".to_owned()),
            HuddlePhase::Idle => (pal.chrome_muted, "Ended".to_owned()),
        };

        let header = self.render_huddle_titlebar(&snap, people.len(), &pal, cx);

        let has_screen_share =
            self.huddle_video
                .as_ref()
                .and_then(WeakEntity::upgrade)
                .is_some_and(|vs| {
                    vs.read(cx).frames().keys().any(|k| {
                        matches!(k, VideoTileKey::RemoteScreen(_) | VideoTileKey::LocalScreen)
                    })
                });

        let mut stage = v_flex()
            .id("popout-huddle-stage-container")
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(pal.stage_bg)
            .px_5()
            .py_4()
            .gap_3()
            .when(self.huddle_music_open, |el| {
                el.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.huddle_music_open = false;
                        cx.notify();
                    }),
                )
            });

        // Top-left compact floating status + topic pill (matches official Slack's top-left chip).
        stage = stage.child(
            h_flex()
                .absolute()
                .left(px(14.))
                .top(px(12.))
                .px_2p5()
                .py_1()
                .gap_2()
                .items_center()
                .rounded_lg()
                .bg(pal.tile_pill_bg)
                .border_1()
                .border_color(pal.drawer_border)
                .shadow_sm()
                .child(div().size(px(7.)).rounded_full().bg(status_color))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(pal.tile_pill_fg)
                        .child(status_text),
                )
                .child(self.render_huddle_topic_bar(snap.topic.as_deref(), &pal, cx)),
        );

        // Top-right floating Lounge Music player pill + song-selector dropdown (matches Official
        // Slack's solo/lounge music banner that lets you switch between all 10 Slack tracks or stop).
        if let Some(active_song_key) = snap.jukebox_song.clone() {
            stage = stage.child(self.render_huddle_music_pill(&active_song_key, &pal, cx));
        }

        if snap.audio_only {
            let send_resume = self.send.clone();
            stage = stage.child(
                h_flex()
                    .mt(px(36.))
                    .px_3()
                    .py_1p5()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .rounded_xl()
                    .bg(pal.popover_bg)
                    .border_1()
                    .border_color(pal.warning.opacity(0.65))
                    .shadow_sm()
                    .text_xs()
                    .text_color(pal.popover_fg)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Headphones)
                                    .size(px(13.))
                                    .text_color(pal.warning),
                            )
                            .child("Audio-only mode — video paused to save bandwidth"),
                    )
                    .child(
                        div()
                            .id("popout-huddle-resume-video")
                            .cursor_pointer()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(pal.drawer_surface)
                            .hover(move |el| el.bg(pal.drawer_hover))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Resume video")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                (send_resume)(Command::SetHuddleAudioOnly(false));
                                if this.fixtures.sample {
                                    if let Some(dock) = this.huddle_dock_entity() {
                                        dock.update(cx, |d, cx| d.set_audio_only_local(false, cx));
                                    }
                                }
                            })),
                    ),
            );
        }

        if let Some(err) = snap.error.clone() {
            stage = stage.child(
                h_flex()
                    .mt(px(36.))
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .rounded_xl()
                    .bg(pal.popover_bg)
                    .border_1()
                    .border_color(pal.danger.opacity(0.65))
                    .shadow_sm()
                    .text_xs()
                    .text_color(pal.popover_fg)
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size(px(14.))
                            .text_color(pal.danger),
                    )
                    .child(err),
            );
        }

        if has_screen_share {
            if let Some(screen_stage) = self.render_huddle_stage(&state, &pal, cx) {
                stage = stage.child(screen_stage);
            }
            stage = stage.child(self.render_huddle_filmstrip(&state, &people, &pal, cx));
        } else {
            let has_bottom_overlay = state.session.captions_enabled
                || !state.captions.is_empty()
                || !state.reactions.is_empty();
            stage = stage.child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .when(has_bottom_overlay, |el| el.pb(px(48.)))
                    .items_center()
                    .justify_center()
                    .child(self.render_huddle_tiles(&state, &people, &pal, cx)),
            );
        }

        // Floating effect burst banner (top-center, never shifts stage layout)
        if let Some((uid, effect_name, _)) = &state.active_effect {
            let who = self.fixtures.user_name(uid).to_owned();
            let (glyph, label) = effect_label(effect_name);
            stage = stage.child(
                h_flex()
                    .absolute()
                    .top(px(12.))
                    .left_0()
                    .right_0()
                    .justify_center()
                    .child(
                        h_flex()
                            .px_4()
                            .py_2()
                            .gap_2()
                            .items_center()
                            .rounded_full()
                            .bg(pal.popover_bg)
                            .border_1()
                            .border_color(pal.success.opacity(0.65))
                            .shadow_xl()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(pal.popover_fg)
                            .child(format!(
                                "{glyph} {glyph} {who} triggered {label}! {glyph} {glyph}"
                            )),
                    ),
            );
        }

        // Floating stacking reactions (bottom-left above dock)
        if let Some(reactions_el) = self.render_huddle_floating_reactions(&state, &pal) {
            stage = stage.child(reactions_el);
        }

        // Floating live captions subtitle overlay (bottom-center above dock)
        if let Some(captions_el) = self.render_huddle_floating_captions(&state, &pal) {
            stage = stage.child(captions_el);
        }

        let active_popover = self.render_huddle_active_popover(&snap, &channel, &people, &pal, cx);

        let body: gpui_kit::gpui::AnyElement = if let Some(drawer) = self.huddle_drawer {
            let drawer_el = self.render_huddle_drawer(
                drawer,
                &state,
                &pal,
                on_jump.clone(),
                on_open_canvas.clone(),
                cx,
            );
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(
                    h_resizable("huddle-popout-drawer")
                        .with_handle_appearance(Rc::new(
                            |_: &gpui_kit::base::ResizeHandleContext,
                             _: &mut Window,
                             _: &mut App| {
                                Some(div().into_any_element())
                            },
                        ))
                        .child(resizable_panel().child(stage))
                        .child(
                            resizable_panel()
                                .size(px(340.))
                                .size_range(px(260.)..px(720.))
                                .fixed()
                                .child(drawer_el),
                        ),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(stage)
                .into_any_element()
        };

        v_flex()
            .id("popout-huddle-root")
            .relative()
            .size_full()
            .bg(pal.stage_solid)
            .text_color(pal.drawer_fg)
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape"
                        && (this.huddle_topic_editing.take().is_some()
                            || this.huddle_share_open
                            || this.huddle_camera_open
                            || this.huddle_reactions_open
                            || this.huddle_theme_open
                            || this.huddle_more_open
                            || this.huddle_music_open
                            || this.huddle_invite_open
                            || this.huddle_devices_open
                            || this.huddle_drawer.take().is_some())
                    {
                        this.close_share_modal(cx);
                        this.close_huddle_popovers();
                        cx.notify();
                    }
                }),
            )
            .child(header)
            .child(body)
            .child(self.render_huddle_controls(&snap, status_color, &pal, cx))
            .when_some(active_popover, |root, popover| {
                // Full-window transparent click-away backdrop so clicking anywhere outside any
                // huddle dropdown (audio, video, reactions, theme, invite, more) dismisses it.
                root.child(
                    div()
                        .id("popout-huddle-popover-backdrop")
                        .absolute()
                        .inset_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.close_huddle_popovers();
                                cx.notify();
                            }),
                        )
                        .child(
                            h_flex()
                                .absolute()
                                .bottom(px(68.))
                                .left_0()
                                .right_0()
                                .justify_center()
                                .px_4()
                                .child(
                                    div()
                                        .id("popout-huddle-popover-surface")
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .child(popover),
                                ),
                        ),
                )
            })
            .when(self.huddle_share_open, |root| {
                root.child(self.render_huddle_share_modal(
                    snap.share_source_id.as_deref(),
                    &pal,
                    cx,
                ))
            })
    }

    fn render_huddle_drawer(
        &self,
        drawer: HuddleDrawer,
        state: &crate::huddle_dock::HuddleViewState,
        pal: &HuddlePalette,
        on_jump: JumpHandler,
        on_open_canvas: CanvasOpenHandler,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let content: gpui_kit::gpui::AnyElement = match drawer {
            HuddleDrawer::Thread => self
                .render_huddle_thread_drawer(&state.session, pal, on_jump, cx)
                .into_any_element(),
            HuddleDrawer::Notes => self
                .render_huddle_notes_drawer(&state.session, pal, on_open_canvas, cx)
                .into_any_element(),
            HuddleDrawer::Diagnostics => self
                .render_huddle_diagnostics_drawer(state, pal, cx)
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .min_w_0()
            .bg(pal.drawer_bg)
            .border_l_1()
            .border_color(pal.drawer_border)
            .text_color(pal.drawer_fg)
            .child(content)
    }

    pub(super) fn resolve_huddle_thread_root(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
    ) -> Option<Ts> {
        if let Some(root) = snap
            .thread_root_ts
            .as_ref()
            .filter(|ts| !ts.as_str().trim().is_empty())
        {
            return Some(root.clone());
        }
        // If `rooms.join` didn't include `thread_root_ts` yet, look up the active live
        // `huddle_thread` trail message for this channel in SQLite so we never post to the
        // parent channel or leak unrelated channel history.
        let (Some(store), Some(team)) = (&self.store, &self.team) else {
            return None;
        };
        let guard = store.lock().ok()?;
        let rows = guard.channel_history(team, &snap.channel, None, 20).ok()?;
        rows.into_iter().rev().find_map(|row| {
            if row.deleted
                || row.message.subtype != Some(slackless_core::types::MessageSubtype::HuddleThread)
            {
                return None;
            }
            let has_ended = row
                .message
                .extra
                .get("room")
                .and_then(|r| r.get("has_ended"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let has_participants = row
                .message
                .extra
                .get("room")
                .and_then(|r| r.get("participants"))
                .and_then(|v| v.as_array())
                .is_some_and(|arr| !arr.is_empty());
            (!has_ended && has_participants).then_some(row.message.ts)
        })
    }

    pub(super) fn huddle_thread_messages(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
    ) -> Vec<Message> {
        let root_ts = self.resolve_huddle_thread_root(snap);
        let mut out: Vec<Message> = match root_ts.as_ref() {
            Some(root) => {
                if let (Some(store), Some(team)) = (&self.store, &self.team) {
                    store
                        .lock()
                        .ok()
                        .and_then(|guard| guard.thread_replies(team, &snap.channel, root).ok())
                        .map(|list| {
                            list.into_iter()
                                .filter(|r| {
                                    !r.deleted
                                        && r.message.ts != *root
                                        && r.message.subtype
                                            != Some(
                                                slackless_core::types::MessageSubtype::HuddleThread,
                                            )
                                })
                                .map(|r| r.message)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                } else {
                    self.fixtures
                        .threads
                        .get(&(snap.channel.clone(), root.clone()))
                        .map(|list| {
                            list.iter()
                                .filter(|m| {
                                    m.ts != *root
                                        && m.subtype
                                            != Some(
                                                slackless_core::types::MessageSubtype::HuddleThread,
                                            )
                                })
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default()
                }
            }
            // Brand-new huddle with no thread root yet: always start empty (never pull parent
            // channel chat!).
            None => Vec::new(),
        };
        let mut matched_store = vec![false; out.len()];
        for local in &self.huddle_thread_local {
            let local_preview = self.fixtures.preview(local);
            let local_secs = parse_ts_secs(local.ts.as_str());
            let found = out.iter().enumerate().find(|(ix, m)| {
                if matched_store[*ix] {
                    return false;
                }
                if m.ts == local.ts {
                    return true;
                }
                if m.user != local.user {
                    return false;
                }
                let exact = !local.text.trim().is_empty() && m.text.trim() == local.text.trim();
                let same_preview = !local_preview.trim().is_empty()
                    && self.fixtures.preview(m).trim() == local_preview.trim();
                let close_in_time = match (parse_ts_secs(m.ts.as_str()), local_secs) {
                    (Some(ms), Some(ls)) => (ms - ls) >= -10.0 && (ms - ls) <= 30.0,
                    _ => false,
                };
                exact || same_preview || close_in_time
            });
            if let Some((ix, _)) = found {
                matched_store[ix] = true;
            } else {
                out.push(local.clone());
            }
        }
        out
    }

    /// Drop any optimistic local huddle replies that have already been echoed into the store.
    pub(super) fn prune_echoed_huddle_locals(
        &mut self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
    ) {
        if self.huddle_thread_local.is_empty() {
            return;
        }
        let Some(root) = self.resolve_huddle_thread_root(snap) else {
            return;
        };
        let (Some(store), Some(team)) = (&self.store, &self.team) else {
            return;
        };
        let store_msgs: Vec<Message> = store
            .lock()
            .ok()
            .and_then(|guard| guard.thread_replies(team, &snap.channel, &root).ok())
            .map(|list| {
                list.into_iter()
                    .filter(|r| {
                        !r.deleted
                            && r.message.ts != root
                            && r.message.subtype
                                != Some(slackless_core::types::MessageSubtype::HuddleThread)
                    })
                    .map(|r| r.message)
                    .collect()
            })
            .unwrap_or_default();
        if store_msgs.is_empty() {
            return;
        }
        let mut matched = vec![false; store_msgs.len()];
        let fixtures = &self.fixtures;
        self.huddle_thread_local.retain(|local| {
            let local_preview = fixtures.preview(local);
            let local_secs = parse_ts_secs(local.ts.as_str());
            let found = store_msgs.iter().enumerate().find(|(ix, m)| {
                if matched[*ix] {
                    return false;
                }
                if m.ts == local.ts {
                    return true;
                }
                if m.user != local.user {
                    return false;
                }
                let exact = !local.text.trim().is_empty() && m.text.trim() == local.text.trim();
                let same_preview = !local_preview.trim().is_empty()
                    && fixtures.preview(m).trim() == local_preview.trim();
                let close_in_time = match (parse_ts_secs(m.ts.as_str()), local_secs) {
                    (Some(ms), Some(ls)) => (ms - ls) >= -10.0 && (ms - ls) <= 30.0,
                    _ => false,
                };
                exact || same_preview || close_in_time
            });
            if let Some((ix, _)) = found {
                matched[ix] = true;
                false
            } else {
                true
            }
        });
    }

    fn render_huddle_thread_drawer(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        pal: &HuddlePalette,
        on_jump: JumpHandler,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;

        let channel = snap.channel.clone();
        let channel_label = self
            .fixtures
            .conversation(&channel)
            .map(|c| self.fixtures.conversation_label(c))
            .unwrap_or_else(|| self.title.clone());
        let thread_ts = self.resolve_huddle_thread_root(snap);
        let jump_ch = channel.clone();
        let jump_ts = thread_ts.clone();
        let messages = self.huddle_thread_messages(snap);
        let hover = pal.drawer_hover;

        let jump_for_links = on_jump.clone();
        let links: LinkHandler = Rc::new(move |target, _, cx| match target {
            LinkTarget::Url(url) => cx.open_url(url.as_ref()),
            LinkTarget::Channel(ch) => jump_for_links(ch.clone(), None, false, cx),
            LinkTarget::Thread { channel, thread_ts } => {
                jump_for_links(channel.clone(), Some(thread_ts.clone()), true, cx)
            }
            LinkTarget::Profile(_) => {}
        });

        let view = cx.entity();
        let mut rows: Vec<gpui_kit::gpui::AnyElement> = Vec::with_capacity(messages.len());
        let mut prev_msg: Option<&Message> = None;
        for (ix, msg) in messages.iter().enumerate() {
            let msg_group = SharedString::from(format!("huddle-thread-msg-{ix}"));
            let grouped = prev_msg
                .filter(|prev| same_group(prev, msg))
                .map(|_| msg_group.clone());
            let view_for_action = view.clone();
            let ch_for_action = channel.clone();
            let msg_for_action = msg.clone();
            let handler: ActionHandler = Rc::new(move |action, window, cx| {
                let ch = ch_for_action.clone();
                let m = msg_for_action.clone();
                view_for_action.update(cx, |this, cx| {
                    this.handle_message_action(ch, m, action, window, cx);
                });
            });
            let mut display = msg.clone();
            let reactions = std::mem::take(&mut display.reactions);
            let key = format!("huddle-thread-row-{}", msg.ts.as_str());
            let chips =
                actions::reaction_chips(&key, &reactions, &self.fixtures, Some(&handler), cx)
                    .map(|c| c.pl(px(48.)));
            let row_body = message_row_linked(
                &self.fixtures,
                &display,
                Some(&handler),
                Some(&links),
                grouped,
                cx,
            );
            rows.push(
                v_flex()
                    .id(SharedString::from(key))
                    .group(msg_group)
                    .px_3()
                    .py_0p5()
                    .child(row_body)
                    .children(chips)
                    .into_any_element(),
            );
            prev_msg = Some(msg);
        }

        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .px_4()
                    .h(px(46.))
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(pal.drawer_border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(pal.drawer_fg)
                            .child("Thread"),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .id("huddle-drawer-thread-pop")
                                    .size(px(26.))
                                    .rounded_md()
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .hover(move |el| el.bg(hover))
                                    .child(
                                        Icon::new(IconName::EllipsisVertical)
                                            .size(px(14.))
                                            .text_color(pal.drawer_muted),
                                    )
                                    .on_click(move |_, _, cx| {
                                        on_jump(
                                            jump_ch.clone(),
                                            jump_ts.clone(),
                                            jump_ts.is_some(),
                                            cx,
                                        );
                                    }),
                            )
                            .child(
                                div()
                                    .id("huddle-drawer-close")
                                    .size(px(26.))
                                    .rounded_md()
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .hover(move |el| el.bg(hover))
                                    .child(
                                        Icon::new(IconName::X)
                                            .size(px(14.))
                                            .text_color(pal.drawer_muted),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.huddle_drawer = None;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .id("huddle-drawer-thread-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py_3()
                    .gap_1()
                    .when(rows.is_empty(), |col| {
                        col.child(
                            v_flex()
                                .px_4()
                                .gap_1p5()
                                .py_2()
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            Icon::new(IconName::MessageSquare)
                                                .size(px(14.))
                                                .text_color(pal.drawer_fg),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(pal.drawer_fg)
                                                .child("Every huddle has a thread"),
                                        ),
                                )
                                .child(div().text_xs().text_color(pal.drawer_muted).child(
                                    format!(
                                        "Send messages, files, and links to everyone in the huddle. They're saved as a thread in {channel_label}, so you can access it even after the huddle is done."
                                    ),
                                )),
                        )
                    })
                    .children(rows),
            )
            .child(
                div()
                    .flex_none()
                    .py_2()
                    .border_t_1()
                    .border_color(pal.drawer_border)
                    .child(self.composer.clone()),
            )
    }

    fn render_huddle_notes_drawer(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        pal: &HuddlePalette,
        on_open_canvas: CanvasOpenHandler,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;

        let send_notes = self.send.clone();
        let notes_ch = snap.channel.clone();
        let canvas_id = snap.canvas_file_id.clone();
        let topic_title = snap
            .topic
            .clone()
            .map(|t| format!("Huddle notes: {t}"))
            .unwrap_or_else(|| "Huddle notes".to_owned());
        let hover = pal.drawer_hover;
        let p_hover = pal.primary_hover;

        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .px_4()
                    .h(px(46.))
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(pal.drawer_border)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::FileText)
                                    .size(px(14.))
                                    .text_color(pal.drawer_muted),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(pal.drawer_fg)
                                    .child("Huddle notes"),
                            ),
                    )
                    .child(
                        div()
                            .id("huddle-drawer-notes-close")
                            .size(px(26.))
                            .rounded_md()
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(move |el| el.bg(hover))
                            .child(
                                Icon::new(IconName::X)
                                    .size(px(14.))
                                    .text_color(pal.drawer_muted),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.huddle_drawer = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .p_4()
                    .gap_3()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(pal.drawer_fg)
                            .child(topic_title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(pal.drawer_muted)
                            .child(if canvas_id.is_some() {
                                "A collaborative Slack canvas is attached to this huddle. Open it in the main window to edit sections and action items live."
                            } else {
                                "Start a shared Slack canvas for this huddle to capture notes, links, and follow-up tasks."
                            }),
                    )
                    .child(if let Some(fid) = canvas_id {
                        h_flex()
                            .id("huddle-drawer-open-canvas")
                            .cursor_pointer()
                            .px_3()
                            .py_2()
                            .gap_2()
                            .items_center()
                            .justify_center()
                            .rounded_lg()
                            .bg(pal.primary)
                            .hover(move |el| el.bg(p_hover))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(pal.primary_fg)
                            .child(
                                Icon::new(IconName::FileText)
                                    .size(px(14.))
                                    .text_color(pal.primary_fg),
                            )
                            .child("Open huddle canvas")
                            .on_click(move |_, _, cx| on_open_canvas(fid.clone(), cx))
                    } else {
                        h_flex()
                            .id("huddle-drawer-create-canvas")
                            .cursor_pointer()
                            .px_3()
                            .py_2()
                            .gap_2()
                            .items_center()
                            .justify_center()
                            .rounded_lg()
                            .bg(pal.primary)
                            .hover(move |el| el.bg(p_hover))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(pal.primary_fg)
                            .child(
                                Icon::new(IconName::Plus)
                                    .size(px(14.))
                                    .text_color(pal.primary_fg),
                            )
                            .child("Create huddle notes canvas")
                            .on_click(move |_, window, cx| {
                                (send_notes)(Command::CreateCanvas {
                                    title: topic_title.clone(),
                                    markdown: "# Huddle notes\n\n- ".to_owned(),
                                    channel: Some(notes_ch.clone()),
                                });
                                actions::notify(
                                    "Creating huddle notes",
                                    "Opening a shared canvas for this huddle.",
                                    window,
                                    cx,
                                );
                            })
                    }),
            )
    }

    /// Troubleshooting, Network Stats HUD & Self-Test drawer (`F-COLLAB-074`, `075`, `076`).
    fn render_huddle_diagnostics_drawer(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        use gpui_kit::component::Icon;
        use slackless_core::huddle::{
            HuddleChime, HuddlePhase, play_huddle_chime, play_speaker_test_tone,
        };

        let snap = &state.session;
        let (phase_color, phase_label) = match snap.phase {
            HuddlePhase::Connected => (pal.success, "Connected (UDP TURN Relay)"),
            HuddlePhase::Joining | HuddlePhase::Connecting => (pal.warning, "Connecting…"),
            HuddlePhase::Reconnecting => (pal.warning, "Reconnecting…"),
            HuddlePhase::Failed if snap.udp_blocked => (pal.danger, "UDP Blocked — Check Firewall"),
            HuddlePhase::Failed => (pal.danger, "Connection Failed"),
            HuddlePhase::Leaving => (pal.drawer_muted, "Leaving…"),
            HuddlePhase::Idle => (pal.drawer_muted, "Idle"),
        };
        let video_codec = snap
            .active_video_codec
            .as_deref()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| {
                if let Some(backend) = snap
                    .perf
                    .tx_backend
                    .as_deref()
                    .filter(|b| !b.is_empty() && *b != "none")
                {
                    if backend.contains("(HW)") {
                        "video/H264 (VideoToolbox HW)".to_owned()
                    } else {
                        "video/H264 (OpenH264 SW)".to_owned()
                    }
                } else if snap.video || snap.sharing {
                    "video/H264".to_owned()
                } else {
                    "Idle (AV1 / H.264 ready)".to_owned()
                }
            });
        let tx_kb = snap.bytes_sent as f64 / 1024.0;
        let rx_kb = snap.bytes_received as f64 / 1024.0;
        let rms_pct = (state.levels.local_rms.clamp(0.0, 1.0) * 100.0) as u32;
        let audio_only = snap.audio_only;
        let send_ao = self.send.clone();
        let test_out = snap.output_device.clone();
        let chime_out = snap.output_device.clone();
        let hover = pal.drawer_hover;

        let stat_row = |label: &'static str, value: String| {
            h_flex()
                .w_full()
                .min_w_0()
                .gap_3()
                .justify_between()
                .items_center()
                .text_xs()
                .child(div().flex_none().text_color(pal.drawer_muted).child(label))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(pal.drawer_fg)
                        .child(value),
                )
        };
        let dsp_pill = |label: &'static str, on: bool| {
            div()
                .px_2()
                .py_0p5()
                .rounded_full()
                .bg(if on {
                    pal.success.opacity(0.18)
                } else {
                    pal.drawer_border.opacity(0.5)
                })
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if on { pal.success } else { pal.drawer_muted })
                .child(label)
        };

        v_flex()
            .id("huddle-drawer-diag-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                h_flex()
                    .px_4()
                    .h(px(46.))
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(pal.drawer_border)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Activity)
                                    .size(px(14.))
                                    .text_color(phase_color),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(pal.drawer_fg)
                                    .child("Connection & diagnostics"),
                            ),
                    )
                    .child(
                        div()
                            .id("huddle-drawer-diag-close")
                            .size(px(26.))
                            .rounded_md()
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(move |el| el.bg(hover))
                            .child(
                                Icon::new(IconName::X)
                                    .size(px(14.))
                                    .text_color(pal.drawer_muted),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.huddle_drawer = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .p_3p5()
                    .gap_3()
                    .child(
                        v_flex()
                            .p_3()
                            .gap_1p5()
                            .rounded_xl()
                            .bg(pal.drawer_surface)
                            .border_1()
                            .border_color(pal.drawer_border)
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .pb_1()
                                    .child(div().size(px(8.)).rounded_full().bg(phase_color))
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(pal.drawer_fg)
                                            .child(phase_label),
                                    ),
                            )
                            .child(stat_row("Audio codec", "Opus 48 kHz Stereo".to_owned()))
                            .child(stat_row("Video codec", video_codec))
                            .child(stat_row(
                                "Outbound (TX)",
                                format!("{} pkts ({tx_kb:.1} KB)", snap.packets_sent),
                            ))
                            .child(stat_row(
                                "Inbound (RX)",
                                format!("{} pkts ({rx_kb:.1} KB)", snap.packets_received),
                            ))
                            .child(stat_row(
                                "Signaling keepalive",
                                format!("{} pongs", snap.pongs_sent),
                            ))
                            .child(stat_row(
                                "Screen audio",
                                snap.screen_audio_device.clone().unwrap_or_else(|| {
                                    if slackless_core::huddle::system_audio_source_label().is_some()
                                    {
                                        if snap.sharing {
                                            "Off (unchecked)".to_owned()
                                        } else {
                                            "Ready (macOS Native System Audio)".to_owned()
                                        }
                                    } else {
                                        "None (install BlackHole 2ch)".to_owned()
                                    }
                                }),
                            )),
                    )
                    .child(
                        v_flex()
                            .p_3()
                            .gap_1p5()
                            .rounded_xl()
                            .bg(pal.drawer_surface)
                            .border_1()
                            .border_color(pal.drawer_border)
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(pal.drawer_fg)
                                    .child("Performance & Video Pipeline"),
                            )
                            .child(stat_row(
                                "CPU / Memory",
                                format!(
                                    "{}.{}%  ·  {} MB RSS",
                                    snap.perf.cpu_tenths_pct / 10,
                                    snap.perf.cpu_tenths_pct % 10,
                                    snap.perf.rss_mb
                                ),
                            ))
                            .when_some(snap.perf.tx_backend.clone(), |col, backend| {
                                let res = snap
                                    .perf
                                    .tx_resolution
                                    .clone()
                                    .unwrap_or_else(|| "—".to_owned());
                                col.child(stat_row("TX encoder", backend)).child(stat_row(
                                    "TX stream",
                                    format!(
                                        "{res} @ {}.{} fps · {:.1} ms · {} kbps",
                                        snap.perf.tx_fps_tenths / 10,
                                        snap.perf.tx_fps_tenths % 10,
                                        snap.perf.tx_encode_us as f64 / 1000.0,
                                        snap.perf.tx_kbps
                                    ),
                                ))
                            })
                            .when_some(snap.perf.rx_backend.clone(), |col, backend| {
                                let res = snap
                                    .perf
                                    .rx_resolution
                                    .clone()
                                    .unwrap_or_else(|| "—".to_owned());
                                col.child(stat_row("RX decoder", backend)).child(stat_row(
                                    "RX stream",
                                    format!(
                                        "{res} @ {}.{} fps · {:.1} ms · {} kbps",
                                        snap.perf.rx_fps_tenths / 10,
                                        snap.perf.rx_fps_tenths % 10,
                                        snap.perf.rx_decode_us as f64 / 1000.0,
                                        snap.perf.rx_kbps
                                    ),
                                ))
                            }),
                    )
                    .child(
                        v_flex()
                            .p_3()
                            .gap_2()
                            .rounded_xl()
                            .bg(pal.drawer_surface)
                            .border_1()
                            .border_color(pal.drawer_border)
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(pal.drawer_fg)
                                    .child("Sonora Audio Processing"),
                            )
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .child(dsp_pill("AEC3", snap.aec))
                                    .child(dsp_pill("Noise Supp", snap.ns))
                                    .child(dsp_pill("Auto Gain", snap.agc)),
                            ),
                    )
                    .child(
                        v_flex()
                            .p_3()
                            .gap_2()
                            .rounded_xl()
                            .bg(pal.drawer_surface)
                            .border_1()
                            .border_color(pal.drawer_border)
                            .child(
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        v_flex()
                                            .gap_0p5()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(pal.drawer_fg)
                                                    .child("Audio-only mode"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(pal.drawer_muted)
                                                    .child("Pause video to save bandwidth"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("huddle-diag-audio-only")
                                            .cursor_pointer()
                                            .px_2p5()
                                            .py_1()
                                            .rounded_full()
                                            .bg(if audio_only {
                                                pal.warning
                                            } else {
                                                pal.drawer_border
                                            })
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(if audio_only {
                                                pal.warning_fg
                                            } else {
                                                pal.drawer_fg
                                            })
                                            .child(if audio_only { "On" } else { "Off" })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                let next = !audio_only;
                                                (send_ao)(Command::SetHuddleAudioOnly(next));
                                                if this.fixtures.sample {
                                                    if let Some(dock) = this.huddle_dock_entity() {
                                                        dock.update(cx, |d, cx| {
                                                            d.set_audio_only_local(next, cx)
                                                        });
                                                    }
                                                }
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .p_3()
                            .gap_2()
                            .rounded_xl()
                            .bg(pal.drawer_surface)
                            .border_1()
                            .border_color(pal.drawer_border)
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(pal.drawer_fg)
                                    .child("Audio self-test"),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(stat_row("Microphone level", format!("{rms_pct}%")))
                                    .child(
                                        div()
                                            .w_full()
                                            .h(px(6.))
                                            .rounded_full()
                                            .bg(pal.drawer_border)
                                            .overflow_hidden()
                                            .child(
                                                div()
                                                    .h_full()
                                                    .w(px((2.8 * rms_pct as f32).clamp(4., 260.)))
                                                    .rounded_full()
                                                    .bg(pal.success),
                                            ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .pt_1()
                                    .child(
                                        div()
                                            .id("huddle-diag-test-tone")
                                            .flex_1()
                                            .cursor_pointer()
                                            .py_1p5()
                                            .rounded_lg()
                                            .bg(pal.drawer_bg)
                                            .border_1()
                                            .border_color(pal.drawer_border)
                                            .hover(move |el| el.bg(hover))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(pal.drawer_fg)
                                            .child("Test speaker")
                                            .on_click(move |_, _, _| {
                                                let _ = play_speaker_test_tone(test_out.as_deref());
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id("huddle-diag-test-chime")
                                            .flex_1()
                                            .cursor_pointer()
                                            .py_1p5()
                                            .rounded_lg()
                                            .bg(pal.drawer_bg)
                                            .border_1()
                                            .border_color(pal.drawer_border)
                                            .hover(move |el| el.bg(hover))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(pal.drawer_fg)
                                            .child("Test chime")
                                            .on_click(move |_, _, _| {
                                                let _ = play_huddle_chime(
                                                    HuddleChime::Join,
                                                    chime_out.as_deref(),
                                                );
                                            }),
                                    ),
                            ),
                    ),
            )
    }

    fn render_huddle_titlebar(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        people_count: usize,
        pal: &HuddlePalette,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui_kit::component::Icon;

        let channel_name = self
            .fixtures
            .conversation(&snap.channel)
            .map(|c| {
                self.fixtures
                    .conversation_label(c)
                    .trim_start_matches("# ")
                    .to_owned()
            })
            .unwrap_or_else(|| self.title.clone());
        let count_text = if people_count == 1 {
            "1 person".to_owned()
        } else {
            format!("{people_count} people")
        };
        let ctrl_hover = pal.ctrl_hover;

        TitleBar::new()
            .h(px(38.))
            .bg(pal.chrome_bg)
            .border_b_1()
            .border_color(pal.chrome_border)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pr_2p5()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .justify_center()
                            .gap_1p5()
                            .child(
                                Icon::new(IconName::Headphones)
                                    .size(px(13.))
                                    .text_color(pal.chrome_muted),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(pal.chrome_fg)
                                    .child(format!("Huddle in {channel_name}")),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(pal.chrome_muted)
                                    .child(count_text),
                            ),
                    )
                    .child(
                        div()
                            .id("popout-huddle-dock-back")
                            .size(px(26.))
                            .rounded_md()
                            .hover(move |el| el.bg(ctrl_hover))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::PanelLeft)
                                    .size(px(14.))
                                    .text_color(pal.chrome_muted),
                            )
                            .on_click(|_, window, _| {
                                window.remove_window();
                            }),
                    ),
            )
    }

    fn render_huddle_music_pill(
        &self,
        active_key: &str,
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::AnyElement {
        use gpui_kit::component::Icon;

        const SONGS: [(&str, &str); 10] = [
            ("jazz", "Smooth Jazz"),
            ("waiting", "Waiting Room"),
            ("subway", "Subway Music"),
            ("whistle", "Whistle Tune"),
            ("focus", "Focus Beats"),
            ("lofi", "Lo-fi Haus"),
            ("disco", "Happy Bass"),
            ("zen", "Sound Bath"),
            ("nature", "Nature Meditation"),
            ("piano", "Piano Concerto"),
        ];

        let active_label = SONGS
            .iter()
            .find(|(k, _)| *k == active_key)
            .map_or("Smooth Jazz", |(_, name)| *name);

        let send_stop = self.send.clone();
        let pill_bg = pal.tile_pill_bg;
        let pill_fg = pal.tile_pill_fg;
        let border = pal.drawer_border;
        let hover_bg = pal.drawer_hover;
        let muted = pal.drawer_muted;
        let primary = pal.primary;
        let pop_bg = pal.popover_bg;
        let pop_fg = pal.popover_fg;
        let pop_border = pal.popover_border;
        let pop_hover = pal.popover_hover;

        let mut wrapper = v_flex()
            .absolute()
            .right(px(14.))
            .top(px(12.))
            .items_end()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        wrapper = wrapper.child(
            h_flex()
                .px_2p5()
                .py_1()
                .gap_1p5()
                .items_center()
                .rounded_lg()
                .bg(pill_bg)
                .border_1()
                .border_color(border)
                .shadow_sm()
                .child(
                    h_flex()
                        .id("popout-huddle-music-picker-btn")
                        .cursor_pointer()
                        .items_center()
                        .gap_1p5()
                        .px_1()
                        .py_0p5()
                        .rounded_md()
                        .hover(move |el| el.bg(hover_bg))
                        .child(Icon::new(IconName::Music).size(px(13.)).text_color(primary))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(pill_fg)
                                .child(active_label),
                        )
                        .child(
                            Icon::new(IconName::ChevronDown)
                                .size(px(12.))
                                .text_color(muted),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            let next = !this.huddle_music_open;
                            this.close_huddle_popovers();
                            this.huddle_music_open = next;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("popout-huddle-music-stop-btn")
                        .cursor_pointer()
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .hover(move |el| el.bg(hover_bg))
                        .child(Icon::new(IconName::X).size(px(12.)).text_color(muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.huddle_music_open = false;
                            (send_stop)(Command::SetHuddleJukebox(None));
                            if this.fixtures.sample {
                                if let Some(dock) = this.huddle_dock_entity() {
                                    dock.update(cx, |d, cx| d.set_jukebox_local(None, cx));
                                }
                            }
                            cx.notify();
                        })),
                ),
        );

        if self.huddle_music_open {
            let mut menu = v_flex()
                .w(px(200.))
                .p_1p5()
                .gap_0p5()
                .rounded_xl()
                .bg(pop_bg)
                .border_1()
                .border_color(pop_border)
                .shadow_xl();

            for (key, name) in SONGS {
                let is_active = key == active_key;
                let send_pick = self.send.clone();
                let key_owned = key.to_owned();
                menu = menu.child(
                    h_flex()
                        .id(SharedString::from(format!("popout-huddle-song-{key}")))
                        .cursor_pointer()
                        .px_2p5()
                        .py_1p5()
                        .rounded_md()
                        .items_center()
                        .justify_between()
                        .text_xs()
                        .text_color(if is_active { primary } else { pop_fg })
                        .font_weight(if is_active {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .hover(move |el| el.bg(pop_hover))
                        .child(name)
                        .when(is_active, |el| {
                            el.child(Icon::new(IconName::Check).size(px(12.)).text_color(primary))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.huddle_music_open = false;
                            (send_pick)(Command::SetHuddleJukebox(Some(key_owned.clone())));
                            if this.fixtures.sample {
                                if let Some(dock) = this.huddle_dock_entity() {
                                    dock.update(cx, |d, cx| {
                                        d.set_jukebox_local(Some(key_owned.clone()), cx)
                                    });
                                }
                            }
                            cx.notify();
                        })),
                );
            }
            wrapper = wrapper.child(menu);
        }

        wrapper.into_any_element()
    }

    fn render_huddle_topic_bar(
        &self,
        topic: Option<&str>,
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let hover = pal.drawer_hover;
        if let Some(input) = &self.huddle_topic_editing {
            return h_flex()
                .items_center()
                .gap_1p5()
                .px_1p5()
                .py_0p5()
                .rounded_md()
                .border_1()
                .border_color(pal.drawer_border)
                .bg(pal.drawer_bg)
                .child(
                    div()
                        .w(px(180.))
                        .text_xs()
                        .text_color(pal.drawer_fg)
                        .child(Textarea::new(input).appearance(false).bordered(false)),
                )
                .child(
                    div()
                        .id("popout-huddle-topic-save")
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .bg(pal.primary)
                        .cursor_pointer()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(pal.primary_fg)
                        .child("Save")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(inp) = this.huddle_topic_editing.take() {
                                let text = inp.read(cx).value().trim().to_owned();
                                (this.send)(Command::SetHuddleTopic(text.clone()));
                                if this.fixtures.sample {
                                    if let Some(dock) = this.huddle_dock_entity() {
                                        let me = this.fixtures.self_user.clone();
                                        dock.update(cx, |d, cx| {
                                            d.apply_data_event(
                                                &slackless_core::huddle::HuddleDataEvent::Topic {
                                                    user: me,
                                                    topic: text,
                                                },
                                                cx,
                                            );
                                        });
                                    }
                                }
                                cx.notify();
                            }
                        })),
                )
                .child(
                    div()
                        .id("popout-huddle-topic-cancel")
                        .px_1p5()
                        .py_0p5()
                        .rounded_md()
                        .hover(move |el| el.bg(hover))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(pal.drawer_muted)
                        .child("✕")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.huddle_topic_editing = None;
                            cx.notify();
                        })),
                );
        }
        let initial = topic.unwrap_or_default().to_owned();
        h_flex().min_w_0().items_center().child(
            h_flex()
                .id("popout-huddle-topic-edit")
                .px_1p5()
                .py_0p5()
                .gap_1()
                .items_center()
                .rounded_md()
                .hover(move |el| el.bg(hover))
                .cursor_pointer()
                .text_xs()
                .text_color(if topic.is_some() {
                    pal.tile_pill_fg
                } else {
                    pal.drawer_muted
                })
                .child(
                    div().max_w(px(200.)).truncate().child(
                        topic
                            .map(ToOwned::to_owned)
                            .unwrap_or_else(|| "Add a topic".to_owned()),
                    ),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    let init = initial.clone();
                    let input = cx.new(|cx| {
                        let mut st = TextareaState::new(window, cx)
                            .placeholder("What are you discussing?")
                            .auto_grow(1, 1);
                        st.set_value(init, window, cx);
                        st
                    });
                    input.read(cx).focus_handle(cx).focus(window, cx);
                    this.huddle_topic_editing = Some(input);
                    cx.notify();
                })),
        )
    }

    fn render_huddle_stage(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::gpui::Div> {
        use slackless_core::huddle::{HuddleDrawMessage, VideoTileKey};

        let mut screens: Vec<(String, String, crate::huddle_dock::DisplayVideoFrame)> = Vec::new();
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
                let local_item = if !state.session.attendee_id.is_empty() {
                    state.session.attendee_id.clone()
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
        let draw_btn = {
            let item = primary_item_id.clone();
            let hue = self.huddle_draw_hue;
            h_flex()
                .id("popout-huddle-stage-draw")
                .h(px(26.))
                .px_2p5()
                .gap_1()
                .items_center()
                .rounded_full()
                .cursor_pointer()
                .bg(if self.huddle_draw_mode {
                    primary
                } else {
                    Hsla::from(rgba(0xFFFFFF1C))
                })
                .hover(move |el| {
                    el.bg(if self.huddle_draw_mode {
                        emerald
                    } else {
                        Hsla::from(rgba(0xFFFFFF2E))
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
                .child(if self.huddle_draw_mode {
                    "Drawing"
                } else {
                    "Draw"
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.huddle_draw_mode = !this.huddle_draw_mode;
                    this.huddle_drawing_active = false;
                    this.huddle_draw_points.clear();
                    this.huddle_draw_local_segments.clear();
                    this.huddle_draw_local_item.clear();
                    if this.huddle_draw_mode {
                        let msg = HuddleDrawMessage::ChangeHue {
                            item_id: item.clone(),
                            hue,
                        };
                        (this.send)(Command::SendHuddleDraw(msg.clone()));
                        this.apply_local_draw(msg, cx);
                    }
                    cx.notify();
                }))
        };

        let swatches = self.huddle_draw_mode.then(|| {
            h_flex()
                .items_center()
                .gap_1()
                .px_1()
                .children(HUES.iter().enumerate().map(|(ix, &(hue, _label))| {
                    let selected = self.huddle_draw_hue == hue;
                    let item = primary_item_id.clone();
                    div()
                        .id(("popout-draw-hue", ix))
                        .size(px(15.))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(hsla(hue as f32 / 360.0, 0.88, 0.56, 1.0))
                        .border_2()
                        .border_color(if selected {
                            white
                        } else {
                            Hsla::from(rgba(0xFFFFFF30))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.huddle_draw_hue = hue;
                            let msg = HuddleDrawMessage::ChangeHue {
                                item_id: item.clone(),
                                hue,
                            };
                            (this.send)(Command::SendHuddleDraw(msg.clone()));
                            this.apply_local_draw(msg, cx);
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
                            rgba(0xFFFFFF2A)
                        } else {
                            rgba(0xFFFFFF10)
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
                            rgba(0xFFFFFF2A)
                        } else {
                            rgba(0xFFFFFF10)
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

        let visible_screens: Vec<(String, String, crate::huddle_dock::DisplayVideoFrame)> =
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
                .bg(rgb(0x0B0D10))
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
                        .border_color(rgba(0xFFFFFF1A))
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
                        .border_color(rgba(0xFFFFFF1A))
                        .children(swatches)
                        .child(draw_btn)
                        .child(
                            div()
                                .id("popout-huddle-stage-expand")
                                .h(px(26.))
                                .px_2()
                                .rounded_full()
                                .cursor_pointer()
                                .bg(rgba(0xFFFFFF14))
                                .hover(|el| el.bg(rgba(0xFFFFFF28)))
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
        if let Some(dock) = self.huddle_dock_entity() {
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
        frame: crate::huddle_dock::DisplayVideoFrame,
        state: &crate::huddle_dock::HuddleViewState,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        use gpui_kit::gpui::{Corners, ObjectFit};
        use slackless_core::huddle::HuddleDrawMessage;

        let filter_id = item_id.clone();
        let strokes: Vec<_> = state
            .draw_strokes
            .iter()
            .filter(|s| s.item_id.is_empty() || s.item_id == filter_id || pane_ix == 0)
            .cloned()
            .collect();
        let local_live_segments = if pane_ix == 0
            && self.huddle_drawing_active
            && !self.huddle_draw_local_segments.is_empty()
        {
            Some((
                self.huddle_draw_hue,
                self.huddle_draw_local_segments.clone(),
            ))
        } else {
            None
        };
        let cursors: Vec<_> = state
            .draw_cursors
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
        let is_surface = matches!(frame, crate::huddle_dock::DisplayVideoFrame::Surface(_));
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
                if pane_ix == 0 {
                    if let Ok(mut guard) = bounds_cell.lock() {
                        *guard = image_bounds;
                    }
                }
                image_bounds
            },
            move |bounds, image_bounds, window, _| {
                match &frame {
                    #[cfg(target_os = "macos")]
                    crate::huddle_dock::DisplayVideoFrame::Surface(surface) => {
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
                    crate::huddle_dock::DisplayVideoFrame::Image(img_frame) => {
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
                        stroke.hue,
                        alpha,
                        width_scale,
                        left,
                        top,
                        width,
                        height,
                    );
                }
                if let Some((hue, ref live_segs)) = local_live_segments {
                    paint_smooth_stroke(window, live_segs, hue, 1.0, 1.0, left, top, width, height);
                }
                for (is_me, cur) in &cursors {
                    if *is_me {
                        continue;
                    }
                    let cx_px = left + cur.x as f32 * width;
                    let cy_px = top + cur.y as f32 * height;
                    let h = (cur.hue.rem_euclid(360)) as f32 / 360.0;
                    if let Ok(halo) = build_smooth_circle(cx_px, cy_px, 11.0).build() {
                        window.paint_path(halo, hsla(h, 0.90, 0.56, 0.22));
                    }
                    if let Ok(ring) = build_smooth_circle(cx_px, cy_px, 6.5).build() {
                        window.paint_path(ring, hsla(0.0, 0.0, 1.0, 0.95));
                    }
                    if let Ok(dot) = build_smooth_circle(cx_px, cy_px, 4.5).build() {
                        window.paint_path(dot, hsla(h, 0.92, 0.54, 0.98));
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

        if self.huddle_draw_mode && pane_ix == 0 {
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
                        let hue_msg = HuddleDrawMessage::ChangeHue {
                            item_id: item_down.clone(),
                            hue: this.huddle_draw_hue,
                        };
                        (this.send)(Command::SendHuddleDraw(hue_msg));
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
                                (this.send)(Command::SendHuddleDraw(HuddleDrawMessage::New {
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
                                (this.send)(Command::SendHuddleDraw(HuddleDrawMessage::Edit {
                                    item_id: item_move.clone(),
                                    segment: refined,
                                }));
                                let new_seg =
                                    make_catmull_rom_segment(p_prev1, p_curr, p_next, p_next);
                                this.huddle_draw_local_segments.push(new_seg);
                                (this.send)(Command::SendHuddleDraw(HuddleDrawMessage::New {
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
                            (this.send)(Command::SendHuddleDraw(pos_msg));
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
        if self.huddle_draw_points.len() == 1 {
            if let Some(&dot) = segs.first() {
                (self.send)(Command::SendHuddleDraw(HuddleDrawMessage::New {
                    item_id: item.clone(),
                    segment: dot,
                }));
            }
        }
        self.huddle_draw_points.clear();
        let end_msg = HuddleDrawMessage::End {
            item_id: item.clone(),
        };
        (self.send)(Command::SendHuddleDraw(end_msg));
        if let Some(dock) = self.huddle_dock_entity() {
            let hue = self.huddle_draw_hue;
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

    fn render_huddle_tiles(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
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
                    state, uid, pal, tile_w, tile_h, avatar_px, solo, cx,
                )
            }))
    }

    fn render_huddle_filmstrip(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
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
                    px(148.),
                    px(96.),
                    44.0,
                    false,
                    cx,
                )
            }))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_huddle_participant_tile(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
        uid: &slackless_core::types::UserId,
        pal: &HuddlePalette,
        tile_w: Pixels,
        tile_h: Pixels,
        avatar_px: f32,
        full_bleed_avatar: bool,
        cx: &Context<Self>,
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;
        use gpui_kit::gpui::ObjectFit;
        use slackless_core::huddle::VideoTileKey;

        let snap = &state.session;
        let is_self = *uid == self.fixtures.self_user;
        let speaking = if is_self {
            !snap.muted && state.levels.local_rms > 0.03
        } else {
            state
                .levels
                .active_speakers
                .iter()
                .any(|(id, lvl)| id == uid && *lvl > 0.05)
        };
        let is_muted = if is_self {
            snap.muted
        } else {
            state.levels.muted_peers.contains(uid)
        };
        let badge_glyph = state
            .sticky_emojis
            .get(uid)
            .map(|s| sticky_emoji_glyph(s).to_owned())
            .or_else(|| state.raised_hands.contains(uid).then(|| "✋".to_owned()));
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
                                Hsla::from(rgba(0x00000000))
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

    fn render_huddle_floating_reactions(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
        pal: &HuddlePalette,
    ) -> Option<gpui_kit::gpui::Div> {
        if state.reactions.is_empty() {
            return None;
        }
        let mut stacked: Vec<(String, usize, Vec<String>)> = Vec::new();
        for entry in &state.reactions {
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

    fn render_huddle_floating_captions(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
        pal: &HuddlePalette,
    ) -> Option<gpui_kit::gpui::Div> {
        let show_captions = state.session.captions_enabled || !state.captions.is_empty();
        if !show_captions {
            return None;
        }
        let mut cap_box = v_flex()
            .max_w(px(620.))
            .px_4()
            .py_2()
            .gap_0p5()
            .rounded_xl()
            .bg(pal.scrim_bg)
            .border_1()
            .border_color(rgba(0xFFFFFF18))
            .shadow_xl()
            .text_xs()
            .text_color(pal.scrim_fg);
        if state.captions.is_empty() {
            cap_box = cap_box.child(
                div()
                    .text_color(pal.scrim_muted)
                    .child("CC · Listening for live speech transcripts…"),
            );
        } else {
            cap_box = cap_box.children(state.captions.iter().map(|(uid, text, is_partial)| {
                let who = self.fixtures.user_name(uid).to_owned();
                h_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(pal.scrim_muted)
                            .child(format!("{who}:")),
                    )
                    .child(
                        div()
                            .text_color(if *is_partial {
                                pal.scrim_muted
                            } else {
                                pal.scrim_fg
                            })
                            .child(text.clone()),
                    )
            }));
        }
        Some(
            h_flex()
                .absolute()
                .bottom(px(14.))
                .left_0()
                .right_0()
                .justify_center()
                .px_4()
                .child(cap_box),
        )
    }

    fn render_huddle_active_popover(
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
                    if this.fixtures.sample {
                        if let Some(dock) = this.huddle_dock_entity() {
                            dock.update(cx, |d, cx| d.set_captions_local(next, cx));
                        }
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
                    if this.fixtures.sample {
                        if let Some(dock) = this.huddle_dock_entity() {
                            dock.update(cx, |d, cx| d.set_jukebox_local(next_song.clone(), cx));
                        }
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
                        if this.fixtures.sample {
                            if let Some(dock) = this.huddle_dock_entity() {
                                dock.update(cx, |d, cx| d.set_blur_local(next_blur, cx));
                            }
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

    fn render_huddle_share_modal(
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
                                    if this.fixtures.sample {
                                        if let Some(dock) = this.huddle_dock_entity() {
                                            dock.update(cx, |d, cx| d.set_sharing_local(None, cx));
                                        }
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
                                if this.fixtures.sample {
                                    if let Some(dock) = this.huddle_dock_entity() {
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
                                        if this.fixtures.sample {
                                            if let Some(dock) = this.huddle_dock_entity() {
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
                                        if this.fixtures.sample {
                                            if let Some(dock) = this.huddle_dock_entity() {
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

    fn render_huddle_controls(
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
                    .bg(if self.huddle_invite_open {
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
                        let next = !this.huddle_invite_open;
                        this.close_huddle_popovers();
                        this.huddle_invite_open = next;
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
                        (send_mute)(Command::SetHuddleMuted(next_muted));
                        if this.fixtures.sample {
                            if let Some(dock) = this.huddle_dock_entity() {
                                dock.update(cx, |d, cx| d.set_muted_local(next_muted, cx));
                            }
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
                        let next = !this.huddle_devices_open;
                        this.close_huddle_popovers();
                        this.huddle_devices_open = next;
                        if this.huddle_devices_open {
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
        let cam_open = self.huddle_camera_open;
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
                        (send_cam)(Command::SetHuddleCamera {
                            enabled: next_cam,
                            device_index: selected_cam_idx,
                            blur: blur_now,
                        });
                        if this.fixtures.sample {
                            if let Some(dock) = this.huddle_dock_entity() {
                                dock.update(cx, |d, cx| d.set_camera_local(next_cam, cx));
                            }
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
                        let next = !this.huddle_camera_open;
                        this.close_huddle_popovers();
                        this.huddle_camera_open = next;
                        if this.huddle_camera_open {
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
                let next = !this.huddle_share_open;
                this.close_share_modal(cx);
                this.close_huddle_popovers();
                this.huddle_share_open = next;
                if this.huddle_share_open {
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
                (send_hand)(Command::SetHuddleHandRaised(next_hand));
                if this.fixtures.sample {
                    if let Some(dock) = this.huddle_dock_entity() {
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
            .bg(if self.huddle_reactions_open {
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
                let next = !this.huddle_reactions_open;
                this.close_huddle_popovers();
                this.huddle_reactions_open = next;
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
            .bg(if self.huddle_theme_open {
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
                let next = !this.huddle_theme_open;
                this.close_huddle_popovers();
                this.huddle_theme_open = next;
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
            .bg(if self.huddle_more_open {
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
                let next = !this.huddle_more_open;
                this.close_huddle_popovers();
                this.huddle_more_open = next;
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
                (send_leave)(Command::LeaveHuddle);
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

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Compute `(alpha, width_scale, needs_anim_frame)` for a fading huddle draw stroke.
/// Active strokes stay bright at `1.0`; completed strokes hold for 3.8s then smoothly ease out
/// over 1.6s at display refresh rate.
fn stroke_fade_curve(ended_at_ms: Option<u64>, now_ms: u64) -> (f32, f32, bool) {
    const HOLD_MS: f32 = 3_800.0;
    const FADE_MS: f32 = 1_600.0;
    let Some(ended_at) = ended_at_ms else {
        return (1.0, 1.0, false);
    };
    let elapsed = now_ms.saturating_sub(ended_at) as f32;
    if elapsed < HOLD_MS {
        // Request animation frames once within the last 100ms of hold so the fade starts smoothly.
        return (1.0, 1.0, elapsed >= HOLD_MS - 100.0);
    }
    if elapsed >= HOLD_MS + FADE_MS {
        return (0.0, 0.72, false);
    }
    let t = ((elapsed - HOLD_MS) / FADE_MS).clamp(0.0, 1.0);
    // Smooth cubic ease-out (`(1 - t)^2` * `(1 + 2t)` complement -> smoothstep).
    let smooth = 1.0 - t * t * (3.0 - 2.0 * t);
    let width_scale = 0.72 + 0.28 * smooth;
    (smooth, width_scale, true)
}

/// Fit a $C^1$-smooth Catmull-Rom cubic Bézier segment from `p1` to `p2` using neighbors `p0`/`p3`,
/// clamping control handles to `0.45 * chord` so sharp bends never self-intersect.
fn make_catmull_rom_segment(
    p0: (f64, f64),
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
) -> slackless_core::huddle::HuddleDrawSegment {
    let chord_dx = p2.0 - p1.0;
    let chord_dy = p2.1 - p1.1;
    let chord_len = (chord_dx * chord_dx + chord_dy * chord_dy).sqrt();
    if chord_len < 1e-9 {
        return slackless_core::huddle::HuddleDrawSegment {
            a: [p1.0, p1.1],
            b: [p1.0, p1.1],
            c: [p2.0, p2.1],
            d: [p2.0, p2.1],
        };
    }
    let max_handle = chord_len * 0.45;
    let clamp_vec = |vx: f64, vy: f64| -> (f64, f64) {
        let len = (vx * vx + vy * vy).sqrt();
        if len > max_handle && len > 1e-9 {
            let s = max_handle / len;
            (vx * s, vy * s)
        } else {
            (vx, vy)
        }
    };
    let (h1x, h1y) = clamp_vec((p2.0 - p0.0) / 6.0, (p2.1 - p0.1) / 6.0);
    let (h2x, h2y) = clamp_vec((p3.0 - p1.0) / 6.0, (p3.1 - p1.1) / 6.0);
    slackless_core::huddle::HuddleDrawSegment {
        a: [p1.0, p1.1],
        b: [p1.0 + h1x, p1.1 + h1y],
        c: [p2.0 - h2x, p2.1 - h2y],
        d: [p2.0, p2.1],
    }
}

fn build_smooth_circle(cx: f32, cy: f32, radius: f32) -> PathBuilder {
    const N: usize = 16;
    let mut pb = PathBuilder::fill();
    for i in 0..N {
        let ang = (i as f32 / N as f32) * std::f32::consts::TAU;
        let p = point(px(cx + radius * ang.cos()), px(cy + radius * ang.sin()));
        if i == 0 {
            pb.move_to(p);
        } else {
            pb.line_to(p);
        }
    }
    pb.close();
    pb
}

/// Build an outward-only semicircular end cap centered at `(cx, cy)` extending in unit direction
/// `(dx, dy)` so it meets a butt-capped stroke edge with zero overlap.
fn build_outward_cap(cx: f32, cy: f32, dx: f32, dy: f32, radius: f32) -> PathBuilder {
    const STEPS: usize = 8;
    let base_ang = dy.atan2(dx) - std::f32::consts::FRAC_PI_2;
    let mut pb = PathBuilder::fill();
    for s in 0..=STEPS {
        let ang = base_ang + (s as f32 / STEPS as f32) * std::f32::consts::PI;
        let p = point(px(cx + radius * ang.cos()), px(cy + radius * ang.sin()));
        if s == 0 {
            pb.move_to(p);
        } else {
            pb.line_to(p);
        }
    }
    pb.close();
    pb
}

#[allow(clippy::too_many_arguments)]
fn paint_smooth_stroke(
    window: &mut Window,
    segments: &[slackless_core::huddle::HuddleDrawSegment],
    hue: i32,
    alpha: f32,
    width_scale: f32,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
) {
    use gpui_kit::gpui::{PathStyle, StrokeOptions};

    if segments.is_empty() {
        return;
    }
    let to_xy = |p: [f64; 2]| (left + p[0] as f32 * width, top + p[1] as f32 * height);
    let h = (hue.rem_euclid(360)) as f32 / 360.0;
    let color = hsla(h, 0.90, 0.54, 0.96 * alpha);
    let stroke_w = 5.0 * width_scale;
    let cap_r = stroke_w * 0.5;

    // Split into contiguous sub-chains so disjoint segments never bridge with a stray line.
    let mut chains: Vec<&[slackless_core::huddle::HuddleDrawSegment]> = Vec::new();
    let mut start = 0usize;
    for i in 1..segments.len() {
        let prev_d = to_xy(segments[i - 1].d);
        let curr_a = to_xy(segments[i].a);
        if (curr_a.0 - prev_d.0).hypot(curr_a.1 - prev_d.1) > 4.0 {
            chains.push(&segments[start..i]);
            start = i;
        }
    }
    chains.push(&segments[start..]);

    for chain in chains {
        // Single-click dot or degenerate zero-length stroke.
        let first_a = to_xy(chain[0].a);
        let total_chord: f32 = chain
            .iter()
            .map(|s| {
                let a = to_xy(s.a);
                let d = to_xy(s.d);
                (d.0 - a.0).hypot(d.1 - a.1)
            })
            .sum();
        if total_chord < 1.0 {
            if let Ok(dot) = build_smooth_circle(first_a.0, first_a.1, cap_r).build() {
                window.paint_path(dot, color);
            }
            continue;
        }

        // Use Lyon's `StrokeTessellator` on the true cubic Bézier curve (never punches holes on
        // self-crossing loops like cursive letters or figure-8s!) with clamped miter limit and
        // clean outward semicircular end caps.
        let opts = StrokeOptions::default()
            .with_line_width(stroke_w)
            .with_miter_limit(1.5)
            .with_tolerance(0.15);
        let mut pb = PathBuilder::default().with_style(PathStyle::Stroke(opts));
        for (ix, seg) in chain.iter().enumerate() {
            let a = to_xy(seg.a);
            let b = to_xy(seg.b);
            let c = to_xy(seg.c);
            let d = to_xy(seg.d);
            if ix == 0 {
                pb.move_to(point(px(a.0), px(a.1)));
            }
            pb.cubic_bezier_to(
                point(px(d.0), px(d.1)),
                point(px(b.0), px(b.1)),
                point(px(c.0), px(c.1)),
            );
        }
        if let Ok(built) = pb.build() {
            window.paint_path(built, color);
        }

        // Start & end outward semicircular caps (strictly outside stroke butt edges -> zero seam).
        let s0 = chain[0];
        let a0 = to_xy(s0.a);
        let t0 = {
            let b0 = to_xy(s0.b);
            let d0 = to_xy(s0.d);
            let (dx, dy) = if (b0.0 - a0.0).hypot(b0.1 - a0.1) > 0.2 {
                (a0.0 - b0.0, a0.1 - b0.1)
            } else {
                (a0.0 - d0.0, a0.1 - d0.1)
            };
            let l = dx.hypot(dy).max(1e-5);
            (dx / l, dy / l)
        };
        if let Ok(cap) = build_outward_cap(a0.0, a0.1, t0.0, t0.1, cap_r).build() {
            window.paint_path(cap, color);
        }
        let sl = chain[chain.len() - 1];
        let dl = to_xy(sl.d);
        let tl = {
            let cl = to_xy(sl.c);
            let al = to_xy(sl.a);
            let (dx, dy) = if (dl.0 - cl.0).hypot(dl.1 - cl.1) > 0.2 {
                (dl.0 - cl.0, dl.1 - cl.1)
            } else {
                (dl.0 - al.0, dl.1 - al.1)
            };
            let l = dx.hypot(dy).max(1e-5);
            (dx / l, dy / l)
        };
        if let Ok(cap) = build_outward_cap(dl.0, dl.1, tl.0, tl.1, cap_r).build() {
            window.paint_path(cap, color);
        }
    }
}

fn sticky_emoji_glyph(code: &str) -> &str {
    match code {
        "raised_hand" => "✋",
        "coffee" => "☕",
        "headphones" => "🎧",
        "white_check_mark" => "✅",
        "+1" | "thumbsup" => "👍",
        "tada" => "🎉",
        "heart" => "❤️",
        "joy" => "😂",
        "clap" => "👏",
        "fire" => "🔥",
        "eyes" => "👀",
        "raised_hands" => "🙌",
        "confetti" => "🎉",
        "hearts" => "💖",
        "sparkles" => "✨",
        "rocket" => "🚀",
        "mic_drop" => "🎤",
        "trophy" => "🏆",
        "unicorn" => "🦄",
        other => other,
    }
}

fn effect_label(code: &str) -> (&'static str, &'static str) {
    match code {
        "confetti" => ("🎉", "Confetti"),
        "hearts" => ("💖", "Hearts"),
        "clap" => ("👏", "Applause"),
        "sparkles" => ("✨", "Sparkles"),
        "rocket" => ("🚀", "Rocket"),
        "mic_drop" => ("🎤", "Mic Drop"),
        "trophy" => ("🏆", "Trophy"),
        "unicorn" => ("🦄", "Unicorn"),
        _ => ("✨", "Effect"),
    }
}
