//! `webrtc-rs` PeerConnection configured specifically for Amazon Chime SDK's SFU constraints:
//! - Offer `SRTP_AES128_CM_HMAC_SHA1_80` first and exclude `AEAD_AES_256_GCM`
//! - Enforce `RTCIceTransportPolicy::Relay`
//! - Use UDP-only TURN URIs from `JOIN_ACK` (`turn:...?transport=udp`)
//! - Opus 48kHz without RED (RFC 2198) + BUNDLEd `recvonly` video placeholder
//! - Full non-trickle SDP offer gathering

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use bytes::Bytes;
use rtc::interceptor::{
    Attribute, AttributedPacket, Interceptor, Packet as InterceptorPacket, Slot, StreamInfo,
    TaggedPacket,
};
use rtc::media::Sample;
use rtc::media_stream::MediaStreamTrack;
use rtc::peer_connection::configuration::media_engine::{
    MIME_TYPE_AV1, MIME_TYPE_H264, MIME_TYPE_OPUS, MediaEngine,
};
use rtc::rtcp::payload_feedbacks::full_intra_request::{FirEntry, FullIntraRequest};
use rtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use rtc::rtcp::payload_feedbacks::receiver_estimated_maximum_bitrate::ReceiverEstimatedMaximumBitrate;
use rtc::rtp_transceiver::rtp_sender::{
    RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters,
    RTCRtpEncodingParameters, RTCRtpHeaderExtensionCapability, RtpCodecKind,
};
use rtc::rtp_transceiver::{PayloadType, RTCRtpTransceiverId};
use rtc::sansio::Protocol;
use rtc::shared::error::Error as RtcError;
use tokio::sync::{Mutex as AsyncMutex, mpsc, watch};
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_local::{TrackLocal, TrackLocalEvent};
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
    RTCIceGatheringState, RTCIceServer, RTCIceTransportPolicy, RTCPeerConnectionState,
    RTCSessionDescription, Registry, SettingEngine, SettingEngineBuilder, SrtpProtectionProfile,
    register_default_interceptors,
};
use webrtc::rtp_transceiver::{
    RTCRtpTransceiverDirection, RTCRtpTransceiverInit, RtpSender, RtpTransceiver,
};

use crate::proto::SdkTurnCredentials;
use crate::signaling::ClientMetricsSnapshot;
use crate::{HuddleError, Result};

const OPUS_PAYLOAD_TYPE: PayloadType = 111;
const H264_PAYLOAD_TYPE: PayloadType = 108;
const H264_FMTP_LINE: &str =
    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f";
const OPUS_FMTP_LINE: &str = "minptime=10;useinbandfec=1";

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

/// Select UDP bind addresses for TURN allocation: bind loopback for local test TURN servers and
/// wildcard (`0.0.0.0:0`, which `webrtc 0.21` expands to usable IPv4 interfaces) for remote servers.
fn udp_bind_addrs_for_turn(udp_uris: &[String]) -> Vec<String> {
    let uses_loopback = udp_uris.iter().any(|u| {
        let lower = u.to_ascii_lowercase();
        lower.contains("://127.0.0.1")
            || lower.starts_with("turn:127.0.0.1")
            || lower.starts_with("turn:localhost")
            || lower.starts_with("turn:[::1]")
    });
    if uses_loopback {
        vec!["127.0.0.1:0".to_owned()]
    } else {
        vec!["0.0.0.0:0".to_owned()]
    }
}

/// Build `SettingEngine` with Chime's mandatory SRTP protection profile order.
pub fn chime_setting_engine() -> SettingEngine {
    // Chime silently drops packets when AEAD_AES_128_GCM is chosen over AES128_CM_SHA1_80,
    // and webrtc-srtp panics on AEAD_AES_256_GCM.
    SettingEngineBuilder::new()
        .with_srtp_protection_profiles(vec![SrtpProtectionProfile::Srtp_Aes128_Cm_Hmac_Sha1_80])
        .build()
}

/// Build the Chime-compatible `MediaEngine` matching official Slack's exact codec order, payload
/// types, RFC 4588 RTX streams, RTCP feedback, and `sdes:mid` RTP header extensions on BOTH audio
/// and video (required for BUNDLEd SFU tracks whose SSRCs arrive dynamically).
pub fn chime_media_engine() -> Result<MediaEngine> {
    chime_media_engine_for_caps(crate::backend::platform_backend().capabilities())
}

