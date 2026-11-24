//! Huddle pop-out stage, video tiles, drawing, captions, drawers, and popovers.
//!
//! Styled to match official Slack's Huddle window across Light, Dark, and tinted workspace themes:
//! theme-matched top title bar & bottom control bar (left: diagnostics + invite; center: mic,
//! camera, share, hand, reactions, background/theme, thread, more; right: red Leave button),
//! luminous rounded-square participant cards on the stage, and a crisp theme-matched side drawer.

use gpui_kit::gpui::{Background, Hsla, linear_color_stop, linear_gradient};

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
        self.huddle_share_open = false;
        self.huddle_reactions_open = false;
        self.huddle_theme_open = false;
        self.huddle_more_open = false;
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

        let has_screen_share = state
            .video_frames
            .keys()
            .any(|k| matches!(k, VideoTileKey::RemoteScreen(_) | VideoTileKey::LocalScreen));

        let mut stage = v_flex()
            .id("popout-huddle-stage-container")
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(pal.stage_bg)
            .px_5()
            .py_4()
            .gap_3();

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

        // Floating popover panel anchored right above the bottom dock
        if let Some(popover) = self.render_huddle_active_popover(&snap, &channel, &people, &pal, cx)
        {
            stage = stage.child(
                h_flex()
                    .absolute()
                    .bottom(px(10.))
                    .left_0()
                    .right_0()
                    .justify_center()
                    .px_4()
                    .child(popover),
            );
        }

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
                            || this.huddle_reactions_open
                            || this.huddle_theme_open
                            || this.huddle_more_open
                            || this.huddle_invite_open
                            || this.huddle_devices_open
                            || this.huddle_drawer.take().is_some())
                    {
                        this.close_huddle_popovers();
                        cx.notify();
                    }
                }),
            )
            .child(header)
            .child(body)
            .child(self.render_huddle_controls(&snap, status_color, &pal, cx))
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
            (!has_ended).then_some(row.message.ts)
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
            .unwrap_or(if snap.video || snap.sharing {
                "video/H264 (OpenH264)"
            } else {
                "Idle (AV1 / H.264 ready)"
            })
            .to_owned();
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
                .justify_between()
                .items_center()
                .text_xs()
                .child(div().text_color(pal.drawer_muted).child(label))
                .child(
                    div()
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
                                snap.screen_audio_device
                                    .clone()
                                    .unwrap_or_else(|| "None (install BlackHole 2ch)".to_owned()),
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

        let mut screens: Vec<(String, String, Arc<gpui_kit::gpui::RenderImage>)> = Vec::new();
        for (k, v) in &state.video_frames {
            if let VideoTileKey::RemoteScreen(uid) = k {
                screens.push((
                    uid.to_string(),
                    format!("{} is sharing their screen", self.fixtures.user_name(uid)),
                    v.clone(),
                ));
            }
        }
        screens.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some(v) = state.video_frames.get(&VideoTileKey::LocalScreen) {
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
                    this.huddle_draw_last_pt = None;
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

        let visible_screens: Vec<(String, String, Arc<gpui_kit::gpui::RenderImage>)> =
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
        img_frame: Arc<gpui_kit::gpui::RenderImage>,
        state: &crate::huddle_dock::HuddleViewState,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        use gpui_kit::gpui::{ObjectFit, StyledImage as _, img};
        use slackless_core::huddle::HuddleDrawMessage;

        let filter_id = item_id.clone();
        let strokes: Vec<_> = state
            .draw_strokes
            .iter()
            .filter(|s| s.item_id.is_empty() || s.item_id == filter_id || pane_ix == 0)
            .cloned()
            .collect();
        let cursors: Vec<_> = state
            .draw_cursors
            .values()
            .filter(|c| c.item_id.is_empty() || c.item_id == filter_id || pane_ix == 0)
            .cloned()
            .collect();
        let bounds_cell = Arc::clone(&self.huddle_stage_bounds);
        let draw_overlay = canvas(
            move |bounds, _, _| {
                if pane_ix == 0 {
                    if let Ok(mut guard) = bounds_cell.lock() {
                        *guard = bounds;
                    }
                }
            },
            move |bounds, _, window, _| {
                let left: f32 = bounds.origin.x.into();
                let top: f32 = bounds.origin.y.into();
                let width: f32 = bounds.size.width.into();
                let height: f32 = bounds.size.height.into();
                if width <= 1.0 || height <= 1.0 {
                    return;
                }
                let to_pt = |[nx, ny]: [f64; 2]| {
                    point(px(left + nx as f32 * width), px(top + ny as f32 * height))
                };
                for stroke in &strokes {
                    if stroke.segments.is_empty() {
                        continue;
                    }
                    let color = hsla(
                        (stroke.hue.rem_euclid(360)) as f32 / 360.0,
                        0.88,
                        0.56,
                        0.92,
                    );
                    let mut path = PathBuilder::stroke(px(3.5));
                    for (ix, seg) in stroke.segments.iter().enumerate() {
                        if ix == 0 {
                            path.move_to(to_pt(seg.a));
                        }
                        path.cubic_bezier_to(to_pt(seg.d), to_pt(seg.b), to_pt(seg.c));
                    }
                    if let Ok(built) = path.build() {
                        window.paint_path(built, color);
                    }
                }
                for cur in &cursors {
                    let color = hsla((cur.hue.rem_euclid(360)) as f32 / 360.0, 0.9, 0.58, 0.95);
                    let center = to_pt([cur.x, cur.y]);
                    let r = px(6.);
                    let mut dot = PathBuilder::fill();
                    dot.move_to(point(center.x - r, center.y));
                    dot.line_to(point(center.x, center.y - r));
                    dot.line_to(point(center.x + r, center.y));
                    dot.line_to(point(center.x, center.y + r));
                    dot.close();
                    if let Ok(built) = dot.build() {
                        window.paint_path(built, color);
                    }
                }
            },
        )
        .size_full();

        let item_down = item_id.clone();
        let item_move = item_id.clone();
        let item_up = item_id;

        let mut pane = div()
            .id(("popout-huddle-stage-pane", pane_ix))
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .bg(rgb(0x0B0D10))
            .child(
                img(img_frame)
                    .w_full()
                    .h_full()
                    .object_fit(ObjectFit::Contain),
            )
            .child(div().absolute().inset_0().child(draw_overlay));

        if self.huddle_draw_mode && pane_ix == 0 {
            pane = pane
                .cursor_crosshair()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        let Some(pt) = this.normalize_stage_point(ev.position) else {
                            return;
                        };
                        this.huddle_drawing_active = true;
                        this.huddle_draw_last_pt = Some(pt);
                        let hue_msg = HuddleDrawMessage::ChangeHue {
                            item_id: item_down.clone(),
                            hue: this.huddle_draw_hue,
                        };
                        (this.send)(Command::SendHuddleDraw(hue_msg.clone()));
                        this.apply_local_draw(hue_msg, cx);
                    }),
                )
                .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                    let Some(pt) = this.normalize_stage_point(ev.position) else {
                        return;
                    };
                    if this.huddle_drawing_active {
                        if let Some(prev) = this.huddle_draw_last_pt {
                            let dx = pt.0 - prev.0;
                            let dy = pt.1 - prev.1;
                            if dx * dx + dy * dy > 0.00004 {
                                let msg = HuddleDrawMessage::New {
                                    item_id: item_move.clone(),
                                    segment: make_linear_segment(prev, pt),
                                };
                                this.huddle_draw_last_pt = Some(pt);
                                (this.send)(Command::SendHuddleDraw(msg.clone()));
                                this.apply_local_draw(msg, cx);
                            }
                        }
                    } else {
                        let pos_msg = HuddleDrawMessage::Position {
                            item_id: item_move.clone(),
                            x: pt.0,
                            y: pt.1,
                        };
                        (this.send)(Command::SendHuddleDraw(pos_msg.clone()));
                        this.apply_local_draw(pos_msg, cx);
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseUpEvent, _, cx| {
                        if this.huddle_drawing_active {
                            this.huddle_drawing_active = false;
                            this.huddle_draw_last_pt = None;
                            let end_msg = HuddleDrawMessage::End {
                                item_id: item_up.clone(),
                            };
                            (this.send)(Command::SendHuddleDraw(end_msg.clone()));
                            this.apply_local_draw(end_msg, cx);
                        }
                    }),
                );
        }
        pane
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
        _cx: &Context<Self>,
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
                    state, uid, pal, tile_w, tile_h, avatar_px, solo,
                )
            }))
    }

    fn render_huddle_filmstrip(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
        people: &[slackless_core::types::UserId],
        pal: &HuddlePalette,
        _cx: &Context<Self>,
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
                self.render_huddle_participant_tile(state, uid, pal, px(148.), px(96.), 44.0, false)
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
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;
        use gpui_kit::gpui::{ObjectFit, StyledImage as _, img};
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
        let video_frame = if is_self {
            state.video_frames.get(&VideoTileKey::LocalCamera).cloned()
        } else {
            state
                .video_frames
                .get(&VideoTileKey::RemoteCamera(uid.clone()))
                .cloned()
        };
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
                img(frame)
                    .size_full()
                    .object_fit(ObjectFit::Cover)
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
        if self.huddle_share_open {
            return Some(self.render_huddle_share_panel(snap.share_source_id.as_deref(), pal, cx));
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
                        "Play lo-fi background music".to_owned()
                    },
                    music_on,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let next_song = (!music_on).then(|| "lofi".to_owned());
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

    fn render_huddle_share_panel(
        &self,
        active_source: Option<&str>,
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let send = self.send.clone();
        let loopback = slackless_core::huddle::system_audio_source_label();
        let hover = pal.popover_hover;
        let primary = pal.primary;
        let primary_fg = pal.primary_fg;
        let surface = pal.drawer_surface;

        v_flex()
            .max_w(px(560.))
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
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(surface)
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(if loopback.is_some() {
                        pal.success
                    } else {
                        pal.popover_muted
                    })
                    .child(if let Some(dev) = loopback {
                        format!("System audio capture ready via {dev}")
                    } else {
                        "System audio: install BlackHole 2ch (or virtual loopback) to share app audio"
                            .to_owned()
                    }),
            )
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(pal.popover_muted)
                            .child("Share a screen or window"),
                    )
                    .when(active_source.is_some(), |row| {
                        let send_stop = send.clone();
                        row.child(
                            div()
                                .id("popout-huddle-share-stop")
                                .px_2p5()
                                .py_1()
                                .rounded_full()
                                .bg(pal.danger)
                                .cursor_pointer()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(pal.danger_fg)
                                .child("Stop sharing")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    (send_stop)(Command::SetHuddleScreenShare { source_id: None });
                                    if this.fixtures.sample {
                                        if let Some(dock) = this.huddle_dock_entity() {
                                            dock.update(cx, |d, cx| d.set_sharing_local(None, cx));
                                        }
                                    }
                                    this.huddle_share_open = false;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(if self.huddle_screens.is_empty() {
                div()
                    .text_xs()
                    .text_color(pal.popover_muted)
                    .child("Scanning screens & windows… (grant Screen Recording permission in macOS System Settings if none appear)")
                    .into_any_element()
            } else {
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .children(self.huddle_screens.iter().enumerate().map(|(ix, src)| {
                        let selected = active_source == Some(src.id.as_str());
                        let src_id = src.id.clone();
                        let send_pick = send.clone();
                        let prefix = if src.is_monitor { "🖥 " } else { "🪟 " };
                        div()
                            .id(("popout-huddle-src", ix))
                            .px_2p5()
                            .py_1()
                            .rounded_lg()
                            .cursor_pointer()
                            .bg(if selected { primary } else { surface })
                            .hover(move |el| el.bg(if selected { primary } else { hover }))
                            .text_xs()
                            .text_color(if selected { primary_fg } else { pal.popover_fg })
                            .child(if selected {
                                format!("✓ {prefix}{}", src.name)
                            } else {
                                format!("{prefix}{}", src.name)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                (send_pick)(Command::SetHuddleScreenShare {
                                    source_id: Some(src_id.clone()),
                                });
                                if this.fixtures.sample {
                                    if let Some(dock) = this.huddle_dock_entity() {
                                        dock.update(cx, |d, cx| {
                                            d.set_sharing_local(Some(src_id.clone()), cx);
                                        });
                                    }
                                }
                                this.huddle_share_open = false;
                                cx.notify();
                            }))
                    }))
                    .into_any_element()
            })
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
        _cx: &Context<Self>,
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
    }

    fn render_huddle_controls(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        status_color: Hsla,
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;
        use slackless_core::huddle::{list_audio_devices, list_screen_sources};

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
        let cam_btn = div()
            .id("popout-huddle-video")
            .size(px(36.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if video_now { active_bg } else { ctrl_bg })
            .hover(move |el| el.bg(if video_now { active_bg } else { ctrl_hover }))
            .child(
                Icon::new(if video_now {
                    IconName::Video
                } else {
                    IconName::VideoOff
                })
                .size(px(16.))
                .text_color(if video_now { active_fg } else { ctrl_fg }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                (send_cam)(Command::SetHuddleCamera {
                    enabled: next_cam,
                    device_index: None,
                    blur: blur_now,
                });
                if this.fixtures.sample {
                    if let Some(dock) = this.huddle_dock_entity() {
                        dock.update(cx, |d, cx| d.set_camera_local(next_cam, cx));
                    }
                }
            }));

        let send_blur = self.send.clone();
        let next_blur = !blur_now;
        let blur_btn = div()
            .id("popout-huddle-blur")
            .h(px(36.))
            .px_2p5()
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if blur_now { active_bg } else { ctrl_bg })
            .hover(move |el| el.bg(if blur_now { active_bg } else { ctrl_hover }))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(if blur_now { active_fg } else { ctrl_fg })
            .child(if blur_now { "Blur on" } else { "Blur" })
            .on_click(cx.listener(move |this, _, _, cx| {
                (send_blur)(Command::SetHuddleBlurBackground(next_blur));
                if this.fixtures.sample {
                    if let Some(dock) = this.huddle_dock_entity() {
                        dock.update(cx, |d, cx| d.set_blur_local(next_blur, cx));
                    }
                }
            }));

        let primary = pal.primary;
        let primary_hover = pal.primary_hover;
        let primary_fg = pal.primary_fg;
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
            .on_click(cx.listener(|this, _, _, cx| {
                let next = !this.huddle_share_open;
                this.close_huddle_popovers();
                this.huddle_share_open = next;
                if this.huddle_share_open {
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
                    .when(video_now, |bar| bar.child(blur_btn))
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

fn make_linear_segment(
    p0: (f64, f64),
    p1: (f64, f64),
) -> slackless_core::huddle::HuddleDrawSegment {
    let lerp = |t: f64| [p0.0 + (p1.0 - p0.0) * t, p0.1 + (p1.1 - p0.1) * t];
    slackless_core::huddle::HuddleDrawSegment {
        a: [p0.0, p0.1],
        b: lerp(1.0 / 3.0),
        c: lerp(2.0 / 3.0),
        d: [p1.0, p1.1],
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
