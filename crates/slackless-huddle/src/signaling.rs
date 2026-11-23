//! Amazon Chime SDK signaling WebSocket client (`[0x05][SdkSignalFrame]` protobuf framing).

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
    SdkPingPongFrame, SdkPingPongType, SdkRemoteVideoUpdateFrame, SdkSignalFrame,
    SdkStreamDescriptor, SdkStreamMediaType, SdkStreamMetricFrame, SdkStreamServiceType,
    SdkSubscribeFrame, SdkVideoQualityAdaptationPreference, SdkVideoSubscriptionConfiguration,
    sdk_metric, sdk_signal_frame,
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

/// Build the initial Chime `JOIN` frame (`protocol_version = 2`, `HAS_STREAM_UPDATE`).
pub fn build_join_frame(audio_session_id: u64) -> SdkSignalFrame {
    SdkSignalFrame {
        timestamp_ms: Some(now_ms()),
        r#type: Some(sdk_signal_frame::Type::Join as i32),
        join: Some(SdkJoinFrame {
            protocol_version: Some(2),
            max_num_of_videos: Some(25),
            flags: Some(SdkJoinFlags::HasStreamUpdate as u32),
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
            wants_compressed_sdp: Some(true),
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

/// Extract remote camera and `#content` screen-share sources from an `SdkIndexFrame`.
pub fn parse_index_sources(
    index: &SdkIndexFrame,
    self_attendee_id: &str,
) -> Vec<RemoteVideoSource> {
    let mut out = Vec::new();
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
        let Some(user_id) = parse_external_user_id(external) else {
            continue;
        };
        out.push(RemoteVideoSource {
            stream_id,
            group_id: src.group_id.unwrap_or(1),
            attendee_id,
            user_id,
            is_content,
            max_bitrate_kbps: src
                .max_bitrate_kbps
                .unwrap_or(if is_content { 1500 } else { 350 }),
        });
    }
    out
}

/// Build a Chime `SUBSCRIBE` frame for audio-only (`send_video = false`) or full `DUPLEX`
/// audio + H.264 video (`send_video = true` or non-empty `receive_sources`).
pub fn build_subscribe_frame(
    creds: &HuddleCredentials,
    sdp_offer: &str,
    wants_compressed_sdp: bool,
    audio_muted: bool,
    send_video: bool,
    receive_sources: &[RemoteVideoSource],
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
    let receive_stream_ids: Vec<u32> = receive_sources.iter().map(|s| s.stream_id).collect();
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

/// Convert [`RemoteVideoSource`]s into Chime `SdkVideoSubscriptionConfiguration` entries.
pub fn build_video_subscriptions(
    sources: &[RemoteVideoSource],
) -> Vec<SdkVideoSubscriptionConfiguration> {
    sources
        .iter()
        .enumerate()
        .map(|(ix, src)| SdkVideoSubscriptionConfiguration {
            mid: Some(format!("v{}", ix + 1)),
            attendee_id: Some(src.attendee_id.clone()),
            stream_id: Some(src.stream_id),
            priority: Some(if src.is_content { 1 } else { 2 }),
            target_bitrate_kbps: Some(src.max_bitrate_kbps.clamp(150, 2000)),
            group_id: Some(src.group_id),
            quality_adaptation_preference: Some(if src.is_content {
                SdkVideoQualityAdaptationPreference::MaintainResolution as i32
            } else {
                SdkVideoQualityAdaptationPreference::Balanced as i32
            }),
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
    if let Some(raw) = compressed.filter(|b| !b.is_empty()) {
        let mut dec = DeflateDecoder::new(raw);
        let mut out = String::new();
        dec.read_to_string(&mut out)
            .map_err(|e| HuddleError::Signaling(format!("sdp inflate failed: {e}")))?;
        return Ok(out);
    }
    if let Some(plain) = sdp_answer.filter(|s| !s.is_empty()) {
        return Ok(plain.to_owned());
    }
    Err(HuddleError::Signaling(
        "SUBSCRIBE_ACK contained neither sdp_answer nor compressed_sdp_answer".to_owned(),
    ))
}

/// Extract a Slack `UserId` from Chime's `ExternalUserId` (`<team>-<call_id>-<user_id>` or
/// `<team>-<call_id>-<user_id>#content`).
pub fn parse_external_user_id(external: &str) -> Option<UserId> {
    let base = external.strip_suffix("#content").unwrap_or(external);
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
    }

    #[test]
    fn signaling_url_appends_chime_protocol_params() {
        let built = build_signaling_url("wss://signal.z3.ec1.m.chime.aws/control/abc").unwrap();
        assert!(built.contains("X-Chime-Control-Protocol-Version=3"));
        assert!(built.contains("X-Amzn-Chime-Send-Close-On-Error=1"));
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
