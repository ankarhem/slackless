//! `webrtc-rs` PeerConnection configured specifically for Amazon Chime SDK's SFU constraints:
//! - Offer `SRTP_AES128_CM_HMAC_SHA1_80` first and exclude `AEAD_AES_256_GCM`
//! - Enforce `RTCIceTransportPolicy::Relay`
//! - Use UDP-only TURN URIs from `JOIN_ACK` (`turn:...?transport=udp`)
//! - Opus 48kHz without RED (RFC 2198) + BUNDLEd `recvonly` video placeholder
//! - Full non-trickle SDP offer gathering

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::mpsc;
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{
    MIME_TYPE_AV1, MIME_TYPE_H264, MIME_TYPE_OPUS, MIME_TYPE_VP8, MIME_TYPE_VP9, MediaEngine,
};
use webrtc::api::setting_engine::SettingEngine;
use webrtc::dtls::extension::extension_use_srtp::SrtpProtectionProfile;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::policy::ice_transport_policy::RTCIceTransportPolicy;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtcp::payload_feedbacks::full_intra_request::{FirEntry, FullIntraRequest};
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtp_transceiver::rtp_codec::{
    RTCRtpCodecCapability, RTCRtpCodecParameters, RTCRtpHeaderExtensionCapability, RTPCodecType,
};
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::rtp_transceiver::{RTCPFeedback, RTCRtpTransceiverInit};
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

use crate::proto::SdkTurnCredentials;
use crate::signaling::ClientMetricsSnapshot;
use crate::{HuddleError, Result};

/// Filter Chime `JOIN_ACK` TURN URIs to UDP-only (`webrtc-rs` does not support TURN-over-TLS/TCP).
pub fn filter_udp_turn_uris(uris: &[String]) -> Vec<String> {
    uris.iter()
        .filter(|u| {
            let lower = u.to_ascii_lowercase();
            lower.starts_with("turn:") && !lower.contains("transport=tcp")
        })
        .cloned()
        .collect()
}

/// Build `SettingEngine` with Chime's mandatory SRTP protection profile order.
pub fn chime_setting_engine() -> SettingEngine {
    let mut setting = SettingEngine::default();
    // Chime silently drops packets when AEAD_AES_128_GCM is chosen over AES128_CM_SHA1_80,
    // and webrtc-srtp panics on AEAD_AES_256_GCM.
    setting.set_srtp_protection_profiles(vec![SrtpProtectionProfile::Srtp_Aes128_Cm_Hmac_Sha1_80]);
    setting
}

