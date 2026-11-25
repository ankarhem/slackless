//! Right-hand companion drawers inside the Pop-out Huddle Window (Thread, Notes, Diagnostics).

use super::*;

impl PopoutWindow {
    pub(super) fn render_huddle_drawer(
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

    pub(in crate::popout) fn resolve_huddle_thread_root(
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

    pub(in crate::popout) fn huddle_thread_messages(
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
    pub(in crate::popout) fn prune_echoed_huddle_locals(
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
                                                if this.fixtures.sample
                                                    && let Some(dock) = this.huddle_dock_entity()
                                                {
                                                    dock.update(cx, |d, cx| {
                                                        d.set_audio_only_local(next, cx)
                                                    });
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
}
