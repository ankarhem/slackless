//! `webrtc-rs` PeerConnection configured specifically for Amazon Chime SDK's SFU constraints:
//! - Offer `SRTP_AES128_CM_HMAC_SHA1_80` first and exclude `AEAD_AES_256_GCM`
//! - Enforce `RTCIceTransportPolicy::Relay`
//! - Use UDP-only TURN URIs from `JOIN_ACK` (`turn:...?transport=udp`)
//! - Opus 48kHz without RED (RFC 2198) + BUNDLEd `recvonly` video placeholder
//! - Full non-trickle SDP offer gathering

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::mpsc;
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MIME_TYPE_OPUS, MIME_TYPE_VP8, MediaEngine};
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
use webrtc::rtp_transceiver::RTCRtpTransceiverInit;
use webrtc::rtp_transceiver::rtp_codec::{
    RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType,
};
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
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

/// Build the Chime-compatible `MediaEngine` (Opus 48kHz without RED, plus VP8 recv placeholder).
pub fn chime_media_engine() -> Result<MediaEngine> {
    let mut m = MediaEngine::default();
    m.register_codec(
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: 48_000,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                rtcp_feedback: Vec::new(),
            },
            payload_type: 111,
            ..Default::default()
        },
        RTPCodecType::Audio,
    )
    .map_err(|e| HuddleError::WebRtc(e.to_string()))?;

    m.register_codec(
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: MIME_TYPE_VP8.to_owned(),
                clock_rate: 90_000,
                channels: 0,
                sdp_fmtp_line: String::new(),
                rtcp_feedback: Vec::new(),
            },
            payload_type: 96,
            ..Default::default()
        },
        RTPCodecType::Video,
    )
    .map_err(|e| HuddleError::WebRtc(e.to_string()))?;
    Ok(m)
}

/// Active `webrtc-rs` connection wired to Chime's TURN SFU.
pub struct ChimeRtcPeer {
    pc: Arc<RTCPeerConnection>,
    audio_track: Arc<TrackLocalStaticSample>,
    packets_sent: Arc<AtomicU64>,
    bytes_sent: Arc<AtomicU64>,
    packets_received: Arc<AtomicU64>,
    bytes_received: Arc<AtomicU64>,
}

impl ChimeRtcPeer {
    /// Create the PeerConnection with relay-only UDP TURN credentials from `JOIN_ACK`.
    pub async fn new(
        turn: &SdkTurnCredentials,
        inbound_opus_tx: mpsc::UnboundedSender<Vec<u8>>,
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

        // Chime's official client sends a BUNDLEd video m-line even for audio-only sessions.
        pc.add_transceiver_from_kind(
            RTPCodecType::Video,
            Some(RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                send_encodings: Vec::new(),
            }),
        )
        .await
        .map_err(|e| HuddleError::WebRtc(e.to_string()))?;

        let packets_received = Arc::new(AtomicU64::new(0));
        let bytes_received = Arc::new(AtomicU64::new(0));
        let rx_pkts = Arc::clone(&packets_received);
        let rx_bytes = Arc::clone(&bytes_received);
        pc.on_track(Box::new(move |track, _, _| {
            let tx = inbound_opus_tx.clone();
            let rx_pkts = Arc::clone(&rx_pkts);
            let rx_bytes = Arc::clone(&rx_bytes);
            Box::pin(async move {
                if track.kind() != RTPCodecType::Audio {
                    return;
                }
                while let Ok((rtp, _)) = track.read_rtp().await {
                    if rtp.payload.is_empty() {
                        continue;
                    }
                    rx_pkts.fetch_add(1, Ordering::Relaxed);
                    rx_bytes.fetch_add(rtp.payload.len() as u64, Ordering::Relaxed);
                    if tx.send(rtp.payload.to_vec()).is_err() {
                        break;
                    }
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
            packets_sent: Arc::new(AtomicU64::new(0)),
            bytes_sent: Arc::new(AtomicU64::new(0)),
            packets_received,
            bytes_received,
        })
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
        self.bytes_sent.fetch_add(len, Ordering::Relaxed);
        Ok(())
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
