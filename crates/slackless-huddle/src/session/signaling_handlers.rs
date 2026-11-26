use std::collections::HashSet;
use std::sync::atomic::Ordering;

use slackless_types::UserId;

use super::connected::{SessionRuntime, announce_screen_share_capabilities};
use super::io::{RtcController, SignalSink};
use super::media::MediaDevices;
use super::supervisor::LoopExit;
use super::video_rx::same_remote_video_sources;
use crate::peer_message::{
    CHIME_TRANSCRIPTION_TOPIC, SLACK_DATA_CHANNEL_TOPIC, decode_chime_transcript_frame,
    decode_slack_peer_message,
};
use crate::proto::{
    SdkAudioStreamIdInfoFrame, SdkDataMessagePayload, SdkIndexFrame, SdkSignalFrame,
};
use crate::signaling::{
    RemoteVideoSource, build_audio_control_frame, build_remote_video_update_frame,
    build_subscribe_frame, decode_sdp_answer_with_prev, now_ms, parse_external_user_id,
    parse_index_sources_with_lookup,
};
use crate::video::VideoTileKey;
use crate::{HuddleAudioConfig, HuddleDataEvent, HuddleError, HuddleSessionSnapshot, HuddleUpdate};

pub(super) async fn handle_signal_frame<R: RtcController, S: SignalSink, M: MediaDevices>(
    frame: SdkSignalFrame,
    config: &mut HuddleAudioConfig,
    snap: &mut HuddleSessionSnapshot,
    peer: &R,
    signal: &mut S,
    state: &mut SessionRuntime<'_, M>,
) -> Option<LoopExit> {
    if let Some(pp) = frame.ping_pong
        && signal.reply_ping_if_needed(&pp).await
    {
        snap.pongs_sent = snap.pongs_sent.saturating_add(1);
    }
    if let Some(index) = frame.index {
        apply_index_frame(&index, config, snap, peer, signal, state).await;
    }
    if let Some(ack) = frame.suback {
        for tm in &ack.tracks {
            if let (Some(ssrc), Some(sid)) = (tm.ssrc, tm.stream_id) {
                state.ssrc_to_stream_id.insert(ssrc, sid);
            }
        }
        if let Ok(mut rt) = state.vdec_routing.lock() {
            rt.ssrc_to_stream_id = state.ssrc_to_stream_id.clone();
        }
        tracing::info!(
            tracks = ack.tracks.len(),
            mapped_ssrcs = state.ssrc_to_stream_id.len(),
            "received renegotiation SUBSCRIBE_ACK"
        );
        match decode_sdp_answer_with_prev(
            ack.sdp_answer.as_deref(),
            ack.compressed_sdp_answer.as_deref(),
            state.last_sdp_answer.as_deref(),
        ) {
            Ok(answer) => {
                state.last_sdp_answer = Some(answer.clone());
                if let Err(err) = peer.set_remote_answer(answer).await {
                    tracing::error!(%err, "renegotiation set_remote_answer failed");
                }
            }
            Err(err) => {
                tracing::error!(%err, "renegotiation decode_sdp_answer failed");
            }
        }
    }
    if let Some(dm) = frame.data_message {
        for msg in dm.messages {
            for ev in parse_incoming_data_message(&msg, &state.creds.attendee_id) {
                match &ev {
                    HuddleDataEvent::Topic { topic, .. } => {
                        snap.topic = (!topic.is_empty()).then(|| topic.clone());
                        let _ = state
                            .updates
                            .send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    HuddleDataEvent::MutedByPeer { .. } => {
                        config.muted = true;
                        state.shared.muted.store(true, Ordering::Relaxed);
                        snap.muted = true;
                        let _ = signal.send_frame(&build_audio_control_frame(true)).await;
                        let _ = state
                            .updates
                            .send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    HuddleDataEvent::Jukebox { song_key, .. } => {
                        state.set_jukebox_player(config.output_device.clone(), song_key.as_deref());
                        snap.jukebox_song = song_key.clone();
                        let _ = state
                            .updates
                            .send(HuddleUpdate::State(Box::new(snap.clone())));
                    }
                    _ => {}
                }
                let _ = state.updates.send(HuddleUpdate::DataEvent(ev));
            }
        }
    }
    if let Some(info) = frame.audio_stream_id_info {
        let new_peer_joined = apply_audio_stream_info(info, state);
        // Re-announce desktop draw capabilities + content source details when
        // a new peer joins while we are already sharing so late joiners can
        // immediately draw on our screen share.
        if new_peer_joined
            && snap.sharing
            && let Some(src_id) = snap.share_source_id.as_deref()
        {
            announce_screen_share_capabilities(&state.creds.attendee_id, signal, src_id).await;
        }
    }
    if let Some(meta) = frame.audio_metadata {
        state.remote_speakers.clear();
        for st in meta.attendee_states {
            let Some(sid) = st.audio_stream_id else {
                continue;
            };
            let Some(uid) = state.stream_users.get(&sid).cloned() else {
                continue;
            };
            let is_muted = st.muted.unwrap_or(false);
            if is_muted {
                state.muted_peers.insert(uid.clone());
            } else {
                state.muted_peers.remove(&uid);
                let vol = st.volume.unwrap_or(0) as f32 / 100.0;
                if vol > 0.05 {
                    state.remote_speakers.push((uid, vol.clamp(0.0, 1.0)));
                }
            }
        }
        state
            .remote_speakers
            .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    }
    if let Some(err) = frame.error {
        return Some(LoopExit::Fatal(HuddleError::Protocol {
            status: err.status.unwrap_or(500),
            description: err.description.unwrap_or_default(),
        }));
    }
    None
}

/// Fold one `AUDIO_STREAM_ID_INFO` frame into `state` (attendee mapping, active stream users,
/// muted peers, and dropped-stream cleanup). Shared between buffered handshake frames and live
/// signaling so mute/unmute/drop transitions during handshake never drift.
///
/// Returns `true` if at least one new non-self peer joined in this frame.
pub(super) fn apply_audio_stream_info<M: MediaDevices>(
    info: SdkAudioStreamIdInfoFrame,
    state: &mut SessionRuntime<'_, M>,
) -> bool {
    let mut new_peer_joined = false;
    for stream in info.streams {
        if let (Some(att), Some(uid)) = (
            stream.attendee_id.as_deref(),
            stream
                .external_user_id
                .as_deref()
                .and_then(parse_external_user_id),
        ) {
            let base = att.strip_suffix("#content").unwrap_or(att);
            state.attendee_to_user.insert(base.to_owned(), uid);
        }
        let Some(sid) = stream.audio_stream_id else {
            continue;
        };
        if stream.dropped == Some(true) {
            if let Some(uid) = state.stream_users.remove(&sid) {
                state.muted_peers.remove(&uid);
            }
            continue;
        }
        if let Some(uid) = stream
            .external_user_id
            .as_deref()
            .and_then(parse_external_user_id)
        {
            if state.stream_users.insert(sid, uid.clone()).is_none() && uid != state.self_user {
                new_peer_joined = true;
            }
            if stream.muted == Some(true) {
                state.muted_peers.insert(uid);
            } else {
                state.muted_peers.remove(&uid);
            }
        }
    }
    new_peer_joined
}

pub(super) async fn apply_index_frame<R: RtcController, S: SignalSink, M: MediaDevices>(
    index: &SdkIndexFrame,
    config: &HuddleAudioConfig,
    snap: &HuddleSessionSnapshot,
    peer: &R,
    signal: &mut S,
    state: &mut SessionRuntime<'_, M>,
) {
    // Ignore our own camera and our own `#content` screen share.
    let next_raw: Vec<RemoteVideoSource> = if snap.audio_only {
        Vec::new()
    } else {
        parse_index_sources_with_lookup(index, &state.creds.attendee_id, &state.attendee_to_user)
            .into_iter()
            .filter(|s| s.attendee_id != state.self_content_attendee)
            .collect()
    };

    let next_tiles: HashSet<VideoTileKey> = next_raw
        .iter()
        .map(|s| {
            if s.is_content {
                VideoTileKey::RemoteScreen(s.user_id.clone())
            } else {
                VideoTileKey::RemoteCamera(s.user_id.clone())
            }
        })
        .collect();
    for removed in state.active_remote_tiles.difference(&next_tiles) {
        let _ = state
            .updates
            .send(HuddleUpdate::VideoTileClosed(removed.clone()));
    }

    let cur_raw: Vec<&RemoteVideoSource> = state.remote_sources.iter().map(|(_, s)| s).collect();
    let same_sources = same_remote_video_sources(&cur_raw, &next_raw);

    if !same_sources {
        tracing::info!(
            raw_descriptors = index.sources.len(),
            deduped = next_raw.len(),
            codec_intersection = ?index.supported_receive_codec_intersection,
            sources = ?next_raw,
            "Chime INDEX updated remote video sources"
        );
        if next_raw.is_empty() {
            let removed_mids: Vec<String> = state
                .remote_sources
                .iter()
                .map(|(m, _)| m.clone())
                .collect();
            if !removed_mids.is_empty() {
                let upd = build_remote_video_update_frame(Vec::new(), removed_mids);
                if let Err(err) = signal.send_frame(&upd).await {
                    tracing::warn!(%err, "failed to send REMOTE_VIDEO_UPDATE unsubscribe");
                }
            }
            state.remote_sources.clear();
            if let Ok(mut rt) = state.vdec_routing.lock() {
                rt.remote_sources.clear();
            }
        } else {
            match peer.prepare_video_receive_offer(next_raw.len()).await {
                Ok((offer, mids)) => {
                    let paired: Vec<(String, RemoteVideoSource)> =
                        mids.into_iter().zip(next_raw).collect();
                    tracing::info!(
                        count = paired.len(),
                        mids = ?paired.iter().map(|(m, s)| (m.as_str(), s.stream_id, s.is_content)).collect::<Vec<_>>(),
                        send_video = snap.video,
                        "sending renegotiation SUBSCRIBE for remote video/screen sources"
                    );
                    match build_subscribe_frame(
                        state.creds,
                        &offer,
                        state.wants_compressed,
                        config.muted,
                        snap.video,
                        &paired,
                    ) {
                        Ok(sub_f) => {
                            if let Err(err) = signal.send_frame(&sub_f).await {
                                tracing::error!(%err, "failed to send renegotiation SUBSCRIBE");
                            } else {
                                state.remote_sources = paired.clone();
                                if let Ok(mut rt) = state.vdec_routing.lock() {
                                    rt.remote_sources = paired;
                                }
                            }
                        }
                        Err(err) => {
                            tracing::error!(%err, "failed to build renegotiation SUBSCRIBE");
                        }
                    }
                }
                Err(err) => {
                    tracing::error!(%err, "prepare_video_receive_offer failed");
                }
            }
        }
    }
    state.active_remote_tiles = next_tiles;
}

pub(super) fn parse_incoming_data_message(
    msg: &SdkDataMessagePayload,
    self_attendee_id: &str,
) -> Vec<HuddleDataEvent> {
    let topic = msg.topic.as_deref().unwrap_or("");
    let data = msg.data.as_deref().unwrap_or(&[]);
    let at_ms = msg
        .ingest_time_ns
        .map(|ns| (ns.max(0) as u64) / 1_000_000)
        .unwrap_or_else(now_ms);

    if topic == CHIME_TRANSCRIPTION_TOPIC {
        return decode_chime_transcript_frame(data, at_ms);
    }

    let sender = msg
        .sender_external_user_id
        .as_deref()
        .and_then(parse_external_user_id)
        .unwrap_or_else(|| UserId::from("U0PEER"));
    let sender_att = msg.sender_attendee_id.as_deref().unwrap_or("");

    if topic == SLACK_DATA_CHANNEL_TOPIC {
        return decode_slack_peer_message(&sender, sender_att, self_attendee_id, data, at_ms);
    }

    Vec::new()
}

/// Official Slack `ContentShareDetails.source_type` string expected by Slack desktop/web clients
/// to unlock remote drawing (`["desktop:screen", "desktop:window"]`).
pub(super) fn content_share_source_type(source_id: &str) -> &'static str {
    if source_id.starts_with("monitor:") {
        "desktop:screen"
    } else {
        "desktop:window"
    }
}
