//! Native huddle lifecycle, dock/floating panel, audio/video toggles, and invites.

use super::super::*;

impl App {
    /// Slack's huddle link for the open conversation (its web client runs the call).
    pub(in crate::app) fn huddle_link(&self) -> String {
        let team = self
            .fixtures
            .workspaces
            .first()
            .map(|team| team.id.to_string())
            .unwrap_or_default();
        format!("https://app.slack.com/huddle/{team}/{}", self.channel)
    }

    /// The header's huddle button: toggles the sample huddle bar/dock in sample mode, or starts /
    /// joins a native Chime audio huddle in live workspaces (with a confirmation warning on
    /// channels with 150+ members, `F-COLLAB-055`).
    pub(in crate::app) fn start_huddle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let has_fixture_bar = self
            .huddle
            .as_ref()
            .is_some_and(|(channel, _)| *channel == self.channel);
        if has_fixture_bar && self.fixtures.sample {
            self.huddle_open = !self.huddle_open;
            if self.huddle_open {
                let settings = self.preferences.read(cx).settings().clone();
                let snap = slackless_core::huddle::HuddleSessionSnapshot {
                    phase: slackless_core::huddle::HuddlePhase::Connected,
                    channel: self.channel.clone(),
                    call_id: "R0SAMPLE".to_owned(),
                    muted: settings.huddle_join_muted,
                    started_at_secs: crate::drafts::now_secs(),
                    huddle_link: Some(self.huddle_link()),
                    input_device: settings.huddle_input_device,
                    output_device: settings.huddle_output_device,
                    aec: settings.huddle_aec,
                    ns: settings.huddle_ns,
                    agc: settings.huddle_agc,
                    ..Default::default()
                };
                self.mount_huddle_dock(snap, window, cx);
            } else {
                self.clear_huddle_dock(cx);
            }
            cx.notify();
            return;
        }
        // Already in a huddle in this channel: pop out the huddle window for quick access.
        if self
            .huddle_session
            .as_ref()
            .is_some_and(|s| s.channel == self.channel)
        {
            self.pop_out_huddle(window, cx);
            return;
        }
        let member_count = self
            .ws
            .member_counts
            .get(&self.channel)
            .copied()
            .or_else(|| {
                self.fixtures
                    .conversation(&self.channel)
                    .and_then(|c| c.num_members)
            })
            .unwrap_or(0);
        let is_channel = self.fixtures.conversation(&self.channel).is_some_and(|c| {
            matches!(
                c.kind(),
                ConversationKind::PublicChannel | ConversationKind::PrivateChannel
            )
        });
        let target_channel = self.channel.clone();
        if is_channel && member_count >= 150 {
            let label = self.label(&self.channel);
            let this = cx.weak_entity();
            window.open_dialog(cx, move |dialog, _, cx| {
                let muted = cx.theme().muted_foreground;
                let this = this.clone();
                let ch = target_channel.clone();
                dialog
                    .title(format!("Start a huddle in {label}?"))
                    .w(px(440.))
                    .child(
                        v_flex()
                            .gap_4()
                            .child(div().text_color(muted).child(format!(
                                "This channel has {member_count} members. Starting a huddle may notify everyone in {label}."
                            )))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .justify_end()
                                    .child(
                                        preferences::form_button(
                                            Button::new("huddle-warn-cancel")
                                                .outline()
                                                .label("Cancel"),
                                            cx,
                                        )
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                    )
                                    .child(
                                        preferences::form_button(
                                            Button::new("huddle-warn-go")
                                                .primary()
                                                .label("Start huddle"),
                                            cx,
                                        )
                                        .on_click(move |_, window, cx| {
                                            window.close_dialog(cx);
                                            let _ = this.update(cx, |app, cx| {
                                                app.join_huddle_in(ch.clone(), window, cx);
                                            });
                                        }),
                                    ),
                            ),
                    )
            });
        } else {
            self.join_huddle_in(target_channel, window, cx);
        }
    }

    /// Join or start a native Chime audio huddle in `channel`.
    pub(in crate::app) fn join_huddle_in(
        &mut self,
        channel: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_huddle_invite(cx);
        let settings = self.preferences.read(cx).settings().clone();
        let audio = slackless_core::huddle::HuddleAudioConfig {
            muted: settings.huddle_join_muted,
            input_device: settings.huddle_input_device.clone(),
            output_device: settings.huddle_output_device.clone(),
            camera_device: settings.huddle_camera_device,
            blur_background: settings.huddle_blur_background,
            aec: settings.huddle_aec,
            ns: settings.huddle_ns,
            agc: settings.huddle_agc,
            transcription_provider: settings.transcription_provider(),
        };
        let initial = slackless_core::huddle::HuddleSessionSnapshot {
            phase: if self.fixtures.sample {
                slackless_core::huddle::HuddlePhase::Connected
            } else {
                slackless_core::huddle::HuddlePhase::Joining
            },
            channel: channel.clone(),
            muted: audio.muted,
            blur_background: audio.blur_background,
            captions_enabled: settings.huddle_auto_captions,
            started_at_secs: crate::drafts::now_secs(),
            input_device: audio.input_device.clone(),
            output_device: audio.output_device.clone(),
            aec: audio.aec,
            ns: audio.ns,
            agc: audio.agc,
            ..Default::default()
        };
        self.mount_huddle_dock(initial, window, cx);
        if self.fixtures.sample {
            self.play_huddle_chime(slackless_core::huddle::HuddleChime::Join, cx);
        } else {
            self.bridge.send(Command::JoinHuddle {
                channel: channel.clone(),
                audio,
            });
        }
        if settings.huddle_auto_open_window {
            self.pop_out_huddle(window, cx);
        }
        cx.notify();
    }

    pub(in crate::app) fn mount_huddle_dock(
        &mut self,
        snap: slackless_core::huddle::HuddleSessionSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let participants = self
            .ws
            .active_huddles
            .get(&snap.channel)
            .cloned()
            .or_else(|| {
                self.fixtures.sample.then(|| {
                    self.fixtures
                        .users
                        .iter()
                        .map(|u| u.id.clone())
                        .filter(|id| *id != self.fixtures.self_user)
                        .take(3)
                        .collect()
                })
            })
            .unwrap_or_default();
        self.huddle_session = Some(snap.clone());
        let dock = crate::huddle_dock::HuddleDock::new(
            self.fixtures.clone(),
            snap.clone(),
            participants,
            cx,
        );
        let sub = cx.subscribe_in(
            &dock,
            window,
            |this: &mut App, _, event: &crate::huddle_dock::HuddleDockEvent, window, cx| match event
            {
                crate::huddle_dock::HuddleDockEvent::ToggleMute => this.toggle_huddle_mute(cx),
                crate::huddle_dock::HuddleDockEvent::ToggleCamera => this.toggle_huddle_camera(cx),
                crate::huddle_dock::HuddleDockEvent::ToggleScreenShare => {
                    this.toggle_huddle_screen_share(window, cx);
                }
                crate::huddle_dock::HuddleDockEvent::Leave => this.leave_huddle(cx),
                crate::huddle_dock::HuddleDockEvent::PopOut => this.pop_out_huddle(window, cx),
                crate::huddle_dock::HuddleDockEvent::OpenFloatingPanel => {
                    this.open_huddle_floating_panel(window, cx);
                }
                crate::huddle_dock::HuddleDockEvent::CloseFloatingPanel => {
                    this.close_huddle_floating_panel(cx);
                }
                crate::huddle_dock::HuddleDockEvent::SendDraw(msg) => {
                    if !this.fixtures.sample {
                        this.bridge.send(Command::SendHuddleDraw(msg.clone()));
                    }
                }
                crate::huddle_dock::HuddleDockEvent::JumpToChannel(ch) => {
                    this.select(ch.clone(), cx);
                }
                crate::huddle_dock::HuddleDockEvent::OpenThread => this.toggle_huddle_thread(cx),
                crate::huddle_dock::HuddleDockEvent::OpenCanvas(fid) => {
                    this.open_huddle_canvas(fid.clone(), cx);
                }
                crate::huddle_dock::HuddleDockEvent::CopyLink(ch) => {
                    this.copy_huddle_link(ch.clone(), window, cx);
                }
                crate::huddle_dock::HuddleDockEvent::OpenBrowser(url) => cx.open_url(url),
                crate::huddle_dock::HuddleDockEvent::RetryJoin(ch) => {
                    this.join_huddle_in(ch.clone(), window, cx);
                }
                crate::huddle_dock::HuddleDockEvent::OpenAudioSettings => {
                    this.open_preferences(Some(Section::AudioVideo), window, cx);
                }
                crate::huddle_dock::HuddleDockEvent::SwitchInputDevice(input) => {
                    let output = this
                        .huddle_session
                        .as_ref()
                        .and_then(|s| s.output_device.clone());
                    this.bridge.send(Command::SetHuddleAudioDevices {
                        input: input.clone(),
                        output,
                    });
                }
                crate::huddle_dock::HuddleDockEvent::SwitchOutputDevice(output) => {
                    let input = this
                        .huddle_session
                        .as_ref()
                        .and_then(|s| s.input_device.clone());
                    this.bridge.send(Command::SetHuddleAudioDevices {
                        input,
                        output: output.clone(),
                    });
                }
            },
        );
        self.huddle_dock = Some(dock);
        self._huddle_dock_sub = Some(sub);
        self.sidebar
            .update(cx, |s, cx| s.set_self_in_huddle(true, cx));
        if let Some((ch, bar)) = &self.huddle
            && *ch == snap.channel
        {
            bar.update(cx, |b, cx| b.update_session(Some(snap), cx));
        }
    }

    pub(in crate::app) fn play_huddle_chime(
        &self,
        chime: slackless_core::huddle::HuddleChime,
        cx: &gpui_kit::gpui::App,
    ) {
        #[cfg(test)]
        {
            let _ = (chime, cx);
        }
        #[cfg(not(test))]
        {
            let settings = self.preferences.read(cx).settings();
            if settings.mute_sounds || !settings.huddle_sounds {
                return;
            }
            let output = settings.huddle_output_device.clone();
            std::thread::spawn(move || {
                let _ = slackless_core::huddle::play_huddle_chime(chime, output.as_deref());
            });
        }
    }

    /// Arm Official Slack's 60-second solo-huddle lounge music timer (`huddles_play_music_timeout`).
    /// Starts Smooth Jazz (`waiting_muzak-9187b04.mp3`) if the user stays alone for 60 seconds,
    /// and cancels automatically if another participant joins or the user leaves the huddle.
    pub(in crate::app) fn arm_huddle_alone_music(
        &mut self,
        channel: ChannelId,
        cx: &mut Context<Self>,
    ) {
        if self.fixtures.sample || !self.preferences.read(cx).settings().huddle_music_when_alone {
            self.huddle_alone_music_task = None;
            return;
        }
        if self.huddle_alone_music_task.is_some() {
            return;
        }
        self.huddle_alone_music_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(60))
                .await;
            let _ = this.update(cx, |app, cx| {
                app.huddle_alone_music_task = None;
                let settings = app.preferences.read(cx).settings();
                if !settings.huddle_music_when_alone || settings.mute_sounds {
                    return;
                }
                let Some(snap) = app.huddle_session.as_ref() else {
                    return;
                };
                if snap.channel != channel
                    || snap.phase != slackless_core::huddle::HuddlePhase::Connected
                    || snap.jukebox_song.is_some()
                {
                    return;
                }
                let peer_count = app.ws.active_huddles.get(&channel).map_or(0, Vec::len);
                if peer_count <= 1 {
                    app.bridge
                        .send(Command::SetHuddleJukebox(Some("jazz".to_owned())));
                }
            });
        }));
    }

    pub(in crate::app) fn open_huddle_floating_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(existing) = self.huddle_floating_panel {
            if existing
                .update(cx, |_, win, _| win.activate_window())
                .is_ok()
            {
                return;
            }
            self.huddle_floating_panel = None;
        }
        let Some(dock) = &self.huddle_dock else {
            return;
        };
        let display_id = window.display(cx).map(|d| d.id());
        self.huddle_floating_panel =
            crate::popout::open_huddle_floating_panel(dock.downgrade(), display_id, cx);
    }

    pub(in crate::app) fn close_huddle_floating_panel(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.huddle_floating_panel.take() {
            let _ = handle.update(cx, |_, win, _| win.remove_window());
        }
    }

    pub(in crate::app) fn clear_huddle_dock(&mut self, cx: &mut Context<Self>) {
        self.close_huddle_floating_panel(cx);
        self.huddle_alone_music_task = None;
        if let Some(dock) = &self.huddle_dock {
            dock.update(cx, |d, cx| d.clear_video_frames(cx));
        }
        self.huddle_session = None;
        self.huddle_dock = None;
        self._huddle_dock_sub = None;
        self.sidebar
            .update(cx, |s, cx| s.set_self_in_huddle(false, cx));
        if let Some((_, bar)) = &self.huddle {
            bar.update(cx, |b, cx| b.update_session(None, cx));
        }
    }

    pub(in crate::app) fn leave_huddle(&mut self, cx: &mut Context<Self>) {
        let was_active = self.huddle_session.is_some() || self.huddle_open;
        self.huddle_open = false;
        self.clear_huddle_dock(cx);
        if was_active {
            self.play_huddle_chime(slackless_core::huddle::HuddleChime::Leave, cx);
        }
        if !self.fixtures.sample {
            self.bridge.send(Command::LeaveHuddle);
        }
        cx.notify();
    }

    pub(in crate::app) fn toggle_huddle_mute(&mut self, cx: &mut Context<Self>) {
        let Some(snap) = self.huddle_session.as_mut() else {
            return;
        };
        let next = !snap.muted;
        snap.muted = next;
        if let Some(dock) = &self.huddle_dock {
            dock.update(cx, |d, cx| d.set_muted_local(next, cx));
        }
        if let Some((ch, bar)) = &self.huddle
            && *ch == snap.channel
        {
            let s = snap.clone();
            bar.update(cx, |b, cx| b.update_session(Some(s), cx));
        }
        if !self.fixtures.sample {
            self.bridge.send(Command::SetHuddleMuted(next));
        }
        cx.notify();
    }

    pub(in crate::app) fn toggle_huddle_camera(&mut self, cx: &mut Context<Self>) {
        let settings = self.preferences.read(cx).settings().clone();
        let Some(snap) = self.huddle_session.as_mut() else {
            return;
        };
        let next = !snap.video;
        snap.video = next;
        snap.blur_background = settings.huddle_blur_background;
        if let Some(dock) = &self.huddle_dock {
            dock.update(cx, |d, cx| d.set_camera_local(next, cx));
        }
        if let Some((ch, bar)) = &self.huddle
            && *ch == snap.channel
        {
            let s = snap.clone();
            bar.update(cx, |b, cx| b.update_session(Some(s), cx));
        }
        if !self.fixtures.sample {
            self.bridge.send(Command::SetHuddleCamera {
                enabled: next,
                device_index: settings.huddle_camera_device,
                blur: settings.huddle_blur_background,
            });
        }
        cx.notify();
    }

    pub(in crate::app) fn toggle_huddle_screen_share(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_sharing = self.huddle_session.as_ref().is_some_and(|s| s.sharing);
        if is_sharing {
            if let Some(snap) = self.huddle_session.as_mut() {
                snap.sharing = false;
                snap.share_source_id = None;
            }
            if let Some(dock) = &self.huddle_dock {
                dock.update(cx, |d, cx| d.set_sharing_local(None, cx));
            }
            if !self.fixtures.sample {
                self.bridge.send(Command::SetHuddleScreenShare {
                    source_id: None,
                    share_audio: false,
                });
            }
            cx.notify();
        } else {
            // Open the full Huddle Window so the user can pick a monitor or window to share.
            self.pop_out_huddle(window, cx);
        }
    }

    pub(in crate::app) fn copy_huddle_link(
        &mut self,
        channel: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.fixtures.sample {
            let url = self.huddle_link();
            cx.write_to_clipboard(ClipboardItem::new_string(url));
            notify("Huddle link copied", String::new(), window, cx);
        } else {
            self.bridge.send(Command::CopyHuddleLink { channel });
        }
    }

    pub(in crate::app) fn open_huddle_canvas(&mut self, file_id: String, cx: &mut Context<Self>) {
        self.ws.active_canvas = Some(file_id.clone());
        self.tab = ConversationTab::Canvas;
        if !self.fixtures.sample {
            self.bridge.send(Command::LoadCanvas { file_id });
        }
        cx.notify();
    }

    pub(in crate::app) fn pop_out_huddle(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dock) = &self.huddle_dock else {
            return;
        };
        let ch = dock.read(cx).session().channel.clone();
        let title = format!("Huddle — {}", self.label(&ch));
        let on_jump = self.popout_jump_handler(window, cx);
        let this = cx.weak_entity();
        let on_open_canvas: crate::popout::CanvasOpenHandler = Rc::new(move |fid, cx| {
            let _ = this.update(cx, |app, cx| app.open_huddle_canvas(fid, cx));
        });
        let display_id = window.display(cx).map(|d| d.id());
        crate::popout::open_popout(
            crate::popout::PopoutKind::Huddle {
                source: dock.downgrade(),
                on_jump,
                on_open_canvas,
            },
            title,
            self.fixtures.clone(),
            self.bridge.store().cloned(),
            self.bridge.team_id().cloned(),
            self.bridge.forwarder(),
            None,
            display_id,
            cx,
        );
    }

    pub(in crate::app) fn show_huddle_invite(
        &mut self,
        invite: crate::huddle_invite::PendingHuddleInvite,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let call_id = invite.call_id.clone();
        let channel = invite.channel.clone();
        let banner =
            crate::huddle_invite::HuddleInviteBanner::new(self.fixtures.clone(), invite, cx);
        let decl_ch = channel.clone();
        let decl_call = call_id.clone();
        let sub = cx.subscribe_in(
            &banner,
            window,
            move |this: &mut App,
                  _,
                  event: &crate::huddle_invite::HuddleInviteEvent,
                  window,
                  cx| {
                match event {
                    crate::huddle_invite::HuddleInviteEvent::Accept(ch) => {
                        this.dismiss_huddle_invite(cx);
                        this.select(ch.clone(), cx);
                        this.join_huddle_in(ch.clone(), window, cx);
                    }
                    crate::huddle_invite::HuddleInviteEvent::Decline => {
                        this.dismiss_huddle_invite(cx);
                        if !this.fixtures.sample {
                            this.bridge.send(Command::DeclineHuddleInvite {
                                channel: decl_ch.clone(),
                                call_id: decl_call.clone(),
                            });
                        }
                    }
                }
            },
        );
        self.huddle_invite = Some(banner);
        self._huddle_invite_sub = Some(sub);
        // Play initial ringtone + repeat every 3 seconds until accepted, declined, or expired.
        self.play_huddle_chime(slackless_core::huddle::HuddleChime::InviteRing, cx);
        let ring_ch = channel.clone();
        let ring_call = call_id.clone();
        cx.spawn(async move |this, cx| {
            for _ in 0..14 {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let still_ringing = this
                    .update(cx, |app, cx| {
                        let same = app.huddle_invite.as_ref().is_some_and(|b| {
                            let inv = b.read(cx).invite();
                            inv.channel == ring_ch && inv.call_id == ring_call
                        });
                        if same {
                            app.play_huddle_chime(
                                slackless_core::huddle::HuddleChime::InviteRing,
                                cx,
                            );
                        }
                        same
                    })
                    .unwrap_or(false);
                if !still_ringing {
                    break;
                }
            }
        })
        .detach();
        // Auto-dismiss unanswered invite after 45 seconds.
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(45))
                .await;
            let _ = this.update(cx, |app, cx| {
                let still_same = app.huddle_invite.as_ref().is_some_and(|b| {
                    let inv = b.read(cx).invite();
                    inv.channel == channel && inv.call_id == call_id
                });
                if still_same {
                    app.dismiss_huddle_invite(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::app) fn dismiss_huddle_invite(&mut self, cx: &mut Context<Self>) {
        if self.huddle_invite.take().is_some() {
            self._huddle_invite_sub = None;
            cx.notify();
        }
    }
    /// The Huddles page: the bundled sample huddle, or the stored `huddle_thread` trails live.
    pub(in crate::app) fn open_huddles(&mut self, cx: &mut Context<Self>) {
        self.record_nav(&NavPlace::Huddles, cx);
        self.show_huddles(cx);
    }

    pub(in crate::app) fn show_huddles(&mut self, cx: &mut Context<Self>) {
        self.save_draft(cx);
        let fixtures = self.fixtures.clone();
        if let Some(store) = self.bridge.store().cloned() {
            // Live mode: keep cached rows visible while refreshing from SQLite.
            self.huddles
                .update(cx, |view, cx| view.begin_live_load(fixtures, cx));
            let team = self.fixtures.workspaces[self.workspace].id.clone();
            cx.spawn(async move |this, cx| {
                let huddles = cx
                    .background_executor()
                    .spawn(async move {
                        let Ok(store) = store.lock() else {
                            return Vec::new();
                        };
                        huddles::store_huddles(&store, &team)
                    })
                    .await;
                this.update(cx, |app, cx| {
                    app.huddles
                        .update(cx, |view, cx| view.set_huddles(huddles, cx));
                })
            })
            .detach();
        } else {
            self.huddles
                .update(cx, |view, cx| view.show_local(&fixtures, cx));
        }
        self.close_full_column_views(cx);
        self.huddles_open = true;
        self.sidebar.update(cx, |s, cx| {
            s.set_home_page(Some(crate::sidebar::HomePage::Huddles), cx)
        });
        cx.notify();
    }
    pub(in crate::app) fn huddle_here(&self) -> bool {
        self.huddle
            .as_ref()
            .is_some_and(|(channel, _)| *channel == self.channel)
            || self.ws.active_huddles.contains_key(&self.channel)
    }
}
