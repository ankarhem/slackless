//! Huddle-specific `CoreBridge` event handling: low-frequency lifecycle/control [`HuddleEvent`]s
//! and high-frequency [`HuddleMediaEvent`]s drained on their own bounded coalescing lane.

use super::super::*;

impl App {
    pub(in crate::app) fn on_huddle_media_event(
        &mut self,
        event: &HuddleMediaEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.huddle_state.clone() else {
            return;
        };
        match event {
            HuddleMediaEvent::AudioLevels(levels) => {
                state.update(cx, |s, cx| s.update_levels(levels.clone(), cx));
            }
            HuddleMediaEvent::VideoFrame { tile, frame } => {
                state.update(cx, |s, cx| s.update_video_frame(tile.clone(), frame, cx));
            }
            HuddleMediaEvent::VideoTileClosed(tile) => {
                state.update(cx, |s, cx| s.close_video_tile(tile, cx));
            }
        }
    }

    pub(super) fn on_huddle_core_event(
        &mut self,
        event: &HuddleEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            HuddleEvent::Updated {
                channel,
                participants,
                ended,
                topic,
                background_id,
                thread_root_ts,
            } => {
                let prev_participants = self
                    .ws
                    .active_huddles
                    .get(channel)
                    .cloned()
                    .unwrap_or_default();
                if *ended {
                    self.ws.active_huddles.remove(channel);
                } else {
                    self.ws
                        .active_huddles
                        .insert(channel.clone(), participants.clone());
                }
                let huddles = self.ws.active_huddles.clone();
                self.sidebar
                    .update(cx, |sidebar, cx| sidebar.set_huddles(huddles, cx));
                if let Some(state) = self.huddle_state.clone()
                    && state.read(cx).session().channel == *channel
                {
                    let self_uid = &self.fixtures.self_user;
                    let new_peers = if *ended {
                        &[][..]
                    } else {
                        participants.as_slice()
                    };
                    let remote_joined = new_peers
                        .iter()
                        .any(|u| u != self_uid && !prev_participants.contains(u));
                    let remote_left = prev_participants
                        .iter()
                        .any(|u| u != self_uid && !new_peers.contains(u));
                    if remote_joined {
                        self.play_huddle_chime(slackless_core::huddle::HuddleChime::PeerJoin, cx);
                    } else if remote_left {
                        self.play_huddle_chime(slackless_core::huddle::HuddleChime::PeerLeave, cx);
                    }

                    let alone_music = self.preferences.read(cx).settings().huddle_music_when_alone;
                    let playing_jukebox = state.read(cx).session().jukebox_song.is_some();
                    if !*ended && participants.len() > 1 {
                        self.huddle_alone_music_task = None;
                        if alone_music && playing_jukebox {
                            self.bridge.send(HuddleControlCommand::SetJukebox(None));
                        }
                    } else if !*ended
                        && participants.len() <= 1
                        && state.read(cx).session().phase
                            == slackless_core::huddle::HuddlePhase::Connected
                    {
                        self.arm_huddle_alone_music(channel.clone(), cx);
                    }
                    state.update(cx, |s, cx| {
                        s.set_participants(
                            if *ended {
                                Vec::new()
                            } else {
                                participants.clone()
                            },
                            cx,
                        );
                        s.apply_room_meta(
                            topic.clone(),
                            background_id.clone(),
                            thread_root_ts.clone(),
                            cx,
                        );
                    });
                }
                cx.notify();
            }
            HuddleEvent::SessionChanged(snap) => {
                let snap = (**snap).clone();
                let prev_phase = self.huddle_session.as_ref().map(|s| s.phase);
                if snap.phase == slackless_core::huddle::HuddlePhase::Idle {
                    if prev_phase.is_some() {
                        self.play_huddle_chime(slackless_core::huddle::HuddleChime::Leave, cx);
                    }
                    self.clear_huddle_dock(cx);
                } else {
                    let newly_connected = snap.phase
                        == slackless_core::huddle::HuddlePhase::Connected
                        && prev_phase != Some(slackless_core::huddle::HuddlePhase::Connected);
                    self.huddle_session = Some(snap.clone());
                    if let Some(state) = self.huddle_state.clone() {
                        state.update(cx, |s, cx| s.update_session(snap.clone(), cx));
                    } else {
                        self.mount_huddle_dock(snap.clone(), window, cx);
                    }
                    if newly_connected {
                        self.play_huddle_chime(slackless_core::huddle::HuddleChime::Join, cx);
                        let settings = self.preferences.read(cx).settings().clone();
                        if settings.huddle_auto_captions
                            && !settings.transcription_provider().is_local()
                            && !snap.captions_enabled
                            && !self.fixtures.sample
                        {
                            self.bridge.send(HuddleCommand::SetCaptions(true));
                        }
                        let peer_count = self
                            .ws
                            .active_huddles
                            .get(&snap.channel)
                            .map_or(0, Vec::len);
                        if settings.huddle_music_when_alone
                            && peer_count <= 1
                            && snap.jukebox_song.is_none()
                            && !self.fixtures.sample
                        {
                            self.arm_huddle_alone_music(snap.channel.clone(), cx);
                        }
                    }
                }
                cx.notify();
            }
            HuddleEvent::Data(ev) => {
                if let Some(state) = self.huddle_state.clone() {
                    let show_reactions = self
                        .preferences
                        .read(cx)
                        .settings()
                        .huddle_reaction_animations;
                    if show_reactions
                        || !matches!(ev, slackless_core::huddle::HuddleDataEvent::Reaction { .. })
                    {
                        state.update(cx, |s, cx| s.apply_data_event(ev, cx));
                    }
                }
            }
            HuddleEvent::Invite {
                channel,
                inviter,
                call_id,
            } => {
                if self.preferences.read(cx).settings().notify_huddle_invites {
                    self.show_huddle_invite(
                        crate::huddle_invite::PendingHuddleInvite {
                            channel: channel.clone(),
                            inviter: inviter.clone(),
                            call_id: call_id.clone(),
                        },
                        window,
                        cx,
                    );
                    let sender = self.fixtures.user_name(inviter).to_owned();
                    let tag = format!("huddle-invite:{channel}:{call_id}");
                    self.system_notification_kind(
                        &tag,
                        "Incoming huddle",
                        &format!("{sender} invited you to a huddle"),
                        true,
                        cx,
                    );
                }
            }
            HuddleEvent::InviteCancelled { channel, .. } => {
                let should_dismiss = self.huddle_invite.as_ref().is_some_and(|b| {
                    channel.is_none() || channel.as_ref() == Some(&b.read(cx).invite().channel)
                });
                if should_dismiss {
                    self.dismiss_huddle_invite(cx);
                }
            }
            HuddleEvent::LinkLoaded { url, .. } => {
                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                notify("Huddle link copied", url.clone(), window, cx);
            }
            HuddleEvent::VideoTileClosed(tile) => {
                if let Some(state) = self.huddle_state.clone() {
                    state.update(cx, |s, cx| s.close_video_tile(tile, cx));
                }
            }
        }
    }
}