/// Build the Chime-compatible `MediaEngine` for explicit [`crate::backend::MediaCapabilities`]
/// (deterministically selecting AV1-first vs H.264-first video codec ordering).
pub fn chime_media_engine_for_caps(caps: crate::backend::MediaCapabilities) -> Result<MediaEngine> {
    let mut m = MediaEngine::default();
    m.register_codec(
        RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: 48_000,
                channels: 2,
                sdp_fmtp_line: OPUS_FMTP_LINE.to_owned(),
                rtcp_feedback: vec![RTCPFeedback {
                    typ: "transport-cc".to_owned(),
                    parameter: String::new(),
                }],
            },
            payload_type: OPUS_PAYLOAD_TYPE,
        },
        RtpCodecKind::Audio,
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

    // Register only video codecs we decode natively (`AV1` + `H.264`).
    // Codec order adapts dynamically to runtime hardware capabilities (never assumes M3+):
    // - M3/M4 Macs (`hw_av1_decode == true`): AV1 first (both AV1 & H.264 decode in HW).
    // - M1/M2 Macs (`hw_h264_decode && !hw_av1_decode`): H.264 first so peers prefer HW-decoded
    //   H.264 while still allowing seamless `rav1d` fallback when Chime forces AV1.
    let video_codecs = if caps.prefer_h264_over_av1() {
        [
            (MIME_TYPE_H264, H264_PAYLOAD_TYPE, H264_FMTP_LINE),
            (MIME_TYPE_AV1, 45u8, ""),
        ]
    } else {
        [
            (MIME_TYPE_AV1, 45u8, ""),
            (MIME_TYPE_H264, H264_PAYLOAD_TYPE, H264_FMTP_LINE),
        ]
    };
    for (mime, pt, fmtp) in video_codecs {
        m.register_codec(
            RTCRtpCodecParameters {
                rtp_codec: RTCRtpCodec {
                    mime_type: mime.to_owned(),
                    clock_rate: 90_000,
                    channels: 0,
                    sdp_fmtp_line: fmtp.to_owned(),
                    rtcp_feedback: video_fb(),
                },
                payload_type: pt,
            },
            RtpCodecKind::Video,
        )
        .map_err(|e| HuddleError::WebRtc(e.to_string()))?;
    }

    for (primary, rtx_pt) in [(45u8, 46u8), (H264_PAYLOAD_TYPE, 109u8)] {
        m.register_codec(
            RTCRtpCodecParameters {
                rtp_codec: RTCRtpCodec {
                    mime_type: "video/rtx".to_owned(),
                    clock_rate: 90_000,
                    channels: 0,
                    sdp_fmtp_line: format!("apt={primary}"),
                    rtcp_feedback: Vec::new(),
                },
                payload_type: rtx_pt,
            },
            RtpCodecKind::Video,
        )
        .map_err(|e| HuddleError::WebRtc(e.to_string()))?;
    }

    for (uri, kind) in [
        ("urn:ietf:params:rtp-hdrext:sdes:mid", RtpCodecKind::Audio),
        ("urn:ietf:params:rtp-hdrext:sdes:mid", RtpCodecKind::Video),
        (
            "urn:ietf:params:rtp-hdrext:ssrc-audio-level",
            RtpCodecKind::Audio,
        ),
        (
            "http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time",
            RtpCodecKind::Audio,
        ),
        (
            "http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time",
            RtpCodecKind::Video,
        ),
        (
            "http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01",
            RtpCodecKind::Audio,
        ),
        (
            "http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01",
            RtpCodecKind::Video,
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

/// Inbound RTCP feedback received on an outbound local video `RtpSender`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoSenderFeedback {
    /// Remote SFU/viewer requested an immediate IDR keyframe (RTCP PLI or FIR).
    ForceKeyframe,
    /// Remote SFU/viewer estimated available uplink bandwidth in bits per second (RTCP REMB).
    TargetBitrateBps(u32),
}

/// Classify an inbound RTCP packet on an outbound video sender into [`VideoSenderFeedback`].
pub(crate) fn classify_rtcp_video_feedback(
    pkt: &dyn rtc::rtcp::Packet,
) -> Option<VideoSenderFeedback> {
    let any = pkt.as_any();
    if any.downcast_ref::<PictureLossIndication>().is_some()
        || any.downcast_ref::<FullIntraRequest>().is_some()
    {
        Some(VideoSenderFeedback::ForceKeyframe)
    } else if let Some(remb) = any.downcast_ref::<ReceiverEstimatedMaximumBitrate>() {
        if remb.bitrate.is_nan() {
            return None;
        }
        let bps = remb.bitrate.clamp(100_000.0, 10_000_000.0) as u32;
        Some(VideoSenderFeedback::TargetBitrateBps(bps))
    } else {
        None
    }
}

/// Application-most interceptor that marks inbound video feedback RTCP (`PLI`, `FIR`, `REMB`) with
/// [`Attribute::DeliverToApplication`] so `webrtc 0.21` surfaces them via `TrackLocal::poll`.
///
/// In `webrtc 0.21`, inbound RTCP stops at the end of the interceptor chain unless an interceptor
/// explicitly marks packets for application delivery.
#[derive(Default)]
struct VideoFeedbackRtcpInterceptor {
    read_queue: VecDeque<TaggedPacket>,
    write_queue: VecDeque<TaggedPacket>,
}

impl Protocol<TaggedPacket, TaggedPacket, ()> for VideoFeedbackRtcpInterceptor {
    type Rout = TaggedPacket;
    type Wout = TaggedPacket;
    type Eout = ();
    type Error = RtcError;
    type Time = Instant;

    fn handle_read(&mut self, msg: TaggedPacket) -> std::result::Result<(), Self::Error> {
        if let InterceptorPacket::Rtcp(packets) = &msg.message.packet {
            for pkt in packets {
                if classify_rtcp_video_feedback(pkt.as_ref()).is_some() {
                    let mut attributes = msg.message.attributes.clone();
                    attributes.push(Attribute::DeliverToApplication);
                    self.read_queue.push_back(TaggedPacket {
                        now: msg.now,
                        transport: msg.transport,
                        message: AttributedPacket {
                            attributes,
                            packet: InterceptorPacket::Rtcp(vec![pkt.clone()]),
                        },
                    });
                }
            }
            return Ok(());
        }
        self.read_queue.push_back(msg);
        Ok(())
    }

    fn poll_read(&mut self) -> Option<Self::Rout> {
        self.read_queue.pop_front()
    }

    fn handle_write(&mut self, msg: TaggedPacket) -> std::result::Result<(), Self::Error> {
        self.write_queue.push_back(msg);
        Ok(())
    }

    fn poll_write(&mut self) -> Option<Self::Wout> {
        self.write_queue.pop_front()
    }
}

impl Interceptor for VideoFeedbackRtcpInterceptor {
    fn bind_local_stream(&mut self, _info: &StreamInfo) {}
    fn unbind_local_stream(&mut self, _info: &StreamInfo) {}
    fn bind_remote_stream(&mut self, _info: &StreamInfo) {}
    fn unbind_remote_stream(&mut self, _info: &StreamInfo) {}
}

/// One inbound RTP Opus audio packet read from Chime's mixed remote audio track.
#[derive(Debug, Clone)]
pub struct InboundAudioPacket {
    pub seq: u16,
    pub payload: Bytes,
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

struct ChimePeerHandler {
    pc: Arc<OnceLock<Weak<dyn PeerConnection>>>,
    inbound_opus_tx: mpsc::UnboundedSender<InboundAudioPacket>,
    inbound_video_tx: mpsc::UnboundedSender<InboundVideoPacket>,
    state_tx: mpsc::UnboundedSender<RTCPeerConnectionState>,
    connection_state: Arc<StdMutex<RTCPeerConnectionState>>,
    gather_complete_tx: watch::Sender<u64>,
    remote_tracks: Arc<StdMutex<HashMap<u32, Arc<dyn TrackRemote>>>>,
    packets_received: Arc<AtomicU64>,
    bytes_received: Arc<AtomicU64>,
    total_packets_received: Arc<AtomicU64>,
    total_bytes_received: Arc<AtomicU64>,
}

async fn resolve_track_mid(
    pc_cell: &OnceLock<Weak<dyn PeerConnection>>,
    receiver_id: Option<rtc::rtp_transceiver::RTCRtpReceiverId>,
    track_id: &str,
) -> String {
    let Some(pc) = pc_cell.get().and_then(Weak::upgrade) else {
        return String::new();
    };
    let transceivers = pc.get_transceivers().await;
    if let Some(rx_id) = receiver_id {
        let tid: RTCRtpTransceiverId = rx_id.into();
        if let Some(t) = transceivers.iter().find(|t| t.id() == tid) {
            return t.mid().await.ok().flatten().unwrap_or_default();
        }
    }
    for t in transceivers {
        if let Ok(Some(receiver)) = t.receiver().await
            && receiver.track().track_id().await == track_id
        {
            return t.mid().await.ok().flatten().unwrap_or_default();
        }
    }
    String::new()
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for ChimePeerHandler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            self.gather_complete_tx
                .send_modify(|seq| *seq = seq.wrapping_add(1));
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        if let Ok(mut guard) = self.connection_state.lock() {
            *guard = state;
        }
        let _ = self.state_tx.send(state);
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let audio_tx = self.inbound_opus_tx.clone();
        let video_tx = self.inbound_video_tx.clone();
        let pc_cell = Arc::clone(&self.pc);
        let remote_tracks = Arc::clone(&self.remote_tracks);
        let rx_pkts = Arc::clone(&self.packets_received);
        let rx_bytes = Arc::clone(&self.bytes_received);
        let rx_total = Arc::clone(&self.total_packets_received);
        let rx_total_bytes = Arc::clone(&self.total_bytes_received);

        tokio::spawn(async move {
            match track.kind().await {
                RtpCodecKind::Audio => {
                    while let Some(evt) = track.poll().await {
                        match evt {
                            TrackRemoteEvent::OnRtpPacket(rtp) => {
                                if rtp.payload.is_empty() {
                                    continue;
                                }
                                rx_pkts.fetch_add(1, Ordering::Relaxed);
                                rx_total.fetch_add(1, Ordering::Relaxed);
                                let len = rtp.payload.len() as u64;
                                rx_bytes.fetch_add(len, Ordering::Relaxed);
                                rx_total_bytes.fetch_add(len, Ordering::Relaxed);
                                if audio_tx
                                    .send(InboundAudioPacket {
                                        seq: rtp.header.sequence_number,
                                        payload: rtp.payload,
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            TrackRemoteEvent::OnEnded | TrackRemoteEvent::OnError => break,
                            _ => {}
                        }
                    }
                }
                RtpCodecKind::Video => {
                    let mut ssrc = track.ssrcs().await.first().copied().unwrap_or(0);
                    let track_id = track.track_id().await;
                    let mut mime_type = if ssrc != 0 {
                        track
                            .codec(ssrc)
                            .await
                            .map(|c| c.mime_type)
                            .unwrap_or_default()
                    } else {
                        String::new()
                    };
                    let mut mid = resolve_track_mid(&pc_cell, None, &track_id).await;
                    if ssrc != 0
                        && let Ok(mut map) = remote_tracks.lock()
                    {
                        map.insert(ssrc, Arc::clone(&track));
                    }
                    let mut logged_attach = false;
                    let mut logged_first_pkt = false;

                    while let Some(evt) = track.poll().await {
                        match evt {
                            TrackRemoteEvent::OnOpen(init) => {
                                if init.ssrc != 0 {
                                    ssrc = init.ssrc;
                                    if let Ok(mut map) = remote_tracks.lock() {
                                        map.insert(ssrc, Arc::clone(&track));
                                    }
                                }
                                if mime_type.is_empty() && ssrc != 0 {
                                    mime_type = track
                                        .codec(ssrc)
                                        .await
                                        .map(|c| c.mime_type)
                                        .unwrap_or_default();
                                }
                                if mid.is_empty() {
                                    mid = resolve_track_mid(
                                        &pc_cell,
                                        Some(init.receiver_id),
                                        &track_id,
                                    )
                                    .await;
                                }
                                if !logged_attach {
                                    logged_attach = true;
                                    tracing::info!(
                                        ssrc,
                                        mid = %mid,
                                        track_id = %track_id,
                                        mime = %mime_type,
                                        "remote video track attached"
                                    );
                                }
                            }
                            TrackRemoteEvent::OnRtpPacket(rtp) => {
                                if rtp.payload.is_empty() {
                                    continue;
                                }
                                if ssrc == 0 {
                                    ssrc = rtp.header.ssrc;
                                    if let Ok(mut map) = remote_tracks.lock() {
                                        map.insert(ssrc, Arc::clone(&track));
                                    }
                                }
                                if mime_type.is_empty() {
                                    mime_type = track
                                        .codec(ssrc)
                                        .await
                                        .map(|c| c.mime_type)
                                        .unwrap_or_default();
                                }
                                if !logged_attach {
                                    logged_attach = true;
                                    tracing::info!(
                                        ssrc,
                                        mid = %mid,
                                        track_id = %track_id,
                                        mime = %mime_type,
                                        pt = rtp.header.payload_type,
                                        "remote video track attached"
                                    );
                                }
                                if !logged_first_pkt {
                                    logged_first_pkt = true;
                                    tracing::info!(
                                        ssrc,
                                        mid = %mid,
                                        mime = %mime_type,
                                        seq = rtp.header.sequence_number,
                                        marker = rtp.header.marker,
                                        bytes = rtp.payload.len(),
                                        "received first RTP video packet"
                                    );
                                }
                                rx_pkts.fetch_add(1, Ordering::Relaxed);
                                rx_total.fetch_add(1, Ordering::Relaxed);
                                let len = rtp.payload.len() as u64;
                                rx_bytes.fetch_add(len, Ordering::Relaxed);
                                rx_total_bytes.fetch_add(len, Ordering::Relaxed);
                                if video_tx
                                    .send(InboundVideoPacket {
                                        ssrc,
                                        mid: mid.clone(),
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
                            TrackRemoteEvent::OnEnded | TrackRemoteEvent::OnError => break,
                            _ => {}
                        }
                    }
                    if ssrc != 0
                        && let Ok(mut map) = remote_tracks.lock()
                    {
                        map.remove(&ssrc);
                    }
                    tracing::info!(ssrc, mid = %mid, "remote video track ended");
                }
                _ => {}
            }
        });
    }
}

/// Active `webrtc-rs` connection wired to Chime's TURN SFU.
pub struct ChimeRtcPeer {
    pc: Arc<dyn PeerConnection>,
    audio_track: Arc<TrackLocalStaticSample>,
    audio_sender: Arc<dyn RtpSender>,
    audio_ssrc: u32,
    audio_payload_type: Arc<AtomicU8>,
    video_track: Arc<TrackLocalStaticSample>,
    video_sender: Arc<dyn RtpSender>,
    video_ssrc: u32,
    video_payload_type: Arc<AtomicU8>,
    receive_video_transceivers: AsyncMutex<Vec<Arc<dyn RtpTransceiver>>>,
    connection_state: Arc<StdMutex<RTCPeerConnectionState>>,
    gather_complete_tx: watch::Sender<u64>,
    remote_tracks: Arc<StdMutex<HashMap<u32, Arc<dyn TrackRemote>>>>,
    packets_sent: Arc<AtomicU64>,
    bytes_sent: Arc<AtomicU64>,
    packets_received: Arc<AtomicU64>,
    bytes_received: Arc<AtomicU64>,
    total_packets_sent: Arc<AtomicU64>,
    total_packets_received: Arc<AtomicU64>,
    total_bytes_sent: Arc<AtomicU64>,
    total_bytes_received: Arc<AtomicU64>,
}

fn non_zero_random_ssrc() -> u32 {
    loop {
        let ssrc = rand::random::<u32>();
        if ssrc != 0 {
            return ssrc;
        }
    }
}

impl ChimeRtcPeer {
    /// Create the PeerConnection with relay-only UDP TURN credentials from `JOIN_ACK`.
    pub async fn new(
        turn: &SdkTurnCredentials,
        inbound_opus_tx: mpsc::UnboundedSender<InboundAudioPacket>,
        inbound_video_tx: mpsc::UnboundedSender<InboundVideoPacket>,
        state_tx: mpsc::UnboundedSender<RTCPeerConnectionState>,
    ) -> Result<Self> {
        let udp_uris = filter_udp_turn_uris(&turn.uris);
        if udp_uris.is_empty() {
            return Err(HuddleError::WebRtc(
                "JOIN_ACK returned no UDP TURN URIs".to_owned(),
            ));
        }

        let udp_bind_addrs = udp_bind_addrs_for_turn(&udp_uris);
        let mut media = chime_media_engine()?;
        let registry = register_default_interceptors(Registry::new(), &mut media)
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?
            .with(Slot::from(14_000), VideoFeedbackRtcpInterceptor::default());

        let config = RTCConfigurationBuilder::new()
            .with_ice_servers(vec![RTCIceServer {
                urls: udp_uris,
                username: turn.username.clone().unwrap_or_default(),
                credential: turn.password.clone().unwrap_or_default(),
            }])
            .with_ice_transport_policy(RTCIceTransportPolicy::Relay)
            .build();

        let pc_cell: Arc<OnceLock<Weak<dyn PeerConnection>>> = Arc::new(OnceLock::new());
        let connection_state = Arc::new(StdMutex::new(RTCPeerConnectionState::New));
        let (gather_complete_tx, _) = watch::channel(0u64);
        let remote_tracks: Arc<StdMutex<HashMap<u32, Arc<dyn TrackRemote>>>> =
            Arc::new(StdMutex::new(HashMap::new()));
        let packets_received = Arc::new(AtomicU64::new(0));
        let bytes_received = Arc::new(AtomicU64::new(0));
        let total_packets_received = Arc::new(AtomicU64::new(0));
        let total_bytes_received = Arc::new(AtomicU64::new(0));

        let handler = Arc::new(ChimePeerHandler {
            pc: Arc::clone(&pc_cell),
            inbound_opus_tx,
            inbound_video_tx,
            state_tx,
            connection_state: Arc::clone(&connection_state),
            gather_complete_tx: gather_complete_tx.clone(),
            remote_tracks: Arc::clone(&remote_tracks),
            packets_received: Arc::clone(&packets_received),
            bytes_received: Arc::clone(&bytes_received),
            total_packets_received: Arc::clone(&total_packets_received),
            total_bytes_received: Arc::clone(&total_bytes_received),
        });

        let pc: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::new()
                .with_configuration(config)
                .with_media_engine(media)
                .with_interceptor_registry(registry)
                .with_setting_engine(chime_setting_engine())
                .with_handler(handler)
                .with_udp_addrs(udp_bind_addrs)
                .build()
                .await
                .map_err(|e| HuddleError::WebRtc(e.to_string()))?,
        );
        let _ = pc_cell.set(Arc::downgrade(&pc));

        let audio_ssrc = non_zero_random_ssrc();
        let audio_track = Arc::new(
            TrackLocalStaticSample::new(
                Instant::now(),
                MediaStreamTrack::new(
                    "AmazonChimeExpressAudio".to_owned(),
                    "AmazonChimeExpressAudio".to_owned(),
                    "AmazonChimeExpressAudio".to_owned(),
                    RtpCodecKind::Audio,
                    vec![RTCRtpEncodingParameters {
                        rtp_coding_parameters: RTCRtpCodingParameters {
                            ssrc: Some(audio_ssrc),
                            ..Default::default()
                        },
                        codec: RTCRtpCodec {
                            mime_type: MIME_TYPE_OPUS.to_owned(),
                            clock_rate: 48_000,
                            channels: 2,
                            sdp_fmtp_line: OPUS_FMTP_LINE.to_owned(),
                            rtcp_feedback: Vec::new(),
                        },
                        ..Default::default()
                    }],
                ),
            )
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?,
        );
        let audio_sender = pc
            .add_track(Arc::clone(&audio_track) as Arc<dyn TrackLocal>)
            .await
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?;

        // Local H.264 video track on mid 1 (official Slack camera slot).
        let video_ssrc = non_zero_random_ssrc();
        let video_track = Arc::new(
            TrackLocalStaticSample::new(
                Instant::now(),
                MediaStreamTrack::new(
                    "AmazonChimeExpressVideo".to_owned(),
                    "AmazonChimeExpressVideo".to_owned(),
                    "AmazonChimeExpressVideo".to_owned(),
                    RtpCodecKind::Video,
                    vec![RTCRtpEncodingParameters {
                        rtp_coding_parameters: RTCRtpCodingParameters {
                            ssrc: Some(video_ssrc),
                            ..Default::default()
                        },
                        codec: RTCRtpCodec {
                            mime_type: MIME_TYPE_H264.to_owned(),
                            clock_rate: 90_000,
                            channels: 0,
                            sdp_fmtp_line: H264_FMTP_LINE.to_owned(),
                            rtcp_feedback: Vec::new(),
                        },
                        ..Default::default()
                    }],
                ),
            )
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?,
        );
        let video_sender = pc
            .add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal>)
            .await
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?;

        Ok(Self {
            pc,
            audio_track,
            audio_sender,
            audio_ssrc,
            audio_payload_type: Arc::new(AtomicU8::new(OPUS_PAYLOAD_TYPE)),
            video_track,
            video_sender,
            video_ssrc,
            video_payload_type: Arc::new(AtomicU8::new(H264_PAYLOAD_TYPE)),
            receive_video_transceivers: AsyncMutex::new(Vec::new()),
            connection_state,
            gather_complete_tx,
            remote_tracks,
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
        inbound_opus_tx: mpsc::UnboundedSender<InboundAudioPacket>,
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
        let mut rx_transceivers = self.receive_video_transceivers.lock().await;
        while rx_transceivers.len() < count {
            let transceiver = self
                .pc
                .add_transceiver_from_kind(
                    RtpCodecKind::Video,
                    Some(RTCRtpTransceiverInit {
                        direction: RTCRtpTransceiverDirection::Recvonly,
                        streams: Vec::new(),
                        send_encodings: Vec::new(),
                    }),
                )
                .await
                .map_err(|e| HuddleError::WebRtc(e.to_string()))?;
            rx_transceivers.push(transceiver);
        }

        let offer = self.create_gathered_offer().await?;
        let mut mids = Vec::with_capacity(count);
        for (ix, transceiver) in rx_transceivers.iter().take(count).enumerate() {
            let mid = transceiver
                .mid()
                .await
                .ok()
                .flatten()
                .unwrap_or_else(|| (ix + 2).to_string());
            mids.push(mid);
        }
        Ok((offer, mids))
    }

    /// Spawn a lightweight RTCP feedback reader on the local outbound video `TrackLocal` so
    /// incoming PLI/FIR trigger instant IDR keyframes (enabling longer 10s GOPs without pulsing)
    /// and incoming REMB smoothly adapts encoder bitrate.
    pub async fn attach_video_feedback(&self, tx: mpsc::UnboundedSender<VideoSenderFeedback>) {
        let track = Arc::clone(&self.video_track);
        tokio::spawn(async move {
            while let Some(evt) = track.poll().await {
                if let TrackLocalEvent::OnRtcpPacket(packets) = evt {
                    for pkt in packets {
                        if let Some(fb) = classify_rtcp_video_feedback(pkt.as_ref())
                            && tx.send(fb).is_err()
                        {
                            return;
                        }
                    }
                }
            }
        });
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
        let pkts: Vec<Box<dyn rtc::rtcp::Packet>> = vec![Box::new(pli), Box::new(fir)];

        let remote_track = self
            .remote_tracks
            .lock()
            .ok()
            .and_then(|map| map.get(&media_ssrc).cloned());
        if let Some(track) = remote_track {
            let _ = track.write_rtcp(pkts).await;
        } else {
            let _ = self.video_track.write_rtcp(pkts).await;
        }
    }

    /// Create and fully gather a non-trickle SDP offer containing the relay candidate.
    pub async fn create_gathered_offer(&self) -> Result<String> {
        let offer = self
            .pc
            .create_offer(None)
            .await
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?;

        // Subscribe before `set_local_description` so a fast gathering completion cannot race.
        let mut gather_rx = self.gather_complete_tx.subscribe();
        gather_rx.borrow_and_update();

        self.pc
            .set_local_description(offer)
            .await
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?;

        // Wait up to 6s for TURN relay candidate gathering to complete.
        let _ = tokio::time::timeout(Duration::from_secs(6), gather_rx.changed()).await;

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
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?;

        // Update outbound payload types from the negotiated sender parameters so RTP writes
        // match the payload types accepted in Chime's SDP answer.
        if let Ok(params) = self.audio_sender.get_parameters().await
            && let Some(codec) = params.rtp_parameters.codecs.first()
        {
            self.audio_payload_type
                .store(codec.payload_type, Ordering::Relaxed);
        }
        if let Ok(params) = self.video_sender.get_parameters().await {
            let h264_pt = params
                .rtp_parameters
                .codecs
                .iter()
                .find(|c| c.rtp_codec.mime_type.eq_ignore_ascii_case(MIME_TYPE_H264))
                .or_else(|| params.rtp_parameters.codecs.first())
                .map(|c| c.payload_type);
            if let Some(pt) = h264_pt {
                self.video_payload_type.store(pt, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    /// Cloneable handle for writing outbound H.264 frames on a dedicated async task without
    /// blocking the main audio/signaling select loop during large IDR keyframe bursts.
    pub fn video_sender(&self) -> VideoTrackSender {
        VideoTrackSender {
            video_track: Arc::clone(&self.video_track),
            video_ssrc: self.video_ssrc,
            video_payload_type: Arc::clone(&self.video_payload_type),
            packets_sent: Arc::clone(&self.packets_sent),
            bytes_sent: Arc::clone(&self.bytes_sent),
            total_packets_sent: Arc::clone(&self.total_packets_sent),
            total_bytes_sent: Arc::clone(&self.total_bytes_sent),
        }
    }

    /// Write one encoded 20ms Opus audio packet onto the local track (zero-copy `Bytes`).
    pub async fn write_opus_frame(&self, opus_bytes: Bytes) -> Result<()> {
        let len = opus_bytes.len() as u64;
        let sample = Sample {
            data: opus_bytes,
            duration: Duration::from_millis(20),
            ..Sample::new(Instant::now())
        };
        self.audio_track
            .write_sample(
                self.audio_ssrc,
                self.audio_payload_type.load(Ordering::Relaxed),
                &sample,
                &[],
            )
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
        self.video_sender()
            .write_h264_frame(annex_b, duration)
            .await
    }

    pub fn connection_state(&self) -> RTCPeerConnectionState {
        self.connection_state
            .lock()
            .map(|guard| *guard)
            .unwrap_or(RTCPeerConnectionState::Closed)
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

/// Dedicated outbound H.264 RTP track sender so video frame writes run concurrently with
/// 20 ms Opus audio and Chime signaling.
#[derive(Clone)]
pub struct VideoTrackSender {
    video_track: Arc<TrackLocalStaticSample>,
    video_ssrc: u32,
    video_payload_type: Arc<AtomicU8>,
    packets_sent: Arc<AtomicU64>,
    bytes_sent: Arc<AtomicU64>,
    total_packets_sent: Arc<AtomicU64>,
    total_bytes_sent: Arc<AtomicU64>,
}

impl VideoTrackSender {
    pub async fn write_h264_frame(&self, annex_b: Vec<u8>, duration: Duration) -> Result<()> {
        let len = annex_b.len() as u64;
        let sample = Sample {
            data: Bytes::from(annex_b),
            duration,
            ..Sample::new(Instant::now())
        };
        self.video_track
            .write_sample(
                self.video_ssrc,
                self.video_payload_type.load(Ordering::Relaxed),
                &sample,
                &[],
            )
            .await
            .map_err(|e| HuddleError::WebRtc(e.to_string()))?;
        self.packets_sent.fetch_add(1, Ordering::Relaxed);
        self.total_packets_sent.fetch_add(1, Ordering::Relaxed);
        self.bytes_sent.fetch_add(len, Ordering::Relaxed);
        self.total_bytes_sent.fetch_add(len, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::atomic::AtomicUsize;

    #[derive(Clone)]
    struct NoopHandler;

    #[async_trait::async_trait]
    impl PeerConnectionEventHandler for NoopHandler {}

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

    #[test]
    fn filter_udp_turn_uris_handles_case_schemes_and_empty() {
        assert!(filter_udp_turn_uris(&[]).is_empty());
        let uris = vec![
            "TURN:ice.example:3478?transport=UDP".to_owned(),
            "turn:ice.example:3478".to_owned(),
            "TURN:ice.example:3478?TRANSPORT=TCP".to_owned(),
            "turns:ice.example:443?transport=udp".to_owned(),
            "stun:stun.example:3478".to_owned(),
        ];
        assert_eq!(
            filter_udp_turn_uris(&uris),
            vec![
                "TURN:ice.example:3478?transport=UDP".to_owned(),
                "turn:ice.example:3478".to_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn chime_media_engine_for_caps_orders_av1_vs_h264_deterministically() {
        async fn video_m_line(caps: crate::backend::MediaCapabilities) -> String {
            let media = chime_media_engine_for_caps(caps).unwrap();
            let pc = PeerConnectionBuilder::new()
                .with_media_engine(media)
                .with_handler(Arc::new(NoopHandler))
                .with_udp_addrs(vec!["127.0.0.1:0"])
                .build()
                .await
                .unwrap();
            pc.add_transceiver_from_kind(RtpCodecKind::Video, None)
                .await
                .unwrap();
            let offer = pc.create_offer(None).await.unwrap();
            let line = offer
                .sdp
                .lines()
                .find(|l| l.starts_with("m=video "))
                .expect("SDP must include m=video")
                .to_owned();
            let _ = pc.close().await;
            line
        }

        // M3+ (HW AV1 + HW H.264) -> AV1 (PT 45) advertised ahead of H.264 (PT 108).
        let m3_line = video_m_line(crate::backend::MediaCapabilities {
            hw_h264_decode: true,
            hw_av1_decode: true,
            ..Default::default()
        })
        .await;
        let av1_pos_m3 = m3_line.find(" 45").expect("AV1 PT 45");
        let h264_pos_m3 = m3_line.find(" 108").expect("H.264 PT 108");
        assert!(
            av1_pos_m3 < h264_pos_m3,
            "M3+ should advertise AV1 before H.264: {m3_line}"
        );

        // M1/M2 (HW H.264 only, no HW AV1) -> H.264 (PT 108) advertised ahead of AV1 (PT 45).
        let m1_line = video_m_line(crate::backend::MediaCapabilities {
            hw_h264_decode: true,
            hw_av1_decode: false,
            ..Default::default()
        })
        .await;
        let av1_pos_m1 = m1_line.find(" 45").expect("AV1 PT 45");
        let h264_pos_m1 = m1_line.find(" 108").expect("H.264 PT 108");
        assert!(
            h264_pos_m1 < av1_pos_m1,
            "M1/M2 should advertise H.264 before AV1: {m1_line}"
        );
    }

    #[test]
    fn classify_rtcp_video_feedback_handles_pli_fir_remb_and_others() {
        use rtc::rtcp::receiver_report::ReceiverReport;

        let pli = PictureLossIndication {
            sender_ssrc: 1,
            media_ssrc: 2,
        };
        assert_eq!(
            classify_rtcp_video_feedback(&pli),
            Some(VideoSenderFeedback::ForceKeyframe)
        );

        let fir = FullIntraRequest {
            sender_ssrc: 1,
            media_ssrc: 2,
            fir: vec![FirEntry {
                ssrc: 2,
                sequence_number: 1,
            }],
        };
        assert_eq!(
            classify_rtcp_video_feedback(&fir),
            Some(VideoSenderFeedback::ForceKeyframe)
        );

        let remb_low = ReceiverEstimatedMaximumBitrate {
            sender_ssrc: 1,
            bitrate: 10_000.0,
            ssrcs: vec![2],
        };
        assert_eq!(
            classify_rtcp_video_feedback(&remb_low),
            Some(VideoSenderFeedback::TargetBitrateBps(100_000))
        );

        let remb_mid = ReceiverEstimatedMaximumBitrate {
            sender_ssrc: 1,
            bitrate: 1_500_000.0,
            ssrcs: vec![2],
        };
        assert_eq!(
            classify_rtcp_video_feedback(&remb_mid),
            Some(VideoSenderFeedback::TargetBitrateBps(1_500_000))
        );

        let remb_high = ReceiverEstimatedMaximumBitrate {
            sender_ssrc: 1,
            bitrate: 50_000_000.0,
            ssrcs: vec![2],
        };
        assert_eq!(
            classify_rtcp_video_feedback(&remb_high),
            Some(VideoSenderFeedback::TargetBitrateBps(10_000_000))
        );

        let remb_nan = ReceiverEstimatedMaximumBitrate {
            sender_ssrc: 1,
            bitrate: f32::NAN,
            ssrcs: vec![2],
        };
        assert_eq!(classify_rtcp_video_feedback(&remb_nan), None);

        let rr = ReceiverReport::default();
        assert_eq!(classify_rtcp_video_feedback(&rr), None);
    }

    #[tokio::test]
    async fn chime_rtc_peer_rejects_zero_udp_turn_uris_and_tracks_metrics_snapshot() {
        let (a_tx, _a_rx) = mpsc::unbounded_channel();
        let (v_tx, _v_rx) = mpsc::unbounded_channel();
        let (s_tx, _s_rx) = mpsc::unbounded_channel();

        // Zero UDP TURN URIs returns WebRtc error cleanly.
        let tcp_only = SdkTurnCredentials {
            uris: vec!["turns:ice.example:443?transport=tcp".into()],
            ..Default::default()
        };
        let err = ChimeRtcPeer::new(&tcp_only, a_tx.clone(), v_tx.clone(), s_tx.clone())
            .await
            .err()
            .expect("zero UDP TURN URIs must fail");
        assert!(matches!(err, HuddleError::WebRtc(ref m) if m.contains("no UDP TURN")));

        // Valid UDP TURN URI constructs peer; take_metrics_snapshot drains deltas & clamps RMS.
        let udp_turn = SdkTurnCredentials {
            username: Some("u".into()),
            password: Some("p".into()),
            uris: vec!["turn:127.0.0.1:3478?transport=udp".into()],
            ..Default::default()
        };
        let peer = ChimeRtcPeer::new(&udp_turn, a_tx, v_tx, s_tx)
            .await
            .expect("peer with UDP TURN builds");
        peer.packets_sent.store(50, Ordering::Relaxed);
        peer.bytes_sent.store(4_000, Ordering::Relaxed);
        peer.packets_received.store(40, Ordering::Relaxed);
        peer.bytes_received.store(3_000, Ordering::Relaxed);

        let snap1 = peer.take_metrics_snapshot(0.5);
        assert_eq!(snap1.mic_pps, 50.0);
        assert_eq!(snap1.mic_bitrate_bps, 32_000.0);
        assert_eq!(snap1.spk_pps, 40.0);
        assert_eq!(snap1.spk_bitrate_bps, 24_000.0);
        assert!((snap1.mic_audio_level - 16_383.5).abs() < 1.0);

        // Second call sees zeroed delta counters and clamped RMS bounds.
        let snap2 = peer.take_metrics_snapshot(2.0);
        assert_eq!(snap2.mic_pps, 0.0);
        assert_eq!(snap2.spk_pps, 0.0);
        assert_eq!(snap2.mic_audio_level, 32_767.0);

        let snap3 = peer.take_metrics_snapshot(-0.5);
        assert_eq!(snap3.mic_audio_level, 0.0);

        peer.close().await;
    }

    #[tokio::test]
    async fn chime_rtc_peer_gathers_relay_offer_and_renegotiates_receive_transceivers() {
        use rtc::stun::attributes::{ATTR_NONCE, ATTR_REALM};
        use rtc::stun::error_code::CODE_UNAUTHORIZED;
        use rtc::stun::message::{
            CLASS_ERROR_RESPONSE, CLASS_SUCCESS_RESPONSE, Getter as _, METHOD_ALLOCATE,
            METHOD_REFRESH, Message as StunMessage, MessageType,
        };
        use rtc::stun::textattrs::{Nonce, Realm};
        use rtc::turn::proto::lifetime::Lifetime;
        use rtc::turn::proto::relayaddr::RelayedAddress;

        let turn_sock = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("bind mock TURN socket");
        let turn_addr = turn_sock.local_addr().expect("mock TURN addr");
        let relay_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 51234);
        let allocations = Arc::new(AtomicUsize::new(0));
        let alloc_srv = Arc::clone(&allocations);

        let server = tokio::spawn(async move {
            let mut buf = vec![0u8; 2048];
            loop {
                let Ok((n, peer_addr)) = turn_sock.recv_from(&mut buf).await else {
                    break;
                };
                let mut msg = StunMessage::new();
                msg.raw = buf[..n].to_vec();
                if msg.decode().is_err() {
                    continue;
                }
                let mut resp = StunMessage::new();
                let built = match msg.typ.method {
                    METHOD_ALLOCATE if msg.get(ATTR_NONCE).is_err() => resp.build(&[
                        Box::new(msg.transaction_id),
                        Box::new(MessageType::new(METHOD_ALLOCATE, CLASS_ERROR_RESPONSE)),
                        Box::new(CODE_UNAUTHORIZED),
                        Box::new(Realm::new(ATTR_REALM, "chime.test".to_owned())),
                        Box::new(Nonce::new(ATTR_NONCE, "test-nonce".to_owned())),
                    ]),
                    METHOD_ALLOCATE => {
                        alloc_srv.fetch_add(1, Ordering::SeqCst);
                        resp.build(&[
                            Box::new(msg.transaction_id),
                            Box::new(MessageType::new(METHOD_ALLOCATE, CLASS_SUCCESS_RESPONSE)),
                            Box::new(RelayedAddress {
                                ip: relay_addr.ip(),
                                port: relay_addr.port(),
                            }),
                            Box::new(Lifetime(Duration::from_secs(600))),
                        ])
                    }
                    METHOD_REFRESH => {
                        let mut lifetime = Lifetime::default();
                        let release = lifetime.get_from(&msg).is_ok() && lifetime.0.is_zero();
                        resp.build(&[
                            Box::new(msg.transaction_id),
                            Box::new(MessageType::new(METHOD_REFRESH, CLASS_SUCCESS_RESPONSE)),
                            Box::new(Lifetime(if release {
                                Duration::ZERO
                            } else {
                                Duration::from_secs(600)
                            })),
                        ])
                    }
                    _ => continue,
                };
                if built.is_ok() {
                    let _ = turn_sock.send_to(&resp.raw, peer_addr).await;
                }
            }
        });

        let (a_tx, _a_rx) = mpsc::unbounded_channel();
        let (v_tx, _v_rx) = mpsc::unbounded_channel();
        let (s_tx, _s_rx) = mpsc::unbounded_channel();
        let turn = SdkTurnCredentials {
            username: Some("chime-user".into()),
            password: Some("chime-pass".into()),
            uris: vec![
                format!("turn:{turn_addr}?transport=udp"),
                format!("turns:{turn_addr}?transport=tcp"),
            ],
            ..Default::default()
        };

        let peer = ChimeRtcPeer::new(&turn, a_tx, v_tx, s_tx)
            .await
            .expect("ChimeRtcPeer builds against mock TURN");

        let offer_sdp = peer
            .create_gathered_offer()
            .await
            .expect("initial gathered relay offer succeeds");
        assert!(
            allocations.load(Ordering::SeqCst) >= 1,
            "TURN allocation must occur"
        );
        assert!(
            offer_sdp.contains("typ relay"),
            "offer must contain relay candidate:\n{offer_sdp}"
        );
        assert!(
            !offer_sdp.contains("typ host"),
            "Relay policy must exclude host candidates:\n{offer_sdp}"
        );
        assert!(
            offer_sdp.contains("a=rtpmap:111 opus/48000/2"),
            "offer must advertise Opus PT 111:\n{offer_sdp}"
        );
        assert!(
            offer_sdp.contains("minptime=10;useinbandfec=1"),
            "offer must include Opus fmtp:\n{offer_sdp}"
        );
        assert!(
            offer_sdp.contains("a=rtpmap:108 H264/90000"),
            "offer must advertise H.264 PT 108:\n{offer_sdp}"
        );
        assert!(
            offer_sdp.contains("urn:ietf:params:rtp-hdrext:sdes:mid"),
            "offer must include sdes:mid header extension:\n{offer_sdp}"
        );
        assert!(
            offer_sdp.contains("a=mid:0") && offer_sdp.contains("a=mid:1"),
            "offer must have mid 0 (audio) and mid 1 (video):\n{offer_sdp}"
        );

        // Renegotiate with 2 dynamic Recvonly video transceivers (mid 2 and mid 3).
        let (reneg_sdp, mids) = peer
            .prepare_video_receive_offer(2)
            .await
            .expect("renegotiation offer with 2 recvonly video slots succeeds");
        assert_eq!(mids, vec!["2".to_owned(), "3".to_owned()]);
        assert!(
            reneg_sdp.contains("typ relay"),
            "renegotiated offer must retain relay candidate:\n{reneg_sdp}"
        );
        assert!(
            reneg_sdp.contains("a=mid:2") && reneg_sdp.contains("a=mid:3"),
            "renegotiated offer must contain mid 2 and mid 3:\n{reneg_sdp}"
        );
        assert!(
            reneg_sdp.contains("a=rtpmap:45 AV1/90000"),
            "recvonly video slots must advertise AV1 PT 45:\n{reneg_sdp}"
        );
        assert!(
            reneg_sdp.contains("a=recvonly"),
            "recvonly video slots must be recvonly:\n{reneg_sdp}"
        );

        peer.close().await;
        server.abort();
    }
}
