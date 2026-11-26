//! Real local WebSocket integration tests for [`ChimeSignalClient`]: verifies auth headers,
//! query params, `[0x05][SdkSignalFrame]` binary framing, `JOIN` -> `JOIN_ACK` handshake,
//! server `PING` -> client `PONG`, protocol error propagation, and clean `LEAVE` teardown.

#![allow(clippy::unwrap_used, clippy::result_large_err)]

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use slackless_huddle::proto::{
    SdkErrorFrame, SdkJoinAckFrame, SdkPingPongFrame, SdkPingPongType, SdkSignalFrame,
    sdk_signal_frame,
};
use slackless_huddle::signaling::{
    ChimeSignalClient, build_join_frame, decode_wire_frame, encode_wire_frame,
};
use slackless_huddle::{HuddleCredentials, HuddleError};
use slackless_types::ChannelId;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

fn creds_for(url: String) -> HuddleCredentials {
    HuddleCredentials {
        channel: ChannelId::from("C0HUDDLE"),
        call_id: "R0CALL".into(),
        meeting_id: "M0MEET".into(),
        attendee_id: "att-1".into(),
        external_user_id: "T0TEAM-R0CALL-U0SELF".into(),
        join_token: "join-token-abc".into(),
        signaling_url: url,
        audio_host_url: "127.0.0.1:3478".into(),
        turn_control_url: "https://example.invalid/turn".into(),
        thread_root_ts: None,
        canvas_file_id: None,
        huddle_link: None,
    }
}

#[tokio::test]
async fn chime_signal_client_full_ws_handshake_ping_pong_and_leave() {
    slackless_huddle::install_default_crypto_provider();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws =
            tokio_tungstenite::accept_hdr_async(stream, |req: &Request, mut resp: Response| {
                let uri = req.uri().to_string();
                assert!(
                    uri.contains("X-Chime-Control-Protocol-Version=3"),
                    "missing Chime version query param: {uri}"
                );
                assert_eq!(
                    req.headers()
                        .get("Sec-WebSocket-Protocol")
                        .and_then(|v| v.to_str().ok()),
                    Some("_aws_wt_session, join-token-abc")
                );
                assert_eq!(
                    req.headers().get("Origin").and_then(|v| v.to_str().ok()),
                    Some("https://app.slack.com")
                );
                resp.headers_mut()
                    .insert("Sec-WebSocket-Protocol", "_aws_wt_session".parse().unwrap());
                Ok(resp)
            })
            .await
            .unwrap();

        // 1. Expect client JOIN frame.
        let join_msg = ws.next().await.unwrap().unwrap();
        let WsMessage::Binary(bytes) = join_msg else {
            panic!("expected binary JOIN frame");
        };
        let join_frame = decode_wire_frame(&bytes).unwrap().unwrap();
        assert_eq!(join_frame.r#type, Some(sdk_signal_frame::Type::Join as i32));
        assert!(join_frame.join.is_some());

        // 2. Send an interleaved server PING before JOIN_ACK, then JOIN_ACK.
        let ping = SdkSignalFrame {
            r#type: Some(sdk_signal_frame::Type::PingPong as i32),
            ping_pong: Some(SdkPingPongFrame {
                r#type: Some(SdkPingPongType::Ping as i32),
                ping_id: Some(77),
            }),
            ..Default::default()
        };
        ws.send(WsMessage::Binary(encode_wire_frame(&ping).into()))
            .await
            .unwrap();

        let join_ack = SdkSignalFrame {
            r#type: Some(sdk_signal_frame::Type::JoinAck as i32),
            joinack: Some(SdkJoinAckFrame {
                wants_compressed_sdp: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        ws.send(WsMessage::Binary(encode_wire_frame(&join_ack).into()))
            .await
            .unwrap();

        // 3. Send another server PING and expect client PONG with matching ping_id.
        let ping2 = SdkSignalFrame {
            r#type: Some(sdk_signal_frame::Type::PingPong as i32),
            ping_pong: Some(SdkPingPongFrame {
                r#type: Some(SdkPingPongType::Ping as i32),
                ping_id: Some(88),
            }),
            ..Default::default()
        };
        ws.send(WsMessage::Binary(encode_wire_frame(&ping2).into()))
            .await
            .unwrap();

        let pong_msg = ws.next().await.unwrap().unwrap();
        let WsMessage::Binary(pong_bytes) = pong_msg else {
            panic!("expected binary PONG frame");
        };
        let pong = decode_wire_frame(&pong_bytes).unwrap().unwrap();
        assert_eq!(pong.r#type, Some(sdk_signal_frame::Type::PingPong as i32));
        let pp = pong.ping_pong.unwrap();
        assert_eq!(pp.r#type, Some(SdkPingPongType::Pong as i32));
        assert_eq!(pp.ping_id, Some(88));

        // 4. Expect client LEAVE frame on clean teardown.
        let leave_msg = ws.next().await.unwrap().unwrap();
        let WsMessage::Binary(leave_bytes) = leave_msg else {
            panic!("expected binary LEAVE frame");
        };
        let leave = decode_wire_frame(&leave_bytes).unwrap().unwrap();
        assert_eq!(leave.r#type, Some(sdk_signal_frame::Type::Leave as i32));
        let _ = ws.close(None).await;
    });

    let creds = creds_for(format!("ws://{addr}/control/M0MEET"));
    let mut client = ChimeSignalClient::connect(&creds).await.unwrap();
    client.send_frame(&build_join_frame(12345)).await.unwrap();

    let mut saw_interleaved_ping = false;
    let ack = client
        .wait_for_join_ack(Duration::from_secs(3), |frame| {
            if frame.ping_pong.is_some() {
                saw_interleaved_ping = true;
            }
        })
        .await
        .unwrap();
    assert!(saw_interleaved_ping);
    assert_eq!(ack.wants_compressed_sdp, Some(true));

    let next = client.next_frame().await.unwrap().unwrap();
    assert!(
        client
            .reply_ping_if_needed(next.ping_pong.as_ref().unwrap())
            .await
    );

    client
        .send_leave_and_close(Duration::from_millis(400))
        .await;
    server.await.unwrap();
}

#[tokio::test]
async fn chime_signal_client_propagates_protocol_error_during_join_ack() {
    slackless_huddle::install_default_crypto_provider();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws =
            tokio_tungstenite::accept_hdr_async(stream, |_req: &Request, mut resp: Response| {
                resp.headers_mut()
                    .insert("Sec-WebSocket-Protocol", "_aws_wt_session".parse().unwrap());
                Ok(resp)
            })
            .await
            .unwrap();
        let _ = ws.next().await; // consume JOIN
        let err = SdkSignalFrame {
            error: Some(SdkErrorFrame {
                status: Some(403),
                description: Some("join token expired".into()),
            }),
            ..Default::default()
        };
        ws.send(WsMessage::Binary(encode_wire_frame(&err).into()))
            .await
            .unwrap();
        let _ = ws.close(None).await;
    });

    let creds = creds_for(format!("ws://{addr}/control/M0MEET"));
    let mut client = ChimeSignalClient::connect(&creds).await.unwrap();
    client.send_frame(&build_join_frame(1)).await.unwrap();
    let res = client
        .wait_for_join_ack(Duration::from_secs(2), |_| {})
        .await;
    assert!(matches!(
        res,
        Err(HuddleError::Protocol { status: 403, ref description }) if description == "join token expired"
    ));
    server.await.unwrap();
}