/// Build the Chime-compatible `MediaEngine` matching official Slack's exact codec order, payload
/// types, RFC 4588 RTX streams, RTCP feedback, and `sdes:mid` RTP header extensions on BOTH audio
/// and video (required for BUNDLEd SFU tracks whose SSRCs arrive dynamically).
pub fn chime_media_engine() -> Result<MediaEngine> {
    let mut m = MediaEngine::default();
    m.register_codec(
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: 48_000,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                rtcp_feedback: vec![RTCPFeedback {
                    typ: "transport-cc".to_owned(),
                    parameter: String::new(),
                }],
            },
            payload_type: 111,
            ..Default::default()
        },
        RTPCodecType::Audio,
    )
    .map_err(|e| HuddleError::WebRtc(e.to_string()))?;

    let video_fb = || {
        vec![
            RTCPFeedback {
                typ: "goog-remb".to_owned(),
                parameter: String::new(),
            },
            RTCPFeedback {
                typ: "transport-cc".to_owned(),
                parameter: String::new(),
            },
            RTCPFeedback {
                typ: "ccm".to_owned(),
                parameter: "fir".to_owned(),
            },
            RTCPFeedback {
                typ: "nack".to_owned(),
                parameter: String::new(),
            },
            RTCPFeedback {
                typ: "nack".to_owned(),
                parameter: "pli".to_owned(),
            },
        ]
    };

    for (mime, pt, fmtp) in [
        (MIME_TYPE_AV1, 45u8, ""),
        (
            MIME_TYPE_H264,
            108u8,
            "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f",
        ),
        (MIME_TYPE_VP8, 96u8, ""),
        (MIME_TYPE_VP9, 98u8, "profile-id=0"),
    ] {
        m.register_codec(
            RTCRtpCodecParameters {
                capability: RTCRtpCodecCapability {
                    mime_type: mime.to_owned(),
                    clock_rate: 90_000,
                    channels: 0,
                    sdp_fmtp_line: fmtp.to_owned(),
                    rtcp_feedback: video_fb(),
                },
                payload_type: pt,
                ..Default::default()
            },
            RTPCodecType::Video,
        )
        .map_err(|e| HuddleError::WebRtc(e.to_string()))?;
    }

    for (primary, rtx_pt) in [(45u8, 46u8), (108, 109), (96, 97), (98, 99)] {
        m.register_codec(
            RTCRtpCodecParameters {
                capability: RTCRtpCodecCapability {
                    mime_type: "video/rtx".to_owned(),
                    clock_rate: 90_000,
                    channels: 0,
                    sdp_fmtp_line: format!("apt={primary}"),
                    rtcp_feedback: Vec::new(),
                },
                payload_type: rtx_pt,
                ..Default::default()
            },
            RTPCodecType::Video,
        )
        .map_err(|e| HuddleError::WebRtc(e.to_string()))?;
    }

    for (uri, kind) in [
        ("urn:ietf:params:rtp-hdrext:sdes:mid", RTPCodecType::Audio),
        ("urn:ietf:params:rtp-hdrext:sdes:mid", RTPCodecType::Video),
        (
            "urn:ietf:params:rtp-hdrext:ssrc-audio-level",
            RTPCodecType::Audio,
        ),
        (
            "http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time",
            RTPCodecType::Audio,
        ),
        (
            "http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time",
            RTPCodecType::Video,
        ),
        (
            "http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01",
            RTPCodecType::Audio,
        ),
        (
            "http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01",
            RTPCodecType::Video,
        ),
    ] {
        m.register_header_extension(
            RTCRtpHeaderExtensionCapability {
                uri: uri.to_owned(),
            },
            kind,
            None,
        )
        .map_err(|e| HuddleError::WebRtc(e.to_string()))?;
    }

    Ok(m)
}

/// One inbound RTP video packet read from a remote Chime video/content track.
#[derive(Debug, Clone)]
pub struct InboundVideoPacket {
    pub ssrc: u32,
    pub mid: String,
    pub track_id: String,
    pub mime_type: String,
    pub seq: u16,
    pub payload: Vec<u8>,
    pub marker: bool,
}

/// Active `webrtc-rs` connection wired to Chime's TURN SFU.
pub struct ChimeRtcPeer {
    pc: Arc<RTCPeerConnection>,
    audio_track: Arc<TrackLocalStaticSample>,
    video_track: Arc<TrackLocalStaticSample>,
    packets_sent: Arc<AtomicU64>,
    bytes_sent: Arc<AtomicU64>,
    packets_received: Arc<AtomicU64>,
    bytes_received: Arc<AtomicU64>,
    total_packets_sent: Arc<AtomicU64>,
    total_packets_received: Arc<AtomicU64>,
    total_bytes_sent: Arc<AtomicU64>,
    total_bytes_received: Arc<AtomicU64>,
}

impl ChimeRtcPeer {
    /// Create the PeerConnection with relay-only UDP TURN credentials from `JOIN_ACK`.
    pub async fn new(
        turn: &SdkTurnCredentials,
        inbound_opus_tx: mpsc::UnboundedSender<Vec<u8>>,
        inbound_video_tx: mpsc::UnboundedSender<InboundVideoPacket>,
        state_tx: mpsc::UnboundedSender<RTCPeerConnectionState>,
    ) -> Result<Self> {
        let udp_uris = filter_udp_turn_uris(&turn.uris);
        if udp_uris.is_empty() {
            return Err(HuddleError::WebRtc(
                "JOIN_ACK returned no UDP TURN URIs".to_owned(),
            ));
        }

        let mut media = chime_media_engine()?;
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media)
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?;
        let api = APIBuilder::new()
            .with_media_engine(media)
            .with_interceptor_registry(registry)
            .with_setting_engine(chime_setting_engine())
            .build();

