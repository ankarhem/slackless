//! Amazon Chime SDK signaling WebSocket client (`[0x05][SdkSignalFrame]` protobuf framing).

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use futures_util::{SinkExt as _, StreamExt as _};
use prost::Message as _;
use slackless_types::UserId;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use crate::proto::{
    SdkAudioControlFrame, SdkClientDetails, SdkClientMetricFrame, SdkDataMessageFrame,
    SdkDataMessagePayload, SdkIndexFrame, SdkJoinFlags, SdkJoinFrame, SdkLeaveFrame, SdkMetric,
    SdkPingPongFrame, SdkPingPongType, SdkRemoteVideoUpdateFrame, SdkServerSideNetworkAdaption,
    SdkSignalFrame, SdkStreamDescriptor, SdkStreamMediaType, SdkStreamMetricFrame,
    SdkStreamServiceType, SdkSubscribeFrame, SdkVideoQualityAdaptationPreference,
    SdkVideoSubscriptionConfiguration, sdk_metric, sdk_signal_frame,
};
use crate::{HuddleCredentials, HuddleError, Result};

/// Outbound RTC frame type prefix byte required by Chime signaling.
pub const FRAME_TYPE_RTC: u8 = 0x05;
/// Alternate inbound prefix byte accepted by Chime signaling.
pub const FRAME_TYPE_ALT: u8 = 0x02;

/// Chime signaling stream wrapper.
pub struct ChimeSignalClient {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl ChimeSignalClient {
    /// Dial `<SignalingUrl>?X-Chime-Control-Protocol-Version=3&X-Amzn-Chime-Send-Close-On-Error=1`
    /// with subprotocol auth `["_aws_wt_session", "<JoinToken>"]`.
    pub async fn connect(creds: &HuddleCredentials) -> Result<Self> {
        let url = build_signaling_url(&creds.signaling_url)?;
        let mut req = url
            .into_client_request()
            .map_err(|e| HuddleError::Signaling(e.to_string()))?;
        let proto_header = format!("_aws_wt_session, {}", creds.join_token);
        let value = HeaderValue::from_str(&proto_header)
            .map_err(|e| HuddleError::Signaling(format!("invalid join token header: {e}")))?;
        req.headers_mut().insert("Sec-WebSocket-Protocol", value);
        req.headers_mut()
            .insert("Origin", HeaderValue::from_static("https://app.slack.com"));

        let (ws, _resp) = connect_async(req)
            .await
            .map_err(|e| HuddleError::Signaling(e.to_string()))?;
        Ok(Self { ws })
    }

    /// Encode and send a single `[0x05][SdkSignalFrame]` binary websocket message.
    pub async fn send_frame(&mut self, frame: &SdkSignalFrame) -> Result<()> {
        let payload = encode_wire_frame(frame);
        self.ws
            .send(WsMessage::Binary(payload.into()))
            .await
            .map_err(|e| HuddleError::Signaling(e.to_string()))
    }

    /// Receive the next decoded `SdkSignalFrame`, skipping websocket ping/pong control frames.
    /// Returns `Ok(None)` on clean remote close.
    pub async fn next_frame(&mut self) -> Result<Option<SdkSignalFrame>> {
        while let Some(item) = self.ws.next().await {
            match item.map_err(|e| HuddleError::Signaling(e.to_string()))? {
                WsMessage::Binary(bytes) => {
                    if let Some(frame) = decode_wire_frame(&bytes)? {
                        return Ok(Some(frame));
                    }
                }
                WsMessage::Ping(data) => {
                    let _ = self.ws.send(WsMessage::Pong(data)).await;
                }
                WsMessage::Pong(_) | WsMessage::Text(_) | WsMessage::Frame(_) => {}
                WsMessage::Close(_) => return Ok(None),
            }
        }
        Ok(None)
    }

    /// Close the websocket cleanly.
    pub async fn close(mut self) {
        let _ = self.ws.close(None).await;
    }
}

/// Append the Chime protocol query parameters to `SignalingUrl`.
pub fn build_signaling_url(base: &str) -> Result<String> {
    let mut url = url::Url::parse(base)
        .map_err(|e| HuddleError::Signaling(format!("invalid signaling url: {e}")))?;
    url.query_pairs_mut()
        .append_pair("X-Chime-Control-Protocol-Version", "3")
        .append_pair("X-Amzn-Chime-Send-Close-On-Error", "1");
    Ok(url.into())
}

/// Prefix protobuf bytes with `FRAME_TYPE_RTC` (`0x05`).
pub fn encode_wire_frame(frame: &SdkSignalFrame) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + frame.encoded_len());
    buf.push(FRAME_TYPE_RTC);
    frame
        .encode(&mut buf)
        .expect("Vec<u8> prost encode cannot fail");
    buf
}

