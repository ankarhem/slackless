//! Huddle pop-out stage, video tiles, drawing, captions, drawers, and popovers.

use super::*;

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
    }

    pub(super) fn render_huddle_window(
        &self,
        on_jump: JumpHandler,
        on_open_canvas: CanvasOpenHandler,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        use gpui_kit::component::Icon;
        use slackless_core::huddle::{HuddlePhase, VideoTileKey};

        let white = rgb(0xFFFFFF);
        let muted = rgb(0xABABAD);
        let emerald = rgb(0x2BAC76);
        let amber = rgb(0xECB22E);
        let crimson = rgb(0xE01E5A);

        let bg_id = self
            .huddle_state
            .as_ref()
            .and_then(|s| s.session.background_id.as_deref());
        let (stage_bg, tile_bg) = huddle_theme_surface(bg_id);

        let Some(state) = self.huddle_state.clone() else {
            return v_flex()
                .id("popout-huddle-ended")
                .size_full()
                .bg(stage_bg)
                .text_color(white)
                .child(
                    TitleBar::new()
                        .h(px(44.))
                        .bg(stage_bg)
                        .border_b_1()
                        .border_color(rgba(0xFFFFFF12))
                        .child(
                            h_flex()
                                .flex_1()
                                .h_full()
                                .items_center()
                                .gap_2()
                                .child(
                                    Icon::new(IconName::Headphones)
                                        .size(px(14.))
                                        .text_color(muted),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(white)
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
                                .text_color(muted),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
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
            HuddlePhase::Joining | HuddlePhase::Connecting => (amber, "Connecting…".to_owned()),
            HuddlePhase::Connected => (
                emerald,
                crate::huddle_dock::format_elapsed(state.elapsed_secs),
            ),
            HuddlePhase::Reconnecting => (amber, "Reconnecting…".to_owned()),
            HuddlePhase::Leaving => (muted, "Leaving…".to_owned()),
            HuddlePhase::Failed => (crimson, "Failed".to_owned()),
            HuddlePhase::Idle => (muted, "Ended".to_owned()),
        };

        let header = self.render_huddle_titlebar(&snap, status_color, status_text, stage_bg, cx);

        let has_screen_share = state
            .video_frames
            .keys()
            .any(|k| matches!(k, VideoTileKey::RemoteScreen(_) | VideoTileKey::LocalScreen));

        let mut stage = v_flex()
            .id("popout-huddle-stage-container")
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .px_4()
            .py_3()
            .gap_3();

        if snap.audio_only {
            let send_resume = self.send.clone();
            stage = stage.child(
                h_flex()
                    .px_3()
                    .py_1p5()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .rounded_xl()
                    .bg(amber.opacity(0.16))
                    .border_1()
                    .border_color(amber.opacity(0.45))
                    .text_xs()
                    .text_color(white)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Headphones)
                                    .size(px(13.))
                                    .text_color(amber),
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
                            .bg(rgba(0xFFFFFF18))
                            .hover(|el| el.bg(rgba(0xFFFFFF2A)))
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
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .rounded_xl()
                    .bg(crimson.opacity(0.18))
                    .border_1()
                    .border_color(crimson.opacity(0.5))
                    .text_xs()
                    .text_color(white)
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size(px(14.))
                            .text_color(crimson),
                    )
                    .child(err),
            );
        }

        if has_screen_share {
            if let Some(screen_stage) = self.render_huddle_stage(&state, cx) {
                stage = stage.child(screen_stage);
            }
            stage = stage.child(self.render_huddle_filmstrip(&state, &people, tile_bg, cx));
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
                    .child(self.render_huddle_tiles(&state, &people, tile_bg, cx)),
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
                            .bg(rgba(0x14171CE6))
                            .border_1()
                            .border_color(emerald.opacity(0.65))
                            .shadow_xl()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(white)
                            .child(format!(
                                "{glyph} {glyph} {who} triggered {label}! {glyph} {glyph}"
                            )),
                    ),
            );
        }

        // Floating stacking reactions (bottom-left above dock)
        if let Some(reactions_el) = self.render_huddle_floating_reactions(&state) {
            stage = stage.child(reactions_el);
        }

        // Floating live captions subtitle overlay (bottom-center above dock)
        if let Some(captions_el) = self.render_huddle_floating_captions(&state) {
            stage = stage.child(captions_el);
        }

        // Floating popover panel anchored right above the bottom dock
        if let Some(popover) = self.render_huddle_active_popover(&snap, &channel, &people, cx) {
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

        let body = h_flex().flex_1().min_h_0().w_full().child(stage).when_some(
            self.huddle_drawer,
            |row, drawer| {
                row.child(self.render_huddle_drawer(
                    drawer,
                    &state,
                    on_jump.clone(),
                    on_open_canvas.clone(),
                    cx,
                ))
            },
        );

        v_flex()
            .id("popout-huddle-root")
            .relative()
            .size_full()
            .bg(stage_bg)
            .text_color(white)
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape"
                        && (this.huddle_topic_editing.take().is_some()
                            || this.huddle_share_open
                            || this.huddle_reactions_open
                            || this.huddle_theme_open
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
            .child(self.render_huddle_controls(&snap, stage_bg, cx))
    }

    fn render_huddle_drawer(
        &self,
        drawer: HuddleDrawer,
        state: &crate::huddle_dock::HuddleViewState,
        on_jump: JumpHandler,
        on_open_canvas: CanvasOpenHandler,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let content: gpui_kit::gpui::AnyElement = match drawer {
            HuddleDrawer::Thread => self
                .render_huddle_thread_drawer(&state.session, on_jump, cx)
                .into_any_element(),
            HuddleDrawer::Notes => self
                .render_huddle_notes_drawer(&state.session, on_open_canvas, cx)
                .into_any_element(),
            HuddleDrawer::Diagnostics => self
                .render_huddle_diagnostics_drawer(state, cx)
                .into_any_element(),
        };
        v_flex()
            .flex_none()
            .w(px(340.))
            .h_full()
            .bg(rgb(0x1B1D21))
            .border_l_1()
            .border_color(rgba(0xFFFFFF16))
            .text_color(rgb(0xFFFFFF))
            .child(content)
    }

    fn huddle_thread_messages(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
    ) -> Vec<Message> {
        let mut out = if let (Some(store), Some(team)) = (&self.store, &self.team) {
            store
                .lock()
                .ok()
                .and_then(|guard| {
                    let rows = if let Some(root_ts) = &snap.thread_root_ts {
                        guard.thread_replies(team, &snap.channel, root_ts)
                    } else {
                        guard.channel_history(team, &snap.channel, None, 20)
                    };
                    rows.ok().map(|list| {
                        list.into_iter()
                            .filter(|r| !r.deleted)
                            .map(|r| r.message)
                            .collect::<Vec<_>>()
                    })
                })
                .unwrap_or_default()
        } else if let Some(root_ts) = &snap.thread_root_ts {
            self.fixtures
                .threads
                .get(&(snap.channel.clone(), root_ts.clone()))
                .cloned()
                .unwrap_or_default()
        } else {
            self.fixtures
                .messages
                .get(&snap.channel)
                .map(|list| list.iter().rev().take(12).rev().cloned().collect())
                .unwrap_or_default()
        };
        for local in &self.huddle_thread_local {
            if !out.iter().any(|m| m.ts == local.ts) {
                out.push(local.clone());
            }
        }
        out
    }

    fn render_huddle_thread_drawer(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        on_jump: JumpHandler,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;

        let (white, muted, border, emerald) = (
            rgb(0xFFFFFF),
            rgb(0xABABAD),
            rgba(0xFFFFFF16),
            rgb(0x007A5A),
        );
        let channel = snap.channel.clone();
        let thread_ts = snap.thread_root_ts.clone();
        let jump_ch = channel.clone();
        let jump_ts = thread_ts.clone();
        let messages = self.huddle_thread_messages(snap);
        let send = self.send.clone();
        let reply_ch = channel.clone();
        let reply_ts = thread_ts.clone();

        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .px_3p5()
                    .py_2p5()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::MessageSquare)
                                    .size(px(14.))
                                    .text_color(muted),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(white)
                                    .child("Huddle thread"),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .id("huddle-drawer-thread-pop")
                                    .size(px(24.))
                                    .rounded_md()
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .hover(|el| el.bg(rgba(0xFFFFFF1A)))
                                    .child(
                                        Icon::new(IconName::ExternalLink)
                                            .size(px(13.))
                                            .text_color(muted),
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
                                    .size(px(24.))
                                    .rounded_md()
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .hover(|el| el.bg(rgba(0xFFFFFF1A)))
                                    .child(Icon::new(IconName::X).size(px(13.)).text_color(muted))
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
                    .px_3()
                    .py_2p5()
                    .gap_2p5()
                    .when(messages.is_empty(), |col| {
                        col.child(div().py_6().text_xs().text_color(muted).child(
                            "No messages in this huddle thread yet. Drop a link or note below.",
                        ))
                    })
                    .children(messages.into_iter().map(|msg| {
                        let author = msg
                            .user
                            .as_ref()
                            .map(|u| self.fixtures.user_name(u).to_owned())
                            .or_else(|| msg.username.clone())
                            .unwrap_or_else(|| "Slack".to_owned());
                        v_flex()
                            .p_2()
                            .gap_1()
                            .rounded_lg()
                            .bg(rgba(0xFFFFFF0A))
                            .border_1()
                            .border_color(rgba(0xFFFFFF0F))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(white)
                                    .child(author),
                            )
                            .child(div().text_xs().text_color(rgb(0xD1D2D3)).child(msg.text))
                    })),
            )
            .child(
                v_flex()
                    .p_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        div()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgba(0xFFFFFF22))
                            .bg(rgb(0x222529))
                            .px_2()
                            .py_1()
                            .child(Textarea::new(&self.huddle_thread_input).appearance(false)),
                    )
                    .child(
                        h_flex().justify_end().child(
                            div()
                                .id("huddle-drawer-thread-send")
                                .cursor_pointer()
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .bg(emerald)
                                .hover(|el| el.bg(rgb(0x008E69)))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(white)
                                .child("Send")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let raw = this.huddle_thread_input.read(cx).value().to_string();
                                    let trimmed = raw.trim();
                                    if trimmed.is_empty() {
                                        return;
                                    }
                                    let content =
                                        crate::composer::content_of(trimmed, &this.fixtures);
                                    (send)(Command::SendMessage {
                                        channel: reply_ch.clone(),
                                        content,
                                        thread_ts: reply_ts.clone(),
                                        broadcast: false,
                                    });
                                    this.huddle_thread_local.push(Message {
                                        ts: Ts::from(format!(
                                            "{}.000001",
                                            crate::drafts::now_secs()
                                        )),
                                        user: Some(this.fixtures.self_user.clone()),
                                        text: trimmed.to_owned(),
                                        ..Default::default()
                                    });
                                    this.huddle_thread_input.update(cx, |s, cx| {
                                        s.set_value("", window, cx);
                                    });
                                    cx.notify();
                                })),
                        ),
                    ),
            )
    }

    fn render_huddle_notes_drawer(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        on_open_canvas: CanvasOpenHandler,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;

        let (white, muted, border, emerald) = (
            rgb(0xFFFFFF),
            rgb(0xABABAD),
            rgba(0xFFFFFF16),
            rgb(0x007A5A),
        );
        let send_notes = self.send.clone();
        let notes_ch = snap.channel.clone();
        let canvas_id = snap.canvas_file_id.clone();
        let topic_title = snap
            .topic
            .clone()
            .map(|t| format!("Huddle notes: {t}"))
            .unwrap_or_else(|| "Huddle notes".to_owned());

        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .px_3p5()
                    .py_2p5()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::new(IconName::FileText).size(px(14.)).text_color(muted))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(white)
                                    .child("Huddle notes"),
                            ),
                    )
                    .child(
                        div()
                            .id("huddle-drawer-notes-close")
                            .size(px(24.))
                            .rounded_md()
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(|el| el.bg(rgba(0xFFFFFF1A)))
                            .child(Icon::new(IconName::X).size(px(13.)).text_color(muted))
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
                            .text_color(white)
                            .child(topic_title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
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
                            .bg(emerald)
                            .hover(|el| el.bg(rgb(0x008E69)))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(white)
                            .child(Icon::new(IconName::FileText).size(px(14.)).text_color(white))
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
                            .bg(emerald)
                            .hover(|el| el.bg(rgb(0x008E69)))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(white)
                            .child(Icon::new(IconName::Plus).size(px(14.)).text_color(white))
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
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        use gpui_kit::component::Icon;
        use slackless_core::huddle::{
            HuddleChime, HuddlePhase, play_huddle_chime, play_speaker_test_tone,
        };

        let (white, muted, border, emerald, amber, crimson) = (
            rgb(0xFFFFFF),
            rgb(0xABABAD),
            rgba(0xFFFFFF16),
            rgb(0x2BAC76),
            rgb(0xECB22E),
            rgb(0xE01E5A),
        );
        let snap = &state.session;
        let (phase_color, phase_label) = match snap.phase {
            HuddlePhase::Connected => (emerald, "Connected (UDP TURN Relay)"),
            HuddlePhase::Joining | HuddlePhase::Connecting => (amber, "Connecting…"),
            HuddlePhase::Reconnecting => (amber, "Reconnecting…"),
            HuddlePhase::Failed if snap.udp_blocked => (crimson, "UDP Blocked — Check Firewall"),
            HuddlePhase::Failed => (crimson, "Connection Failed"),
            HuddlePhase::Leaving => (muted, "Leaving…"),
            HuddlePhase::Idle => (muted, "Idle"),
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

        let stat_row = |label: &'static str, value: String| {
            h_flex()
                .justify_between()
                .items_center()
                .text_xs()
                .child(div().text_color(muted).child(label))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(white)
                        .child(value),
                )
        };
        let dsp_pill = |label: &'static str, on: bool| {
            div()
                .px_2()
                .py_0p5()
                .rounded_full()
                .bg(if on {
                    emerald.opacity(0.2)
                } else {
                    rgba(0xFFFFFF12)
                })
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if on { emerald } else { muted })
                .child(label)
        };

        v_flex()
            .id("huddle-drawer-diag-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                h_flex()
                    .px_3p5()
                    .py_2p5()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(border)
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
                                    .text_color(white)
                                    .child("Connection & diagnostics"),
                            ),
                    )
                    .child(
                        div()
                            .id("huddle-drawer-diag-close")
                            .size(px(24.))
                            .rounded_md()
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(|el| el.bg(rgba(0xFFFFFF1A)))
                            .child(Icon::new(IconName::X).size(px(13.)).text_color(muted))
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
                            .bg(rgba(0xFFFFFF0A))
                            .border_1()
                            .border_color(border)
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
                                            .text_color(white)
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
                            .gap_2()
                            .rounded_xl()
                            .bg(rgba(0xFFFFFF0A))
                            .border_1()
                            .border_color(border)
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(white)
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
                            .bg(rgba(0xFFFFFF0A))
                            .border_1()
                            .border_color(border)
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
                                                    .text_color(white)
                                                    .child("Audio-only mode"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(muted)
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
                                            .bg(if audio_only { amber } else { rgba(0xFFFFFF18) })
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(if audio_only {
                                                rgb(0x14171C)
                                            } else {
                                                white
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
                            .bg(rgba(0xFFFFFF0A))
                            .border_1()
                            .border_color(border)
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(white)
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
                                            .bg(rgba(0xFFFFFF18))
                                            .overflow_hidden()
                                            .child(
                                                div()
                                                    .h_full()
                                                    .w(px((2.8 * rms_pct as f32).clamp(4., 280.)))
                                                    .rounded_full()
                                                    .bg(emerald),
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
                                            .bg(rgba(0xFFFFFF14))
                                            .hover(|el| el.bg(rgba(0xFFFFFF24)))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(white)
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
                                            .bg(rgba(0xFFFFFF14))
                                            .hover(|el| el.bg(rgba(0xFFFFFF24)))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(white)
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

    #[allow(clippy::too_many_arguments)]
    fn render_huddle_titlebar(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        status_color: Rgba,
        status_text: String,
        bar_bg: Rgba,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui_kit::component::Icon;

        let white = rgb(0xFFFFFF);
        let muted = rgb(0xABABAD);
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
        let send_copy = self.send.clone();
        let copy_ch = channel.clone();
        let notes_open = self.huddle_drawer == Some(HuddleDrawer::Notes);
        let thread_open = self.huddle_drawer == Some(HuddleDrawer::Thread);
        let diag_open = self.huddle_drawer == Some(HuddleDrawer::Diagnostics);

        let notes_pill = h_flex()
            .id("popout-huddle-canvas")
            .h(px(28.))
            .px_2p5()
            .gap_1p5()
            .items_center()
            .rounded_lg()
            .bg(if notes_open {
                rgba(0xFFFFFF2A)
            } else {
                rgba(0xFFFFFF12)
            })
            .hover(|el| el.bg(rgba(0xFFFFFF22)))
            .cursor_pointer()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(white)
            .child(
                Icon::new(IconName::FileText)
                    .size(px(13.))
                    .text_color(white),
            )
            .child("Notes")
            .on_click(cx.listener(|this, _, _, cx| {
                this.huddle_drawer = if this.huddle_drawer == Some(HuddleDrawer::Notes) {
                    None
                } else {
                    Some(HuddleDrawer::Notes)
                };
                cx.notify();
            }));

        TitleBar::new()
            .h(px(44.))
            .bg(bar_bg)
            .border_b_1()
            .border_color(rgba(0xFFFFFF10))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pr_3()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Headphones)
                                    .size(px(14.))
                                    .text_color(status_color),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(white)
                                    .child(self.title.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(status_color.opacity(0.18))
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(status_color)
                                    .child(status_text),
                            )
                            .child(self.render_huddle_topic_bar(snap.topic.as_deref(), cx)),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap_1p5()
                            .child(
                                h_flex()
                                    .id("popout-huddle-header-invite")
                                    .h(px(28.))
                                    .px_2p5()
                                    .gap_1p5()
                                    .items_center()
                                    .rounded_lg()
                                    .bg(if self.huddle_invite_open {
                                        rgba(0xFFFFFF28)
                                    } else {
                                        rgba(0xFFFFFF12)
                                    })
                                    .hover(|el| el.bg(rgba(0xFFFFFF22)))
                                    .cursor_pointer()
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(white)
                                    .child(
                                        Icon::new(IconName::UserPlus)
                                            .size(px(13.))
                                            .text_color(white),
                                    )
                                    .child("Invite")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let next = !this.huddle_invite_open;
                                        this.close_huddle_popovers();
                                        this.huddle_invite_open = next;
                                        cx.notify();
                                    })),
                            )
                            .child(notes_pill)
                            .child(
                                h_flex()
                                    .id("popout-huddle-thread")
                                    .h(px(28.))
                                    .px_2p5()
                                    .gap_1p5()
                                    .items_center()
                                    .rounded_lg()
                                    .bg(if thread_open {
                                        rgba(0xFFFFFF2A)
                                    } else {
                                        rgba(0xFFFFFF12)
                                    })
                                    .hover(|el| el.bg(rgba(0xFFFFFF22)))
                                    .cursor_pointer()
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(white)
                                    .child(
                                        Icon::new(IconName::MessageSquare)
                                            .size(px(13.))
                                            .text_color(white),
                                    )
                                    .child("Thread")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.huddle_drawer =
                                            if this.huddle_drawer == Some(HuddleDrawer::Thread) {
                                                None
                                            } else {
                                                Some(HuddleDrawer::Thread)
                                            };
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("popout-huddle-diag")
                                    .size(px(28.))
                                    .rounded_lg()
                                    .bg(if diag_open {
                                        rgba(0xFFFFFF2A)
                                    } else {
                                        rgba(0xFFFFFF12)
                                    })
                                    .hover(|el| el.bg(rgba(0xFFFFFF22)))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Icon::new(IconName::Activity)
                                            .size(px(13.))
                                            .text_color(status_color),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.huddle_drawer = if this.huddle_drawer
                                            == Some(HuddleDrawer::Diagnostics)
                                        {
                                            None
                                        } else {
                                            Some(HuddleDrawer::Diagnostics)
                                        };
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("popout-huddle-copy-link")
                                    .size(px(28.))
                                    .rounded_lg()
                                    .bg(rgba(0xFFFFFF12))
                                    .hover(|el| el.bg(rgba(0xFFFFFF22)))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Icon::new(IconName::Link).size(px(13.)).text_color(muted),
                                    )
                                    .on_click(move |_, _, _| {
                                        (send_copy)(Command::CopyHuddleLink {
                                            channel: copy_ch.clone(),
                                        });
                                    }),
                            )
                            .child(
                                div()
                                    .id("popout-huddle-browser")
                                    .size(px(28.))
                                    .rounded_lg()
                                    .bg(rgba(0xFFFFFF12))
                                    .hover(|el| el.bg(rgba(0xFFFFFF22)))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Icon::new(IconName::ExternalLink)
                                            .size(px(13.))
                                            .text_color(muted),
                                    )
                                    .on_click(move |_, _, cx| cx.open_url(&browser_url)),
                            ),
                    ),
            )
    }

    fn render_huddle_topic_bar(
        &self,
        topic: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let white = rgb(0xFFFFFF);
        let muted = rgb(0xABABAD);
        if let Some(input) = &self.huddle_topic_editing {
            return h_flex()
                .items_center()
                .gap_1p5()
                .px_2()
                .py_0p5()
                .rounded_lg()
                .border_1()
                .border_color(rgba(0xFFFFFF28))
                .bg(rgb(0x2B2E33))
                .child(
                    div()
                        .w(px(180.))
                        .text_xs()
                        .text_color(white)
                        .child(Textarea::new(input).appearance(false).bordered(false)),
                )
                .child(
                    div()
                        .id("popout-huddle-topic-save")
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .bg(rgb(0x007A5A))
                        .cursor_pointer()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(white)
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
                        .hover(|el| el.bg(rgba(0xFFFFFF16)))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(muted)
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
                .px_2()
                .py_0p5()
                .gap_1()
                .items_center()
                .rounded_md()
                .hover(|el| el.bg(rgba(0xFFFFFF12)))
                .cursor_pointer()
                .text_xs()
                .text_color(if topic.is_some() { white } else { muted })
                .child(
                    div().max_w(px(220.)).truncate().child(
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

        let white = rgb(0xFFFFFF);
        let emerald = rgb(0x2BAC76);
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
                    rgb(0x007A5A)
                } else {
                    rgba(0xFFFFFF16)
                })
                .hover(|el| {
                    el.bg(if self.huddle_draw_mode {
                        emerald
                    } else {
                        rgba(0xFFFFFF28)
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
                        .border_color(if selected { white } else { rgba(0xFFFFFF30) })
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
                .rounded_2xl()
                .overflow_hidden()
                .border_1()
                .border_color(rgba(0xFFFFFF18))
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
                        .bg(rgba(0x0E1014CC))
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
                        .bg(rgba(0x0E1014CC))
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
                                .bg(rgba(0xFFFFFF12))
                                .hover(|el| el.bg(rgba(0xFFFFFF24)))
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
        tile_bg: Rgba,
        _cx: &Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let (tile_w, tile_h, avatar_px, grid_max_w) = match people.len() {
            0..=2 => (px(360.), px(244.), 88.0, px(820.)),
            3 => (px(268.), px(196.), 76.0, px(860.)),
            4 => (px(304.), px(192.), 74.0, px(660.)),
            _ => (px(236.), px(154.), 62.0, px(840.)),
        };
        h_flex()
            .max_w(grid_max_w)
            .flex_wrap()
            .items_center()
            .justify_center()
            .gap_3p5()
            .children(people.iter().map(|uid| {
                self.render_huddle_participant_tile(state, uid, tile_bg, tile_w, tile_h, avatar_px)
            }))
    }

    fn render_huddle_filmstrip(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
        people: &[slackless_core::types::UserId],
        tile_bg: Rgba,
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
                self.render_huddle_participant_tile(state, uid, tile_bg, px(148.), px(92.), 42.0)
            }))
    }

    fn render_huddle_participant_tile(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
        uid: &slackless_core::types::UserId,
        tile_bg: Rgba,
        tile_w: Pixels,
        tile_h: Pixels,
        avatar_px: f32,
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;
        use gpui_kit::gpui::{ObjectFit, StyledImage as _, img};
        use slackless_core::huddle::VideoTileKey;

        let white = rgb(0xFFFFFF);
        let emerald = rgb(0x2BAC76);
        let crimson = rgb(0xE01E5A);
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
        let name = self.fixtures.user_name(uid).to_owned();
        let display_name = if is_self {
            format!("{name} (You)")
        } else {
            name
        };

        div()
            .relative()
            .w(tile_w)
            .h(tile_h)
            .rounded_2xl()
            .overflow_hidden()
            .bg(tile_bg)
            .border_2()
            .border_color(if speaking { emerald } else { rgba(0xFFFFFF14) })
            .child(if let Some(frame) = video_frame {
                img(frame)
                    .size_full()
                    .object_fit(ObjectFit::Cover)
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
                                emerald.opacity(0.65)
                            } else {
                                rgba(0x00000000)
                            })
                            .child(crate::avatars::user_chip(&self.fixtures, uid, avatar_px)),
                    )
                    .into_any_element()
            })
            // Top-left frosted sticky/hand-raise pill
            .when_some(badge_glyph, |el, glyph| {
                el.child(
                    div()
                        .absolute()
                        .left(px(10.))
                        .top(px(10.))
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .bg(rgba(0x0E1014D6))
                        .border_1()
                        .border_color(rgba(0xFFFFFF1F))
                        .text_sm()
                        .child(glyph),
                )
            })
            // Bottom-left frosted dark name pill with inline mic-off / active-speaker indicator
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
                    .rounded_lg()
                    .bg(rgba(0x0E1014C8))
                    .when(is_muted, |pill| {
                        pill.child(
                            Icon::new(IconName::MicOff)
                                .size(px(11.))
                                .text_color(crimson),
                        )
                    })
                    .when(!is_muted && speaking, |pill| {
                        pill.child(div().size(px(7.)).rounded_full().bg(emerald))
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(white)
                            .child(display_name),
                    ),
            )
    }

    fn render_huddle_floating_reactions(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
    ) -> Option<gpui_kit::gpui::Div> {
        if state.reactions.is_empty() {
            return None;
        }
        let white = rgb(0xFFFFFF);
        let muted = rgb(0xABABAD);
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
                        .bg(rgba(0x14171CE0))
                        .border_1()
                        .border_color(rgba(0xFFFFFF1F))
                        .shadow_lg()
                        .text_xs()
                        .text_color(white)
                        .child(label)
                        .child(div().text_color(muted).child(names.join(", ")))
                })),
        )
    }

    fn render_huddle_floating_captions(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
    ) -> Option<gpui_kit::gpui::Div> {
        let show_captions = state.session.captions_enabled || !state.captions.is_empty();
        if !show_captions {
            return None;
        }
        let white = rgb(0xFFFFFF);
        let muted = rgb(0xABABAD);
        let mut cap_box = v_flex()
            .max_w(px(620.))
            .px_4()
            .py_2()
            .gap_0p5()
            .rounded_xl()
            .bg(rgba(0x0B0D10DC))
            .border_1()
            .border_color(rgba(0xFFFFFF18))
            .shadow_xl()
            .text_xs()
            .text_color(white);
        if state.captions.is_empty() {
            cap_box = cap_box.child(
                div()
                    .text_color(muted)
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
                            .text_color(muted)
                            .child(format!("{who}:")),
                    )
                    .child(
                        div()
                            .text_color(if *is_partial { muted } else { white })
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
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::gpui::Div> {
        if self.huddle_share_open {
            return Some(self.render_huddle_share_panel(snap.share_source_id.as_deref(), cx));
        }
        if self.huddle_reactions_open {
            return Some(self.render_huddle_reactions_panel(snap, cx));
        }
        if self.huddle_theme_open {
            return Some(self.render_huddle_theme_panel(snap.background_id.as_deref(), cx));
        }
        if self.huddle_invite_open {
            return Some(self.render_huddle_invite_panel(channel, people, cx));
        }
        if self.huddle_devices_open {
            return Some(self.render_huddle_devices_panel(
                snap.input_device.clone(),
                snap.output_device.clone(),
                cx,
            ));
        }
        None
    }

    fn render_huddle_share_panel(
        &self,
        active_source: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let (card_bg, border, muted, white, emerald, crimson) = (
            rgb(0x222529),
            rgba(0xFFFFFF22),
            rgb(0xABABAD),
            rgb(0xFFFFFF),
            rgb(0x007A5A),
            rgb(0xE01E5A),
        );
        let send = self.send.clone();
        let loopback = slackless_core::huddle::system_audio_source_label();
        v_flex()
            .max_w(px(560.))
            .p_3()
            .gap_2p5()
            .rounded_2xl()
            .border_1()
            .border_color(border)
            .bg(card_bg)
            .shadow_2xl()
            .text_color(white)
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgba(0xFFFFFF0A))
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(if loopback.is_some() { emerald } else { muted })
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
                            .text_color(muted)
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
                                .bg(crimson)
                                .cursor_pointer()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(white)
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
                    .text_color(muted)
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
                            .bg(if selected { emerald } else { rgba(0xFFFFFF12) })
                            .hover(|el| el.bg(if selected { emerald } else { rgba(0xFFFFFF22) }))
                            .text_xs()
                            .text_color(white)
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

        let (card_bg, border, muted, white, emerald) = (
            rgb(0x222529),
            rgba(0xFFFFFF22),
            rgb(0xABABAD),
            rgb(0xFFFFFF),
            rgb(0x007A5A),
        );
        let send = self.send.clone();
        let active_sticky = snap.sticky_emoji.clone();

        v_flex()
            .p_3()
            .gap_2p5()
            .rounded_2xl()
            .border_1()
            .border_color(border)
            .bg(card_bg)
            .shadow_2xl()
            .text_color(white)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .items_center()
                    .child(div().w(px(52.)).text_xs().text_color(muted).child("React"))
                    .children(QUICK.iter().enumerate().map(|(ix, &(glyph, code))| {
                        let em = code.to_owned();
                        let send_em = send.clone();
                        div()
                            .id(("popout-huddle-react", ix))
                            .size(px(32.))
                            .rounded_lg()
                            .cursor_pointer()
                            .hover(|el| el.bg(rgba(0xFFFFFF1C)))
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
                            .text_color(muted)
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
                                    .bg(rgba(0xFFFFFF12))
                                    .hover(|el| el.bg(rgba(0xFFFFFF22)))
                                    .text_xs()
                                    .text_color(white)
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
                    .child(div().w(px(52.)).text_xs().text_color(muted).child("Sticky"))
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
                                    .bg(if selected { emerald } else { rgba(0xFFFFFF12) })
                                    .hover(|el| {
                                        el.bg(if selected { emerald } else { rgba(0xFFFFFF22) })
                                    })
                                    .text_xs()
                                    .text_color(white)
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
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        const THEMES: [(&str, &str, u32); 6] = [
            ("default", "Obsidian", 0x1A1D21),
            ("midnight", "Midnight", 0x10172A),
            ("sunset", "Sunset", 0x2B1818),
            ("forest", "Forest", 0x11221B),
            ("ocean", "Ocean", 0x0F212E),
            ("plum", "Aubergine", 0x24142B),
        ];
        let (card_bg, border, muted, white, emerald) = (
            rgb(0x222529),
            rgba(0xFFFFFF22),
            rgb(0xABABAD),
            rgb(0xFFFFFF),
            rgb(0x2BAC76),
        );
        let send = self.send.clone();
        h_flex()
            .p_3()
            .flex_wrap()
            .gap_2()
            .items_center()
            .rounded_2xl()
            .border_1()
            .border_color(border)
            .bg(card_bg)
            .shadow_2xl()
            .text_color(white)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .mr_1()
                    .child("Theme"),
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
                    .bg(if selected {
                        rgba(0xFFFFFF24)
                    } else {
                        rgba(0xFFFFFF10)
                    })
                    .border_1()
                    .border_color(if selected { emerald } else { rgba(0xFFFFFF18) })
                    .hover(|el| el.bg(rgba(0xFFFFFF20)))
                    .text_xs()
                    .text_color(white)
                    .child(
                        div()
                            .size(px(12.))
                            .rounded_full()
                            .bg(rgb(hex))
                            .border_1()
                            .border_color(rgba(0xFFFFFF44)),
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
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let (card_bg, border, muted, white) = (
            rgb(0x222529),
            rgba(0xFFFFFF22),
            rgb(0xABABAD),
            rgb(0xFFFFFF),
        );
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
        v_flex()
            .max_w(px(520.))
            .p_3()
            .gap_2()
            .rounded_2xl()
            .border_1()
            .border_color(border)
            .bg(card_bg)
            .shadow_2xl()
            .text_color(white)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
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
                            .bg(rgba(0xFFFFFF12))
                            .hover(|el| el.bg(rgba(0xFFFFFF22)))
                            .text_xs()
                            .text_color(white)
                            .child(
                                gpui_kit::component::Icon::new(IconName::UserPlus)
                                    .size(px(12.))
                                    .text_color(muted),
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
        _cx: &Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let (card_bg, border, muted, white, emerald) = (
            rgb(0x222529),
            rgba(0xFFFFFF22),
            rgb(0xABABAD),
            rgb(0xFFFFFF),
            rgb(0x007A5A),
        );
        let send_in = self.send.clone();
        let send_out = self.send.clone();
        v_flex()
            .max_w(px(540.))
            .p_3()
            .gap_2()
            .rounded_2xl()
            .border_1()
            .border_color(border)
            .bg(card_bg)
            .shadow_2xl()
            .text_color(white)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
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
                                .bg(if selected { emerald } else { rgba(0xFFFFFF12) })
                                .hover(|el| {
                                    el.bg(if selected { emerald } else { rgba(0xFFFFFF22) })
                                })
                                .text_xs()
                                .text_color(white)
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
                    .text_color(muted)
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
                                .bg(if selected { emerald } else { rgba(0xFFFFFF12) })
                                .hover(|el| {
                                    el.bg(if selected { emerald } else { rgba(0xFFFFFF22) })
                                })
                                .text_xs()
                                .text_color(white)
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

    fn render_huddle_media_controls(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;
        use slackless_core::huddle::list_screen_sources;

        let (muted_now, video_now, blur_now, sharing_now) =
            (snap.muted, snap.video, snap.blur_background, snap.sharing);
        let (dark_btn, dark_hover, white, obsidian, crimson, emerald) = (
            rgb(0x2E3136),
            rgb(0x3B3F46),
            rgb(0xFFFFFF),
            rgb(0x1A1D21),
            rgb(0xE01E5A),
            rgb(0x007A5A),
        );

        let send_mute = self.send.clone();
        let next_muted = !muted_now;
        let mic_btn = div()
            .id("popout-huddle-mic")
            .w(px(44.))
            .h(px(40.))
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if muted_now { crimson } else { dark_btn })
            .hover(|el| el.bg(if muted_now { rgb(0xC91A50) } else { dark_hover }))
            .child(
                Icon::new(if muted_now {
                    IconName::MicOff
                } else {
                    IconName::Mic
                })
                .size(px(17.))
                .text_color(white),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                (send_mute)(Command::SetHuddleMuted(next_muted));
                if this.fixtures.sample {
                    if let Some(dock) = this.huddle_dock_entity() {
                        dock.update(cx, |d, cx| d.set_muted_local(next_muted, cx));
                    }
                }
            }));

        let send_cam = self.send.clone();
        let next_cam = !video_now;
        let cam_btn = div()
            .id("popout-huddle-video")
            .w(px(44.))
            .h(px(40.))
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if video_now { white } else { dark_btn })
            .hover(|el| el.bg(if video_now { rgb(0xE8E8E8) } else { dark_hover }))
            .child(
                Icon::new(if video_now {
                    IconName::Video
                } else {
                    IconName::VideoOff
                })
                .size(px(17.))
                .text_color(if video_now { obsidian } else { white }),
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
            .h(px(40.))
            .px_3()
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if blur_now { white } else { dark_btn })
            .hover(|el| el.bg(if blur_now { rgb(0xE8E8E8) } else { dark_hover }))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(if blur_now { obsidian } else { white })
            .child(if blur_now { "Blur on" } else { "Blur" })
            .on_click(cx.listener(move |this, _, _, cx| {
                (send_blur)(Command::SetHuddleBlurBackground(next_blur));
                if this.fixtures.sample {
                    if let Some(dock) = this.huddle_dock_entity() {
                        dock.update(cx, |d, cx| d.set_blur_local(next_blur, cx));
                    }
                }
            }));

        let share_btn = div()
            .id("popout-huddle-share")
            .w(px(44.))
            .h(px(40.))
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if sharing_now { emerald } else { dark_btn })
            .hover(|el| {
                el.bg(if sharing_now {
                    rgb(0x2BAC76)
                } else {
                    dark_hover
                })
            })
            .child(
                Icon::new(IconName::ScreenShare)
                    .size(px(17.))
                    .text_color(white),
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

        h_flex()
            .gap_2()
            .items_center()
            .child(mic_btn)
            .child(cam_btn)
            .when(video_now, |bar| bar.child(blur_btn))
            .child(share_btn)
    }

    fn render_huddle_collab_controls(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let (dark_btn, dark_hover, white, obsidian, amber) = (
            rgb(0x2E3136),
            rgb(0x3B3F46),
            rgb(0xFFFFFF),
            rgb(0x1A1D21),
            rgb(0xECB22E),
        );
        let hand_now = snap.hand_raised;
        let captions_now = snap.captions_enabled;
        let music_on = snap.jukebox_song.is_some();

        let send_hand = self.send.clone();
        let next_hand = !hand_now;
        let hand_btn = div()
            .id("popout-huddle-hand")
            .w(px(44.))
            .h(px(40.))
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if hand_now { amber } else { dark_btn })
            .hover(|el| el.bg(if hand_now { amber } else { dark_hover }))
            .child(
                gpui_kit::component::Icon::new(IconName::Hand)
                    .size(px(17.))
                    .text_color(if hand_now { obsidian } else { white }),
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
            .w(px(44.))
            .h(px(40.))
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if self.huddle_reactions_open {
                dark_hover
            } else {
                dark_btn
            })
            .hover(|el| el.bg(dark_hover))
            .text_base()
            .child("😊")
            .on_click(cx.listener(|this, _, _, cx| {
                let next = !this.huddle_reactions_open;
                this.close_huddle_popovers();
                this.huddle_reactions_open = next;
                cx.notify();
            }));

        let send_cc = self.send.clone();
        let next_cc = !captions_now;
        let cc_btn = div()
            .id("popout-huddle-cc")
            .w(px(44.))
            .h(px(40.))
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if captions_now { white } else { dark_btn })
            .hover(|el| {
                el.bg(if captions_now {
                    rgb(0xE8E8E8)
                } else {
                    dark_hover
                })
            })
            .child(
                gpui_kit::component::Icon::new(IconName::Captions)
                    .size(px(17.))
                    .text_color(if captions_now { obsidian } else { white }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                (send_cc)(Command::SetHuddleCaptions(next_cc));
                if this.fixtures.sample {
                    if let Some(dock) = this.huddle_dock_entity() {
                        dock.update(cx, |d, cx| d.set_captions_local(next_cc, cx));
                    }
                }
            }));

        let send_music = self.send.clone();
        let next_song = (!music_on).then(|| "lofi".to_owned());
        let music_btn = div()
            .id("popout-huddle-music")
            .w(px(44.))
            .h(px(40.))
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if music_on { rgb(0x611F69) } else { dark_btn })
            .hover(|el| el.bg(if music_on { rgb(0x7A2A84) } else { dark_hover }))
            .child(
                gpui_kit::component::Icon::new(IconName::Music)
                    .size(px(16.))
                    .text_color(white),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                (send_music)(Command::SetHuddleJukebox(next_song.clone()));
                if this.fixtures.sample {
                    if let Some(dock) = this.huddle_dock_entity() {
                        dock.update(cx, |d, cx| d.set_jukebox_local(next_song.clone(), cx));
                    }
                }
            }));

        h_flex()
            .gap_2()
            .items_center()
            .child(react_btn)
            .child(hand_btn)
            .child(cc_btn)
            .child(music_btn)
            .child(self.render_huddle_utility_controls(cx))
    }

    fn render_huddle_utility_controls(&self, cx: &mut Context<Self>) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;
        use slackless_core::huddle::list_audio_devices;

        let (dark_btn, dark_hover, white, crimson) =
            (rgb(0x2E3136), rgb(0x3B3F46), rgb(0xFFFFFF), rgb(0xE01E5A));
        let send_leave = self.send.clone();
        h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .id("popout-huddle-theme-btn")
                    .w(px(44.))
                    .h(px(40.))
                    .rounded_full()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if self.huddle_theme_open {
                        dark_hover
                    } else {
                        dark_btn
                    })
                    .hover(|el| el.bg(dark_hover))
                    .child(
                        Icon::new(IconName::Sparkles)
                            .size(px(16.))
                            .text_color(white),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        let next = !this.huddle_theme_open;
                        this.close_huddle_popovers();
                        this.huddle_theme_open = next;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("popout-huddle-devices-btn")
                    .w(px(44.))
                    .h(px(40.))
                    .rounded_full()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if self.huddle_devices_open {
                        dark_hover
                    } else {
                        dark_btn
                    })
                    .hover(|el| el.bg(dark_hover))
                    .child(
                        Icon::new(IconName::SlidersHorizontal)
                            .size(px(16.))
                            .text_color(white),
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
            )
            .child(div().w(px(1.)).h(px(24.)).mx_1().bg(rgba(0xFFFFFF1C)))
            .child(
                h_flex()
                    .id("popout-huddle-leave")
                    .h(px(40.))
                    .px_4()
                    .gap_2()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(crimson)
                    .hover(|el| el.bg(rgb(0xC91A50)))
                    .cursor_pointer()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(white)
                    .child(
                        Icon::new(IconName::PhoneOff)
                            .size(px(15.))
                            .text_color(white),
                    )
                    .child("Leave")
                    .on_click(move |_, window, _| {
                        (send_leave)(Command::LeaveHuddle);
                        window.remove_window();
                    }),
            )
    }

    fn render_huddle_controls(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        bar_bg: Rgba,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        h_flex()
            .flex_none()
            .h(px(72.))
            .px_4()
            .gap_2()
            .items_center()
            .justify_center()
            .bg(bar_bg)
            .border_t_1()
            .border_color(rgba(0xFFFFFF10))
            .child(self.render_huddle_media_controls(snap, cx))
            .child(self.render_huddle_collab_controls(snap, cx))
    }
}

fn huddle_theme_surface(bg_id: Option<&str>) -> (Rgba, Rgba) {
    match bg_id {
        Some("midnight") => (rgb(0x10172A), rgb(0x1B2440)),
        Some("sunset") => (rgb(0x2B1818), rgb(0x3B2222)),
        Some("forest") => (rgb(0x11221B), rgb(0x1B3328)),
        Some("ocean") => (rgb(0x0F212E), rgb(0x183244)),
        Some("plum") => (rgb(0x24142B), rgb(0x351F3E)),
        _ => (rgb(0x1A1D21), rgb(0x23262B)),
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