        let config = RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: udp_uris,
                username: turn.username.clone().unwrap_or_default(),
                credential: turn.password.clone().unwrap_or_default(),
            }],
            ice_transport_policy: RTCIceTransportPolicy::Relay,
            ..Default::default()
        };

        let pc = Arc::new(
            api.new_peer_connection(config)
                .await
                .map_err(|e| HuddleError::WebRtc(e.to_string()))?,
        );

        let audio_track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: 48_000,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                rtcp_feedback: Vec::new(),
            },
            "AmazonChimeExpressAudio".to_owned(),
            "AmazonChimeExpressAudio".to_owned(),
        ));

        pc.add_track(Arc::clone(&audio_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?;

        // Local H.264 video track on mid 1 (official Slack camera slot).
        let video_track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_H264.to_owned(),
                clock_rate: 90_000,
                channels: 0,
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f"
                        .to_owned(),
                rtcp_feedback: Vec::new(),
            },
            "AmazonChimeExpressVideo".to_owned(),
            "AmazonChimeExpressVideo".to_owned(),
        ));
        pc.add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?;

        let packets_received = Arc::new(AtomicU64::new(0));
        let bytes_received = Arc::new(AtomicU64::new(0));
        let total_packets_received = Arc::new(AtomicU64::new(0));
        let total_bytes_received = Arc::new(AtomicU64::new(0));
        let rx_pkts = Arc::clone(&packets_received);
        let rx_bytes = Arc::clone(&bytes_received);
        let rx_total = Arc::clone(&total_packets_received);
        let rx_total_bytes = Arc::clone(&total_bytes_received);
        pc.on_track(Box::new(move |track, _, transceiver| {
            let audio_tx = inbound_opus_tx.clone();
            let video_tx = inbound_video_tx.clone();
            let rx_pkts = Arc::clone(&rx_pkts);
            let rx_bytes = Arc::clone(&rx_bytes);
            let rx_total = Arc::clone(&rx_total);
            let rx_total_bytes = Arc::clone(&rx_total_bytes);
            Box::pin(async move {
                match track.kind() {
                    RTPCodecType::Audio => {
                        while let Ok((rtp, _)) = track.read_rtp().await {
                            if rtp.payload.is_empty() {
                                continue;
                            }
                            rx_pkts.fetch_add(1, Ordering::Relaxed);
                            rx_total.fetch_add(1, Ordering::Relaxed);
                            let len = rtp.payload.len() as u64;
                            rx_bytes.fetch_add(len, Ordering::Relaxed);
                            rx_total_bytes.fetch_add(len, Ordering::Relaxed);
                            if audio_tx.send(rtp.payload.to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                    RTPCodecType::Video => {
                        let ssrc = track.ssrc();
                        let track_id = track.id();
                        let mime_type = track.codec().capability.mime_type;
                        let mid = transceiver.mid().unwrap_or_default();
                        while let Ok((rtp, _)) = track.read_rtp().await {
                            if rtp.payload.is_empty() {
                                continue;
                            }
                            rx_pkts.fetch_add(1, Ordering::Relaxed);
                            rx_total.fetch_add(1, Ordering::Relaxed);
                            let len = rtp.payload.len() as u64;
                            rx_bytes.fetch_add(len, Ordering::Relaxed);
                            rx_total_bytes.fetch_add(len, Ordering::Relaxed);
                            if video_tx
                                .send(InboundVideoPacket {
                                    ssrc,
                                    mid: mid.to_string(),
                                    track_id: track_id.clone(),
                                    mime_type: mime_type.clone(),
                                    seq: rtp.header.sequence_number,
                                    payload: rtp.payload.to_vec(),
                                    marker: rtp.header.marker,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            })
        }));

        pc.on_peer_connection_state_change(Box::new(move |state| {
            let _ = state_tx.send(state);
            Box::pin(async {})
        }));

        Ok(Self {
            pc,
            audio_track,
            video_track,
            packets_sent: Arc::new(AtomicU64::new(0)),
            bytes_sent: Arc::new(AtomicU64::new(0)),
            packets_received,
            bytes_received,
            total_packets_sent: Arc::new(AtomicU64::new(0)),
            total_packets_received,
            total_bytes_sent: Arc::new(AtomicU64::new(0)),
            total_bytes_received,
        })
    }

    /// Create a `#content` sender PeerConnection (identical media engine & SRTP profile, audio on
    /// mid 0 and H.264 content video on mid 1).
    pub async fn new_content_sender(
        turn: &SdkTurnCredentials,
        inbound_opus_tx: mpsc::UnboundedSender<Vec<u8>>,
        inbound_video_tx: mpsc::UnboundedSender<InboundVideoPacket>,
        state_tx: mpsc::UnboundedSender<RTCPeerConnectionState>,
    ) -> Result<Self> {
        Self::new(turn, inbound_opus_tx, inbound_video_tx, state_tx).await
    }

    /// Ensure at least `count` dedicated `Recvonly` video transceivers exist AFTER the mid-1
    /// camera slot (matching official Slack / Chime SFU expectations for subscribing to remote
    /// cameras and `#content` screen shares), then return the freshly gathered offer SDP and the
    /// assigned `mid` strings for those receive transceivers.
    pub async fn prepare_video_receive_offer(&self, count: usize) -> Result<(String, Vec<String>)> {
        let existing_video = self
            .pc
            .get_transceivers()
            .await
            .into_iter()
            .filter(|t| t.kind() == RTPCodecType::Video)
            .count();
        // Transceiver 0 of video is mid 1 (local camera); receive slots start at index 1 (mid 2+).
        let needed_total = 1 + count;
        for _ in existing_video..needed_total {
            self.pc
                .add_transceiver_from_kind(
                    RTPCodecType::Video,
                    Some(RTCRtpTransceiverInit {
                        direction: RTCRtpTransceiverDirection::Recvonly,
                        send_encodings: Vec::new(),
                    }),
                )
                .await
                .map_err(|e| HuddleError::WebRtc(e.to_string()))?;
        }
        let offer = self.create_gathered_offer().await?;
        let mids: Vec<String> = self
            .pc
            .get_transceivers()
            .await
            .into_iter()
            .filter(|t| t.kind() == RTPCodecType::Video)
            .skip(1)
            .take(count)
            .enumerate()
            .map(|(ix, t)| {
                t.mid()
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| (ix + 2).to_string())
            })
            .collect();
        Ok((offer, mids))
    }

    /// Send RTCP PLI + FIR to request an immediate keyframe for `media_ssrc` (prevents frozen
    /// screen shares or cameras after packet loss).
    pub async fn request_keyframe(&self, media_ssrc: u32) {
        static FIR_SEQ: AtomicU8 = AtomicU8::new(0);
        let pli = PictureLossIndication {
            sender_ssrc: 0,
            media_ssrc,
        };
        let fir = FullIntraRequest {
            sender_ssrc: 0,
            media_ssrc,
            fir: vec![FirEntry {
                ssrc: media_ssrc,
                sequence_number: FIR_SEQ.fetch_add(1, Ordering::Relaxed),
            }],
        };
        let pkts: Vec<Box<dyn webrtc::rtcp::packet::Packet + Send + Sync>> =
            vec![Box::new(pli), Box::new(fir)];
        let _ = self.pc.write_rtcp(&pkts).await;
    }

    /// Create and fully gather a non-trickle SDP offer containing the relay candidate.
    pub async fn create_gathered_offer(&self) -> Result<String> {
        let offer = self
            .pc
            .create_offer(None)
            .await
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?;
        let mut gather_complete = self.pc.gathering_complete_promise().await;
        self.pc
            .set_local_description(offer)
            .await
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?;

        // Wait up to 6s for TURN relay candidate gathering.
        let _ = tokio::time::timeout(Duration::from_secs(6), gather_complete.recv()).await;

        let local =
            self.pc.local_description().await.ok_or_else(|| {
                HuddleError::WebRtc("missing local SDP after gathering".to_owned())
            })?;
        if !local.sdp.contains("typ relay") {
            return Err(HuddleError::UdpBlocked);
        }
        Ok(local.sdp)
    }

    /// Apply Chime's `SUBSCRIBE_ACK` SDP answer.
    pub async fn set_remote_answer(&self, sdp_answer: String) -> Result<()> {
        let answer = RTCSessionDescription::answer(sdp_answer)
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?;
        self.pc
            .set_remote_description(answer)
            .await
            .map_err(|e| HuddleError::WebRtc(e.to_string()))
    }

    /// Write one encoded 20ms Opus audio packet onto the local track.
    pub async fn write_opus_frame(&self, opus_bytes: Vec<u8>) -> Result<()> {
        let len = opus_bytes.len() as u64;
        self.audio_track
            .write_sample(&Sample {
                data: Bytes::from(opus_bytes),
                duration: Duration::from_millis(20),
                ..Default::default()
            })
            .await
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?;
        self.packets_sent.fetch_add(1, Ordering::Relaxed);
        self.total_packets_sent.fetch_add(1, Ordering::Relaxed);
        self.bytes_sent.fetch_add(len, Ordering::Relaxed);
        self.total_bytes_sent.fetch_add(len, Ordering::Relaxed);
        Ok(())
    }

    /// Write one Annex-B H.264 video frame onto the local video track.
    pub async fn write_h264_frame(&self, annex_b: Vec<u8>, duration: Duration) -> Result<()> {
        let len = annex_b.len() as u64;
        self.video_track
            .write_sample(&Sample {
                data: Bytes::from(annex_b),
                duration,
                ..Default::default()
            })
            .await
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?;
        self.packets_sent.fetch_add(1, Ordering::Relaxed);
        self.total_packets_sent.fetch_add(1, Ordering::Relaxed);
        self.bytes_sent.fetch_add(len, Ordering::Relaxed);
        self.total_bytes_sent.fetch_add(len, Ordering::Relaxed);
        Ok(())
    }

    pub fn connection_state(&self) -> RTCPeerConnectionState {
        self.pc.connection_state()
    }

    pub fn total_packets_sent(&self) -> u64 {
        self.total_packets_sent.load(Ordering::Relaxed)
    }

    pub fn total_packets_received(&self) -> u64 {
        self.total_packets_received.load(Ordering::Relaxed)
    }

    pub fn total_bytes_sent(&self) -> u64 {
        self.total_bytes_sent.load(Ordering::Relaxed)
    }

    pub fn total_bytes_received(&self) -> u64 {
        self.total_bytes_received.load(Ordering::Relaxed)
    }

    /// Sample and reset 1-second delta counters for Chime's `CLIENT_METRIC` frame.
    pub fn take_metrics_snapshot(&self, local_rms: f32) -> ClientMetricsSnapshot {
        let tx_pkts = self.packets_sent.swap(0, Ordering::Relaxed) as f64;
        let tx_bytes = self.bytes_sent.swap(0, Ordering::Relaxed) as f64;
        let rx_pkts = self.packets_received.swap(0, Ordering::Relaxed) as f64;
        let rx_bytes = self.bytes_received.swap(0, Ordering::Relaxed) as f64;
        ClientMetricsSnapshot {
            stun_rtt_ms: 25.0,
            mic_pps: tx_pkts,
            mic_bitrate_bps: tx_bytes * 8.0,
            mic_audio_level: (local_rms as f64 * 32767.0).clamp(0.0, 32767.0),
            spk_pps: rx_pkts,
            spk_bitrate_bps: rx_bytes * 8.0,
        }
    }

    pub async fn close(&self) {
        let _ = self.pc.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_turn_uris_to_udp_only() {
        let uris = vec![
            "turn:ice.z2.ec1.m.chime.aws:3478?transport=udp".to_owned(),
            "turns:ice.z2.ec1.m.chime.aws:443?transport=tcp".to_owned(),
            "turn:ice.z2.ec1.m.chime.aws:3478?transport=tcp".to_owned(),
        ];
        assert_eq!(
            filter_udp_turn_uris(&uris),
            vec!["turn:ice.z2.ec1.m.chime.aws:3478?transport=udp".to_owned()]
        );
    }

    #[test]
    fn media_and_setting_engines_configure_cleanly() {
        let _settings = chime_setting_engine();
        let _media = chime_media_engine().expect("chime media engine builds");
    }
}