/// Strip the leading Chime frame-type byte (`0x05` or `0x02`) and decode `SdkSignalFrame`.
pub fn decode_wire_frame(bytes: &[u8]) -> Result<Option<SdkSignalFrame>> {
    let Some((&prefix, rest)) = bytes.split_first() else {
        return Ok(None);
    };
    if prefix != FRAME_TYPE_RTC && prefix != FRAME_TYPE_ALT {
        return Ok(None);
    }
    let frame = SdkSignalFrame::decode(rest)
        .map_err(|e| HuddleError::Signaling(format!("protobuf decode failed: {e}")))?;
    Ok(Some(frame))
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Build the initial Chime `JOIN` frame (`protocol_version = 2`, full stream-update flags, and
/// server-side network adaptation so `video_subscription_configuration` / `REMOTE_VIDEO_UPDATE`
/// are active on the SFU).
pub fn build_join_frame(audio_session_id: u64) -> SdkSignalFrame {
    let flags = SdkJoinFlags::HasStreamUpdate as u32
        | SdkJoinFlags::CompleteVideoSourcesList as u32
        | SdkJoinFlags::ExcludeSelfContentInIndex as u32;
    let adaption = SdkServerSideNetworkAdaption::BandwidthProbingAndVideoQualityAdaption as i32;
    SdkSignalFrame {
        timestamp_ms: Some(now_ms()),
        r#type: Some(sdk_signal_frame::Type::Join as i32),
        join: Some(SdkJoinFrame {
            protocol_version: Some(2),
            max_num_of_videos: Some(25),
            flags: Some(flags),
            client_details: Some(SdkClientDetails {
                app_name: Some("slackless".to_owned()),
                app_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
                platform_name: Some(std::env::consts::OS.to_owned()),
                platform_version: Some("1.0".to_owned()),
                client_source: Some("amazon-chime-sdk-js".to_owned()),
                chime_sdk_version: Some("3.19.0".to_owned()),
                client_utc_offset: Some("+00:00".to_owned()),
                os_name: Some(std::env::consts::OS.to_owned()),
                os_version: Some("14.0".to_owned()),
                ..Default::default()
            }),
            audio_session_id: Some(audio_session_id),
            // Plaintext SDP answers avoid Chime's XOR-diff renegotiation delta format while adding
            // only ~2 KB per SUBSCRIBE_ACK.
            wants_compressed_sdp: Some(false),
            server_side_network_adaption: Some(adaption),
            supported_server_side_network_adaptions: vec![adaption],
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// One remote video or `#content` screen-share stream advertised by Chime's `INDEX` frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteVideoSource {
    pub stream_id: u32,
    pub group_id: u32,
    pub attendee_id: String,
    pub user_id: UserId,
    pub is_content: bool,
    pub max_bitrate_kbps: u32,
}

/// Extract remote camera and `#content` screen-share sources from an `SdkIndexFrame`,
/// deduplicating simulcast layers by `(attendee_id, group_id)` (picking the highest-bitrate
/// stream per group, matching `amazon-chime-sdk-js`).
pub fn parse_index_sources(
    index: &SdkIndexFrame,
    self_attendee_id: &str,
) -> Vec<RemoteVideoSource> {
    parse_index_sources_with_lookup(index, self_attendee_id, &HashMap::new())
}

/// Same as [`parse_index_sources`], using `attendee_to_user` as a fallback when a `#content`
/// stream descriptor omits `external_user_id`.
pub fn parse_index_sources_with_lookup(
    index: &SdkIndexFrame,
    self_attendee_id: &str,
    attendee_to_user: &HashMap<String, UserId>,
) -> Vec<RemoteVideoSource> {
    let mut best_by_group: HashMap<(String, u32), RemoteVideoSource> = HashMap::new();
    let mut order: Vec<(String, u32)> = Vec::new();
    for src in &index.sources {
        let Some(stream_id) = src.stream_id else {
            continue;
        };
        let attendee_id = src.attendee_id.clone().unwrap_or_default();
        if attendee_id.is_empty() || attendee_id == self_attendee_id {
            continue;
        }
        let external = src.external_user_id.as_deref().unwrap_or("");
        let is_content = external.ends_with("#content") || attendee_id.ends_with("#content");
        let base_attendee = attendee_id.strip_suffix("#content").unwrap_or(&attendee_id);
        let user_id = parse_external_user_id(external)
            .or_else(|| attendee_to_user.get(base_attendee).cloned())
            .unwrap_or_else(|| UserId::from(base_attendee));
        let group_id = src.group_id.unwrap_or(1);
        let max_bitrate_kbps = src
            .max_bitrate_kbps
            .unwrap_or(if is_content { 1500 } else { 350 });
        let candidate = RemoteVideoSource {
            stream_id,
            group_id,
            attendee_id: attendee_id.clone(),
            user_id,
            is_content,
            max_bitrate_kbps,
        };
        let key = (attendee_id, group_id);
        match best_by_group.get(&key) {
            Some(prev)
                if prev.max_bitrate_kbps > candidate.max_bitrate_kbps
                    || (prev.max_bitrate_kbps == candidate.max_bitrate_kbps
                        && prev.stream_id >= candidate.stream_id) => {}
            Some(_) => {
                best_by_group.insert(key, candidate);
            }
            None => {
                order.push(key.clone());
                best_by_group.insert(key, candidate);
            }
        }
    }
    order
        .into_iter()
        .filter_map(|k| best_by_group.remove(&k))
        .collect()
}

/// Build a Chime `SUBSCRIBE` frame for audio-only (`send_video = false`) or full `DUPLEX`
/// audio + video (`send_video = true` or non-empty `receive_sources`).
///
/// `receive_sources` carries `(sdp_mid, RemoteVideoSource)` pairs assigned to mid 2+ receive
/// transceivers; `receive_stream_ids` prepends `0` for the mid-1 send video slot when `send_video`
/// is true so Chime's `fixUpSubscriptionOrder` lines up with every video m-section.
pub fn build_subscribe_frame(
    creds: &HuddleCredentials,
    sdp_offer: &str,
    wants_compressed_sdp: bool,
    audio_muted: bool,
    send_video: bool,
    receive_sources: &[(String, RemoteVideoSource)],
) -> Result<SdkSignalFrame> {
    let (plain, compressed) = if wants_compressed_sdp {
        (None, Some(deflate_sdp(sdp_offer)?))
    } else {
        (Some(sdp_offer.to_owned()), None)
    };
    let mut send_streams = vec![SdkStreamDescriptor {
        stream_id: Some(1),
        framerate: Some(15),
        max_bitrate_kbps: Some(600),
        track_label: Some("AmazonChimeExpressAudio".to_owned()),
        group_id: Some(1),
        avg_bitrate_bps: Some(400_000),
        attendee_id: Some(creds.attendee_id.clone()),
        media_type: Some(SdkStreamMediaType::Audio as i32),
        external_user_id: Some(creds.external_user_id.clone()),
        ..Default::default()
    }];
    if send_video {
        send_streams.push(SdkStreamDescriptor {
            stream_id: Some(2),
            framerate: Some(15),
            max_bitrate_kbps: Some(1500),
            track_label: Some("AmazonChimeExpressVideo".to_owned()),
            group_id: Some(2),
            avg_bitrate_bps: Some(800_000),
            attendee_id: Some(creds.attendee_id.clone()),
            media_type: Some(SdkStreamMediaType::Video as i32),
            external_user_id: Some(creds.external_user_id.clone()),
            width: Some(640),
            height: Some(360),
        });
    }
    // Mid 1 is always present in our SDP offer (local camera slot), so whenever we are sending
    // video or subscribing to mid-2+ remote video streams, prepend `0` for mid 1 so
    // `receive_stream_ids` lines up positionally with every video m-section.
    let needs_mid1_placeholder = send_video || !receive_sources.is_empty();
    let mut receive_stream_ids =
        Vec::with_capacity(receive_sources.len() + usize::from(needs_mid1_placeholder));
    if needs_mid1_placeholder {
        receive_stream_ids.push(0);
    }
    receive_stream_ids.extend(receive_sources.iter().map(|(_, s)| s.stream_id));
    let video_subscription_configuration = build_video_subscriptions(receive_sources);
    let duplex = if send_video {
        SdkStreamServiceType::Duplex
    } else {
        SdkStreamServiceType::Rx
    };
    Ok(SdkSignalFrame {
        timestamp_ms: Some(now_ms()),
        r#type: Some(sdk_signal_frame::Type::Subscribe as i32),
        sub: Some(SdkSubscribeFrame {
            duplex: Some(duplex as i32),
            send_streams,
            receive_stream_ids,
            sdp_offer: plain,
            compressed_sdp_offer: compressed,
            audio_host: Some(creds.audio_host_url.clone()),
            audio_checkin: Some(false),
            audio_muted: Some(audio_muted),
            video_subscription_configuration,
        }),
        ..Default::default()
    })
}

/// Build the Chime `SUBSCRIBE` frame for a dedicated `#content` screen-share publisher attendee.
pub fn build_content_subscribe_frame(
    creds: &HuddleCredentials,
    sdp_offer: &str,
    wants_compressed_sdp: bool,
) -> Result<SdkSignalFrame> {
    let (plain, compressed) = if wants_compressed_sdp {
        (None, Some(deflate_sdp(sdp_offer)?))
    } else {
        (Some(sdp_offer.to_owned()), None)
    };
    let send_streams = vec![
        SdkStreamDescriptor {
            stream_id: Some(1),
            framerate: Some(15),
            max_bitrate_kbps: Some(600),
            track_label: Some("AmazonChimeExpressAudio".to_owned()),
            group_id: Some(1),
            avg_bitrate_bps: Some(400_000),
            attendee_id: Some(creds.attendee_id.clone()),
            media_type: Some(SdkStreamMediaType::Audio as i32),
            external_user_id: Some(creds.external_user_id.clone()),
            ..Default::default()
        },
        SdkStreamDescriptor {
            stream_id: Some(2),
            framerate: Some(30),
            max_bitrate_kbps: Some(3500),
            track_label: Some("AmazonChimeExpressVideo".to_owned()),
            group_id: Some(2),
            avg_bitrate_bps: Some(3_000_000),
            attendee_id: Some(creds.attendee_id.clone()),
            media_type: Some(SdkStreamMediaType::Video as i32),
            external_user_id: Some(creds.external_user_id.clone()),
            width: Some(1920),
            height: Some(1200),
        },
    ];
    Ok(SdkSignalFrame {
        timestamp_ms: Some(now_ms()),
        r#type: Some(sdk_signal_frame::Type::Subscribe as i32),
        sub: Some(SdkSubscribeFrame {
            duplex: Some(SdkStreamServiceType::Duplex as i32),
            send_streams,
            receive_stream_ids: vec![0],
            sdp_offer: plain,
            compressed_sdp_offer: compressed,
            audio_host: Some(creds.audio_host_url.clone()),
            audio_checkin: Some(false),
            audio_muted: Some(true),
            video_subscription_configuration: Vec::new(),
        }),
        ..Default::default()
    })
}

/// Build the audio-only Chime `SUBSCRIBE` frame (note: `duplex = RX` describes video direction
/// even while sending audio, and `framerate = 15` is expected by Chime's SFU on audio too).
pub fn build_audio_subscribe_frame(
    creds: &HuddleCredentials,
    sdp_offer: &str,
    wants_compressed_sdp: bool,
    audio_muted: bool,
) -> Result<SdkSignalFrame> {
    build_subscribe_frame(
        creds,
        sdp_offer,
        wants_compressed_sdp,
        audio_muted,
        false,
        &[],
    )
}

/// Convert `(mid, RemoteVideoSource)` pairs into Chime `SdkVideoSubscriptionConfiguration` entries
/// matching official Slack's exact wire parameters (`stream_id: None`, `group_id: Some(...)`,
/// `priority: 4_294_967_285`, `quality_adaptation_preference: 1`).
pub fn build_video_subscriptions(
    sources: &[(String, RemoteVideoSource)],
) -> Vec<SdkVideoSubscriptionConfiguration> {
    sources
        .iter()
        .map(|(mid, src)| SdkVideoSubscriptionConfiguration {
            mid: Some(mid.clone()),
            attendee_id: Some(src.attendee_id.clone()),
            stream_id: None,
            priority: Some(4_294_967_285),
            target_bitrate_kbps: Some(src.max_bitrate_kbps.clamp(350, 2500)),
            group_id: Some(src.group_id),
            quality_adaptation_preference: Some(
                SdkVideoQualityAdaptationPreference::MaintainResolution as i32,
            ),
        })
        .collect()
}

/// Build a Chime `REMOTE_VIDEO_UPDATE` frame to subscribe/unsubscribe remote video & screen streams.
pub fn build_remote_video_update_frame(
    added: Vec<SdkVideoSubscriptionConfiguration>,
    removed_mids: Vec<String>,
) -> SdkSignalFrame {
    SdkSignalFrame {
        timestamp_ms: Some(now_ms()),
        r#type: Some(sdk_signal_frame::Type::RemoteVideoUpdate as i32),
        remote_video_update: Some(SdkRemoteVideoUpdateFrame {
            added_or_updated_video_subscriptions: added,
            removed_video_subscription_mids: removed_mids,
        }),
        ..Default::default()
    }
}

/// Build a Chime `DATA_MESSAGE` frame for huddle reactions, hand raise, topic, or captions.
pub fn build_data_message_frame(topic: &str, data: Vec<u8>, lifetime_ms: u32) -> SdkSignalFrame {
    SdkSignalFrame {
        timestamp_ms: Some(now_ms()),
        r#type: Some(sdk_signal_frame::Type::DataMessage as i32),
        data_message: Some(SdkDataMessageFrame {
            messages: vec![SdkDataMessagePayload {
                topic: Some(topic.to_owned()),
                data: Some(data),
                lifetime_ms: Some(lifetime_ms),
                ..Default::default()
            }],
        }),
        ..Default::default()
    }
}

/// Build a Chime `AUDIO_CONTROL` mute/unmute frame.
pub fn build_audio_control_frame(muted: bool) -> SdkSignalFrame {
    SdkSignalFrame {
        timestamp_ms: Some(now_ms()),
        r#type: Some(sdk_signal_frame::Type::AudioControl as i32),
        audio_control: Some(SdkAudioControlFrame { muted: Some(muted) }),
        ..Default::default()
    }
}

/// Build a `PONG` reply to a server `PING_PONG` frame.
pub fn build_pong_frame(ping_id: u32) -> SdkSignalFrame {
    SdkSignalFrame {
        timestamp_ms: Some(now_ms()),
        r#type: Some(sdk_signal_frame::Type::PingPong as i32),
        ping_pong: Some(SdkPingPongFrame {
            r#type: Some(SdkPingPongType::Pong as i32),
            ping_id: Some(ping_id),
        }),
        ..Default::default()
    }
}

/// Snapshot of real audio/transport stats sent every 1s inside `CLIENT_METRIC`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClientMetricsSnapshot {
    pub stun_rtt_ms: f64,
    pub mic_pps: f64,
    pub mic_bitrate_bps: f64,
    pub mic_audio_level: f64,
    pub spk_pps: f64,
    pub spk_bitrate_bps: f64,
}

/// Build a non-empty Chime `CLIENT_METRIC` frame (Chime drops sessions that go silent or send
/// empty metric payloads).
pub fn build_client_metric_frame(stats: ClientMetricsSnapshot) -> SdkSignalFrame {
    SdkSignalFrame {
        timestamp_ms: Some(now_ms()),
        r#type: Some(sdk_signal_frame::Type::ClientMetric as i32),
        client_metric: Some(SdkClientMetricFrame {
            global_metrics: vec![SdkMetric {
                r#type: Some(sdk_metric::Type::StunRttMs as i32),
                value: Some(stats.stun_rtt_ms.max(1.0)),
            }],
            stream_metric_frames: vec![SdkStreamMetricFrame {
                stream_id: Some(1),
                group_id: Some(1),
                media_type: Some(SdkStreamMediaType::Audio as i32),
                metrics: vec![
                    SdkMetric {
                        r#type: Some(sdk_metric::Type::RtcMicPps as i32),
                        value: Some(stats.mic_pps),
                    },
                    SdkMetric {
                        r#type: Some(sdk_metric::Type::RtcMicBitrate as i32),
                        value: Some(stats.mic_bitrate_bps),
                    },
                    SdkMetric {
                        r#type: Some(sdk_metric::Type::RtcMicAudioLevel as i32),
                        value: Some(stats.mic_audio_level),
                    },
                    SdkMetric {
                        r#type: Some(sdk_metric::Type::RtcSpkPps as i32),
                        value: Some(stats.spk_pps),
                    },
                    SdkMetric {
                        r#type: Some(sdk_metric::Type::RtcSpkBitrate as i32),
                        value: Some(stats.spk_bitrate_bps),
                    },
                ],
            }],
        }),
        ..Default::default()
    }
}

/// Build the Chime `LEAVE` teardown frame (the official Slack teardown mechanism).
pub fn build_leave_frame() -> SdkSignalFrame {
    SdkSignalFrame {
        timestamp_ms: Some(now_ms()),
        r#type: Some(sdk_signal_frame::Type::Leave as i32),
        leave: Some(SdkLeaveFrame {}),
        ..Default::default()
    }
}

/// Raw-deflate compress an SDP offer (matches `amazon-chime-sdk-js`'s `pako.deflateRaw`).
pub fn deflate_sdp(sdp: &str) -> Result<Vec<u8>> {
    let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
    enc.write_all(sdp.as_bytes())
        .map_err(|e| HuddleError::Signaling(format!("sdp deflate failed: {e}")))?;
    enc.finish()
        .map_err(|e| HuddleError::Signaling(format!("sdp deflate finish failed: {e}")))
}

/// Decode SDP answer from `SdkSubscribeAckFrame` (supporting both raw-deflate `compressed_sdp_answer`
/// and plain `sdp_answer`).
pub fn decode_sdp_answer(sdp_answer: Option<&str>, compressed: Option<&[u8]>) -> Result<String> {
    decode_sdp_answer_with_prev(sdp_answer, compressed, None)
}

/// Decode SDP answer from `SdkSubscribeAckFrame`, handling Chime's renegotiation XOR-diff
/// compression against `prev_sdp_answer` whenever `compressed_sdp_answer` is used.
pub fn decode_sdp_answer_with_prev(
    sdp_answer: Option<&str>,
    compressed: Option<&[u8]>,
    prev_sdp_answer: Option<&str>,
) -> Result<String> {
    if let Some(plain) = sdp_answer.filter(|s| !s.is_empty()) {
        return Ok(plain.to_owned());
    }
    if let Some(raw) = compressed.filter(|b| !b.is_empty()) {
        let mut dec = DeflateDecoder::new(raw);
        let mut inflated = Vec::new();
        dec.read_to_end(&mut inflated)
            .map_err(|e| HuddleError::Signaling(format!("sdp inflate failed: {e}")))?;
        if let Some(prev) = prev_sdp_answer {
            if inflated.contains(&0) {
                let prev_bytes = prev.as_bytes();
                let mut restored = Vec::with_capacity(inflated.len());
                for (i, &b) in inflated.iter().enumerate() {
                    let p = prev_bytes.get(i).copied().unwrap_or(0);
                    restored.push(b ^ p);
                }
                return String::from_utf8(restored)
                    .map_err(|e| HuddleError::Signaling(format!("sdp xor-diff utf8 failed: {e}")));
            }
        }
        return String::from_utf8(inflated)
            .map_err(|e| HuddleError::Signaling(format!("sdp inflate utf8 failed: {e}")));
    }
    Err(HuddleError::Signaling(
        "SUBSCRIBE_ACK contained neither sdp_answer nor compressed_sdp_answer".to_owned(),
    ))
}

/// Extract a Slack `UserId` from Chime's `ExternalUserId`.
///
/// Slack formats `ExternalUserId` as `<team>-<call_id>-<user_id>-<nonce>` (for example
/// `T02HD8VA1-R0CLDM5DTPE-UGKSG83K3-12688436843750`), optionally suffixed with `#content`.
/// Prefer the segment starting with `'U'` or `'W'` (Slack workspace/enterprise user IDs) before
/// falling back to the final hyphen-delimited segment.
pub fn parse_external_user_id(external: &str) -> Option<UserId> {
    let base = external.strip_suffix("#content").unwrap_or(external).trim();
    if base.is_empty() {
        return None;
    }
    if let Some(uid) = base.split('-').find(|part| {
        (part.starts_with('U') || part.starts_with('W'))
            && part.len() >= 2
            && part.bytes().all(|b| b.is_ascii_alphanumeric())
    }) {
        return Some(UserId::from(uid));
    }
    let candidate = base.rsplit('-').next()?.trim();
    if candidate.is_empty() {
        None
    } else {
        Some(UserId::from(candidate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::SdkSubscribeAckFrame;

    #[test]
    fn wire_frame_round_trips_with_rtc_prefix() {
        let join = build_join_frame(42);
        let wire = encode_wire_frame(&join);
        assert_eq!(wire[0], FRAME_TYPE_RTC);
        let decoded = decode_wire_frame(&wire).unwrap().unwrap();
        assert_eq!(decoded.r#type, Some(sdk_signal_frame::Type::Join as i32));
        assert_eq!(decoded.join.unwrap().audio_session_id, Some(42));
    }

    #[test]
    fn compressed_sdp_round_trips() {
        let sdp = "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\n";
        let compressed = deflate_sdp(sdp).unwrap();
        let ack = SdkSubscribeAckFrame {
            compressed_sdp_answer: Some(compressed),
            ..Default::default()
        };
        let restored = decode_sdp_answer(
            ack.sdp_answer.as_deref(),
            ack.compressed_sdp_answer.as_deref(),
        )
        .unwrap();
        assert_eq!(restored, sdp);

        // Chime XOR-diffs renegotiated compressed SDP answers against the previous SDP answer.
        let sdp2 = "v=0\r\no=- 1 3 IN IP4 127.0.0.1\r\ns=-\r\n";
        let xor_bytes: Vec<u8> = sdp2
            .as_bytes()
            .iter()
            .enumerate()
            .map(|(i, &b)| b ^ sdp.as_bytes().get(i).copied().unwrap_or(0))
            .collect();
        let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&xor_bytes).unwrap();
        let xor_compressed = enc.finish().unwrap();
        let restored2 =
            decode_sdp_answer_with_prev(None, Some(&xor_compressed), Some(sdp)).unwrap();
        assert_eq!(restored2, sdp2);
    }

    #[test]
    fn external_user_id_extracts_slack_user() {
        assert_eq!(
            parse_external_user_id("T0123-R0987654321-U0ALICE"),
            Some(UserId::from("U0ALICE"))
        );
        assert_eq!(
            parse_external_user_id("T0123-R0987654321-U0BOB#content"),
            Some(UserId::from("U0BOB"))
        );
        assert_eq!(
            parse_external_user_id("T02HD8VA1-R0CLDM5DTPE-UGKSG83K3-12688436843750"),
            Some(UserId::from("UGKSG83K3"))
        );
        assert_eq!(
            parse_external_user_id("T02HD8VA1-R0CLDM5DTPE-W01234567-999#content"),
            Some(UserId::from("W01234567"))
        );
    }

    #[test]
    fn signaling_url_appends_chime_protocol_params() {
        let built = build_signaling_url("wss://signal.z3.ec1.m.chime.aws/control/abc").unwrap();
        assert!(built.contains("X-Chime-Control-Protocol-Version=3"));
        assert!(built.contains("X-Amzn-Chime-Send-Close-On-Error=1"));
    }

    #[test]
    fn parse_index_deduplicates_simulcast_and_resolves_missing_external_user_id() {
        let mut lookup = HashMap::new();
        lookup.insert("att-bob".to_owned(), UserId::from("U0BOB"));

        let index = SdkIndexFrame {
            sources: vec![
                // Two simulcast layers for Alice's camera (same attendee + group).
                SdkStreamDescriptor {
                    stream_id: Some(7),
                    group_id: Some(2),
                    max_bitrate_kbps: Some(300),
                    attendee_id: Some("att-alice".to_owned()),
                    external_user_id: Some("T1-R1-U0ALICE".to_owned()),
                    ..Default::default()
                },
                SdkStreamDescriptor {
                    stream_id: Some(8),
                    group_id: Some(2),
                    max_bitrate_kbps: Some(1200),
                    attendee_id: Some("att-alice".to_owned()),
                    external_user_id: Some("T1-R1-U0ALICE".to_owned()),
                    ..Default::default()
                },
                // Bob's #content screen share omitting external_user_id.
                SdkStreamDescriptor {
                    stream_id: Some(12),
                    group_id: Some(5),
                    max_bitrate_kbps: Some(2000),
                    attendee_id: Some("att-bob#content".to_owned()),
                    external_user_id: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let parsed = parse_index_sources_with_lookup(&index, "att-self", &lookup);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].stream_id, 8);
        assert_eq!(parsed[0].user_id, UserId::from("U0ALICE"));
        assert!(!parsed[0].is_content);
        assert_eq!(parsed[1].stream_id, 12);
        assert_eq!(parsed[1].user_id, UserId::from("U0BOB"));
        assert!(parsed[1].is_content);
    }

    #[test]
    fn subscribe_prepends_zero_placeholder_for_mid1_when_receiving_remote_video() {
        let creds = HuddleCredentials {
            channel: slackless_types::ChannelId::from("C1"),
            call_id: "R1".to_owned(),
            meeting_id: "M1".to_owned(),
            signaling_url: "wss://example.invalid".to_owned(),
            audio_host_url: "audio.invalid:3478".to_owned(),
            turn_control_url: String::new(),
            attendee_id: "att-self".to_owned(),
            external_user_id: "T1-R1-U0SELF".to_owned(),
            join_token: "tok".to_owned(),
            thread_root_ts: None,
            canvas_file_id: None,
            huddle_link: None,
        };
        let remote = RemoteVideoSource {
            stream_id: 12,
            group_id: 5,
            attendee_id: "att-bob#content".to_owned(),
            user_id: UserId::from("U0BOB"),
            is_content: true,
            max_bitrate_kbps: 2000,
        };
        let frame = build_subscribe_frame(
            &creds,
            "v=0\r\n",
            false,
            false,
            false, // camera OFF, still receiving Bob's screen share on mid 2
            &[("2".to_owned(), remote)],
        )
        .unwrap();
        let sub = frame.sub.unwrap();
        assert_eq!(sub.receive_stream_ids, vec![0, 12]);
        assert_eq!(sub.video_subscription_configuration.len(), 1);
        assert_eq!(
            sub.video_subscription_configuration[0].mid.as_deref(),
            Some("2")
        );
    }

    #[tokio::test]
    #[allow(clippy::result_large_err)]
    async fn mock_chime_signaling_handshake_and_leave() {
        use tokio::net::TcpListener;
        use tokio_tungstenite::accept_hdr_async;
        use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut saw_subprotocol = false;
            let mut ws = accept_hdr_async(stream, |req: &Request, mut resp: Response| {
                if let Some(proto) = req.headers().get("sec-websocket-protocol") {
                    let s = proto.to_str().unwrap_or("");
                    if s.contains("_aws_wt_session") && s.contains("join-tok-123") {
                        saw_subprotocol = true;
                        resp.headers_mut().insert(
                            "Sec-WebSocket-Protocol",
                            HeaderValue::from_static("_aws_wt_session"),
                        );
                    }
                }
                Ok(resp)
            })
            .await
            .unwrap();
            assert!(saw_subprotocol, "Chime JoinToken subprotocol must be sent");

            // Expect JOIN -> reply JOIN_ACK.
            if let Some(Ok(WsMessage::Binary(b))) = ws.next().await {
                let frame = decode_wire_frame(&b).unwrap().unwrap();
                assert_eq!(frame.r#type, Some(sdk_signal_frame::Type::Join as i32));
                let ack = SdkSignalFrame {
                    timestamp_ms: Some(now_ms()),
                    r#type: Some(sdk_signal_frame::Type::JoinAck as i32),
                    joinack: Some(crate::proto::SdkJoinAckFrame {
                        video_subscription_limit: Some(25),
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                ws.send(WsMessage::Binary(encode_wire_frame(&ack).into()))
                    .await
                    .unwrap();
            }
            // Expect LEAVE -> reply LEAVE_ACK.
            if let Some(Ok(WsMessage::Binary(b))) = ws.next().await {
                let frame = decode_wire_frame(&b).unwrap().unwrap();
                assert_eq!(frame.r#type, Some(sdk_signal_frame::Type::Leave as i32));
                let ack = SdkSignalFrame {
                    timestamp_ms: Some(now_ms()),
                    r#type: Some(sdk_signal_frame::Type::LeaveAck as i32),
                    leave_ack: Some(crate::proto::SdkLeaveAckFrame {}),
                    ..Default::default()
                };
                ws.send(WsMessage::Binary(encode_wire_frame(&ack).into()))
                    .await
                    .unwrap();
            }
        });

        let creds = HuddleCredentials {
            channel: slackless_types::ChannelId::from("C1"),
            call_id: "R1".into(),
            meeting_id: "M1".into(),
            attendee_id: "A1".into(),
            external_user_id: "T1-R1-U1".into(),
            join_token: "join-tok-123".into(),
            signaling_url: format!("ws://{addr}/control/M1"),
            audio_host_url: "host:3478".into(),
            turn_control_url: String::new(),
            thread_root_ts: None,
            canvas_file_id: None,
            huddle_link: None,
        };
        let mut client = ChimeSignalClient::connect(&creds).await.unwrap();
        client.send_frame(&build_join_frame(77)).await.unwrap();
        let ack = client.next_frame().await.unwrap().unwrap();
        assert_eq!(ack.r#type, Some(sdk_signal_frame::Type::JoinAck as i32));
        assert_eq!(ack.joinack.unwrap().video_subscription_limit, Some(25));

        client.send_frame(&build_leave_frame()).await.unwrap();
        let leave_ack = client.next_frame().await.unwrap().unwrap();
        assert_eq!(
            leave_ack.r#type,
            Some(sdk_signal_frame::Type::LeaveAck as i32)
        );
        client.close().await;
        server.await.unwrap();
    }
}
