//! Integration and unit tests for the RTM client, driver, TLS setup, and security boundaries.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use slackless_types::endpoints::{BROWSER_USER_AGENT, WEB_ORIGIN};
use slackless_types::{ChannelId, SlackEvent, UserId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::http::header::{COOKIE, ORIGIN, USER_AGENT};
use tokio_tungstenite::tungstenite::{self, Message};

use crate::backoff::Backoff;
use crate::client::RtmClient;
use crate::config::{ConnectionState, RtmConfig, RtmError, RtmEvent};
use crate::driver::{Driver, is_trusted_cookie_host};
use crate::frame::TickleReason;
use crate::tls::tls_connector;
use crate::url::ConnectParams;

async fn next_event(rx: &mut mpsc::Receiver<RtmEvent>) -> RtmEvent {
    tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("timed out waiting for RTM event")
        .expect("RTM event stream ended unexpectedly")
}

#[tokio::test]
async fn hello_replays_presence_sub_ignores_bad_frames_and_reconnects_on_goodbye() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        // Session 1: accept, send hello + garbage frame, expect presence_sub, send goodbye.
        let (stream1, _) = listener.accept().await.unwrap();
        let mut ws1 = tokio_tungstenite::accept_async(stream1).await.unwrap();
        ws1.send(Message::text(r#"{"type":"hello"}"#))
            .await
            .unwrap();
        ws1.send(Message::text("not valid json {{{")).await.unwrap();
        let sub1 = ws1.next().await.unwrap().unwrap().into_text().unwrap();
        assert!(sub1.contains("presence_sub"), "got: {sub1}");
        assert!(sub1.contains("U123"), "got: {sub1}");
        ws1.send(Message::text(r#"{"type":"goodbye"}"#))
            .await
            .unwrap();
        let _ = ws1.close(None).await;

        // Session 2: accept reconnect, send hello, verify presence_sub is automatically replayed.
        let (stream2, _) = listener.accept().await.unwrap();
        let mut ws2 = tokio_tungstenite::accept_async(stream2).await.unwrap();
        ws2.send(Message::text(r#"{"type":"hello","fast_reconnect":true}"#))
            .await
            .unwrap();
        let sub2 = ws2.next().await.unwrap().unwrap().into_text().unwrap();
        assert!(sub2.contains("presence_sub"), "got: {sub2}");
        assert!(sub2.contains("U123"), "got: {sub2}");
        // Keep open briefly until client closes.
        let _ = ws2.next().await;
    });

    let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-test"));
    cfg.base_url = format!("ws://{addr}/link/");
    cfg.backoff = Backoff {
        initial: Duration::from_millis(10),
        max: Duration::from_millis(50),
    };
    let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
    let drv = tokio::spawn(driver);

    // Queue presence_sub before hello arrives; driver must replay it after hello.
    client.presence_sub(vec![UserId::from("U123")]).unwrap();

    assert_eq!(
        next_event(&mut rx).await,
        RtmEvent::State(ConnectionState::Connecting)
    );
    assert_eq!(
        next_event(&mut rx).await,
        RtmEvent::State(ConnectionState::Connected {
            fast_reconnect: false
        })
    );
    // Hello event + Goodbye event + Reconnecting + Connecting + Connected (fast_reconnect: true)
    let mut saw_reconnect = false;
    let mut saw_second_connect = false;
    for _ in 0..8 {
        match next_event(&mut rx).await {
            RtmEvent::State(ConnectionState::Reconnecting { .. }) => saw_reconnect = true,
            RtmEvent::State(ConnectionState::Connected {
                fast_reconnect: true,
            }) => {
                saw_second_connect = true;
                break;
            }
            _ => {}
        }
    }
    assert!(saw_reconnect);
    assert!(saw_second_connect);

    client.close();
    let _ = tokio::time::timeout(Duration::from_secs(2), drv).await;
    server.await.unwrap();
}

#[tokio::test]
async fn unauthorized_handshake_stops_without_reconnecting() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = sock.read(&mut buf).await;
        let _ = sock
            .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
            .await;
        let _ = sock.shutdown().await;
    });

    let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-expired"));
    cfg.base_url = format!("ws://{addr}/link/");
    let (_client, mut rx, driver) = RtmClient::new(cfg).unwrap();
    let drv = tokio::spawn(driver);

    assert_eq!(
        next_event(&mut rx).await,
        RtmEvent::State(ConnectionState::Connecting)
    );
    match next_event(&mut rx).await {
        RtmEvent::State(ConnectionState::Disconnected { reason }) => {
            assert!(reason.contains("401"), "got: {reason}");
        }
        other => panic!("expected Disconnected on 401, got {other:?}"),
    }
    tokio::time::timeout(Duration::from_secs(2), drv)
        .await
        .expect("driver should exit promptly on 401")
        .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn ping_pong_liveness_resets_on_pong_and_reconnects_after_max_missed_pongs() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        // Session 1: answer 1st ping with pong (resets unanswered counter), ignore subsequent pings.
        let (stream1, _) = listener.accept().await.unwrap();
        let mut ws1 = tokio_tungstenite::accept_async(stream1).await.unwrap();
        ws1.send(Message::text(r#"{"type":"hello"}"#))
            .await
            .unwrap();

        let ping1: serde_json::Value =
            serde_json::from_str(&ws1.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
        assert_eq!(ping1["type"], "ping");
        let id1 = ping1["id"].as_u64().unwrap();
        ws1.send(Message::text(format!(
            r#"{{"type":"pong","reply_to":{id1}}}"#
        )))
        .await
        .unwrap();

        // Next 2 pings go unanswered (max_missed_pongs = 2); on the 3rd tick client drops.
        let ping2: serde_json::Value =
            serde_json::from_str(&ws1.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
        assert_eq!(ping2["type"], "ping");
        let ping3: serde_json::Value =
            serde_json::from_str(&ws1.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
        assert_eq!(ping3["type"], "ping");

        // Session 2 confirms the client reconnected after missing pongs.
        let (stream2, _) = listener.accept().await.unwrap();
        let mut ws2 = tokio_tungstenite::accept_async(stream2).await.unwrap();
        ws2.send(Message::text(r#"{"type":"hello"}"#))
            .await
            .unwrap();
        let _ = ws2.next().await;
    });

    let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-ping"));
    cfg.base_url = format!("ws://{addr}/link/");
    cfg.ping_interval = Duration::from_millis(25);
    cfg.max_missed_pongs = 2;
    cfg.backoff = Backoff {
        initial: Duration::from_millis(10),
        max: Duration::from_millis(20),
    };
    let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
    let drv = tokio::spawn(driver);

    let mut saw_pong = false;
    let mut saw_missed_reconnect = false;
    let mut connected_count = 0;
    for _ in 0..15 {
        match next_event(&mut rx).await {
            RtmEvent::Event(SlackEvent::Pong(_)) => saw_pong = true,
            RtmEvent::State(ConnectionState::Reconnecting { reason, .. }) => {
                assert!(reason.contains("2 pings unanswered"), "got: {reason}");
                saw_missed_reconnect = true;
            }
            RtmEvent::State(ConnectionState::Connected { .. }) => {
                connected_count += 1;
                if connected_count == 2 {
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(saw_pong);
    assert!(saw_missed_reconnect);
    assert_eq!(connected_count, 2);

    client.close();
    let _ = tokio::time::timeout(Duration::from_secs(2), drv).await;
    server.await.unwrap();
}

#[tokio::test]
async fn full_event_channel_keeps_pings_and_close_flowing() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (pinged_tx, pinged_rx) = oneshot::channel();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        ws.send(Message::text(r#"{"type":"hello"}"#)).await.unwrap();
        // Nobody reads the events, so `hello` cannot be handed over: pings must still come...
        for _ in 0..2 {
            let ping: serde_json::Value =
                serde_json::from_str(&ws.next().await.unwrap().unwrap().into_text().unwrap())
                    .unwrap();
            assert_eq!(ping["type"], "ping");
        }
        pinged_tx.send(()).unwrap();
        // ...and so must the close.
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(_))) => {}
                closed => break closed,
            }
        }
    });

    let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-stalled"));
    cfg.base_url = format!("ws://{addr}/link/");
    cfg.ping_interval = Duration::from_millis(20);
    // Liveness never ends this session: only a stalled driver can stop the pings.
    cfg.max_missed_pongs = 1_000;
    // `Connecting` alone fills the channel; the receiver reads nothing until the end.
    cfg.event_buffer = 1;
    let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
    let drv = tokio::spawn(driver);

    tokio::time::timeout(Duration::from_secs(3), pinged_rx)
        .await
        .expect("pings stopped while the event channel was full")
        .unwrap();
    client.close();
    let closed = tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .expect("close stalled behind the full event channel")
        .unwrap();
    assert!(
        matches!(closed, Some(Ok(Message::Close(_)))),
        "got: {closed:?}"
    );

    // Once the receiver reads again, every event arrives in order.
    assert_eq!(
        next_event(&mut rx).await,
        RtmEvent::State(ConnectionState::Connecting)
    );
    assert_eq!(
        next_event(&mut rx).await,
        RtmEvent::State(ConnectionState::Connected {
            fast_reconnect: false
        })
    );
    assert!(matches!(
        next_event(&mut rx).await,
        RtmEvent::Event(SlackEvent::Hello(_))
    ));
    assert!(matches!(
        next_event(&mut rx).await,
        RtmEvent::State(ConnectionState::Disconnected { .. })
    ));
    tokio::time::timeout(Duration::from_secs(2), drv)
        .await
        .expect("driver should exit once its events are read")
        .unwrap();
}

#[tokio::test]
async fn reconnect_url_is_single_use_and_falls_back_to_base_url() {
    let base_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_addr = base_listener.local_addr().unwrap();
    let redir_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let redir_addr = redir_listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        // Session 1 on base_url: send reconnect_url pointing to redir_listener, then goodbye.
        let (s1, _) = base_listener.accept().await.unwrap();
        let mut ws1 = tokio_tungstenite::accept_async(s1).await.unwrap();
        ws1.send(Message::text(r#"{"type":"hello"}"#))
            .await
            .unwrap();
        ws1.send(Message::text(format!(
            r#"{{"type":"reconnect_url","url":"ws://{redir_addr}/custom-path"}}"#
        )))
        .await
        .unwrap();
        ws1.send(Message::text(r#"{"type":"goodbye"}"#))
            .await
            .unwrap();
        let _ = ws1.close(None).await;

        // Session 2 MUST dial redir_listener with `/custom-path`.
        let (s2, _) = redir_listener.accept().await.unwrap();
        let mut seen_path = String::new();
        #[allow(clippy::result_large_err)]
        let mut ws2 = tokio_tungstenite::accept_hdr_async(
            s2,
            |req: &tungstenite::handshake::server::Request,
             resp: tungstenite::handshake::server::Response| {
                seen_path = req.uri().path().to_owned();
                Ok(resp)
            },
        )
        .await
        .unwrap();
        assert_eq!(seen_path, "/custom-path");
        ws2.send(Message::text(r#"{"type":"hello","fast_reconnect":true}"#))
            .await
            .unwrap();
        ws2.send(Message::text(r#"{"type":"goodbye"}"#))
            .await
            .unwrap();
        let _ = ws2.close(None).await;

        // Session 3 MUST fall back to base_listener (reconnect_url was single-use).
        let (s3, _) = base_listener.accept().await.unwrap();
        let mut ws3 = tokio_tungstenite::accept_async(s3).await.unwrap();
        ws3.send(Message::text(r#"{"type":"hello"}"#))
            .await
            .unwrap();
        let _ = ws3.next().await;
    });

    let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-redir"));
    cfg.base_url = format!("ws://{base_addr}/link/");
    cfg.backoff = Backoff {
        initial: Duration::from_millis(10),
        max: Duration::from_millis(20),
    };
    let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
    let drv = tokio::spawn(driver);

    let mut connects = Vec::new();
    for _ in 0..20 {
        if let RtmEvent::State(ConnectionState::Connected { fast_reconnect }) =
            next_event(&mut rx).await
        {
            connects.push(fast_reconnect);
            if connects.len() == 3 {
                break;
            }
        }
    }
    assert_eq!(connects, vec![false, true, false]);

    client.close();
    let _ = tokio::time::timeout(Duration::from_secs(2), drv).await;
    server.await.unwrap();
}

#[tokio::test]
async fn error_url_expired_clears_stored_reconnect_url() {
    let base_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_addr = base_listener.local_addr().unwrap();
    let unused_redir = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let redir_addr = unused_redir.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (s1, _) = base_listener.accept().await.unwrap();
        let mut ws1 = tokio_tungstenite::accept_async(s1).await.unwrap();
        ws1.send(Message::text(r#"{"type":"hello"}"#))
            .await
            .unwrap();
        ws1.send(Message::text(format!(
            r#"{{"type":"reconnect_url","url":"ws://{redir_addr}/expired"}}"#
        )))
        .await
        .unwrap();
        ws1.send(Message::text(
            r#"{"type":"error","error":{"code":1,"msg":"Socket URL has expired"}}"#,
        ))
        .await
        .unwrap();
        ws1.send(Message::text(r#"{"type":"goodbye"}"#))
            .await
            .unwrap();
        let _ = ws1.close(None).await;

        // Because code 1 cleared reconnect_url, session 2 must hit base_listener directly.
        let (s2, _) = base_listener.accept().await.unwrap();
        let mut ws2 = tokio_tungstenite::accept_async(s2).await.unwrap();
        ws2.send(Message::text(r#"{"type":"hello"}"#))
            .await
            .unwrap();
        let _ = ws2.next().await;
    });

    let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-exp"));
    cfg.base_url = format!("ws://{base_addr}/link/");
    cfg.backoff = Backoff {
        initial: Duration::from_millis(10),
        max: Duration::from_millis(20),
    };
    let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
    let drv = tokio::spawn(driver);

    let mut connected = 0;
    for _ in 0..15 {
        if let RtmEvent::State(ConnectionState::Connected { .. }) = next_event(&mut rx).await {
            connected += 1;
            if connected == 2 {
                break;
            }
        }
    }
    assert_eq!(connected, 2);

    client.close();
    let _ = tokio::time::timeout(Duration::from_secs(2), drv).await;
    server.await.unwrap();
}

#[tokio::test]
async fn in_band_401_and_http_403_stop_while_502_and_503_retry() {
    // 1. In-band error code 401 -> terminal Disconnected
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (s, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
            ws.send(Message::text(
                r#"{"type":"error","error":{"code":401,"msg":"bad token"}}"#,
            ))
            .await
            .unwrap();
        });
        let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-bad"));
        cfg.base_url = format!("ws://{addr}/link/");
        let (_client, mut rx, driver) = RtmClient::new(cfg).unwrap();
        let drv = tokio::spawn(driver);

        let mut stopped = None;
        for _ in 0..5 {
            if let RtmEvent::State(ConnectionState::Disconnected { reason }) =
                next_event(&mut rx).await
            {
                stopped = Some(reason);
                break;
            }
        }
        assert!(
            stopped
                .as_deref()
                .is_some_and(|r| r.contains("invalid auth")),
            "{stopped:?}"
        );
        tokio::time::timeout(Duration::from_secs(2), drv)
            .await
            .unwrap()
            .unwrap();
        server.await.unwrap();
    }

    // 2. HTTP 403 -> terminal Disconnected
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                .await;
            let _ = sock.shutdown().await;
        });
        let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-403"));
        cfg.base_url = format!("ws://{addr}/link/");
        let (_client, mut rx, driver) = RtmClient::new(cfg).unwrap();
        let drv = tokio::spawn(driver);

        assert_eq!(
            next_event(&mut rx).await,
            RtmEvent::State(ConnectionState::Connecting)
        );
        match next_event(&mut rx).await {
            RtmEvent::State(ConnectionState::Disconnected { reason }) => {
                assert!(reason.contains("403"), "got: {reason}");
            }
            other => panic!("expected Disconnected on 403, got {other:?}"),
        }
        tokio::time::timeout(Duration::from_secs(2), drv)
            .await
            .unwrap()
            .unwrap();
        server.await.unwrap();
    }

    // 3. HTTP 502 then 503 then WebSocket success -> retryable reconnects
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for status in ["502 Bad Gateway", "503 Service Unavailable"] {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock
                    .write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n").as_bytes())
                    .await;
                let _ = sock.shutdown().await;
            }
            let (s, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
            ws.send(Message::text(r#"{"type":"hello"}"#)).await.unwrap();
            let _ = ws.next().await;
        });

        let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-retry"));
        cfg.base_url = format!("ws://{addr}/link/");
        cfg.backoff = Backoff {
            initial: Duration::from_millis(10),
            max: Duration::from_millis(20),
        };
        let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
        let drv = tokio::spawn(driver);

        let mut attempts = Vec::new();
        let mut connected = false;
        for _ in 0..12 {
            match next_event(&mut rx).await {
                RtmEvent::State(ConnectionState::Reconnecting { attempt, .. }) => {
                    attempts.push(attempt);
                }
                RtmEvent::State(ConnectionState::Connected { .. }) => {
                    connected = true;
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(attempts, vec![1, 2]);
        assert!(connected);

        client.close();
        let _ = tokio::time::timeout(Duration::from_secs(2), drv).await;
        server.await.unwrap();
    }
}

#[tokio::test]
async fn handshake_sends_origin_cookie_and_user_agent_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (s, _) = listener.accept().await.unwrap();
        let mut seen_origin = None;
        let mut seen_cookie = None;
        let mut seen_ua = None;
        #[allow(clippy::result_large_err)]
        let mut ws = tokio_tungstenite::accept_hdr_async(
            s,
            |req: &tungstenite::handshake::server::Request,
             resp: tungstenite::handshake::server::Response| {
                seen_origin = req
                    .headers()
                    .get(ORIGIN)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                seen_cookie = req
                    .headers()
                    .get(COOKIE)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                seen_ua = req
                    .headers()
                    .get(USER_AGENT)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                Ok(resp)
            },
        )
        .await
        .unwrap();
        assert_eq!(seen_origin.as_deref(), Some(WEB_ORIGIN));
        assert_eq!(seen_cookie.as_deref(), Some("d=xoxd-cookie-val"));
        assert_eq!(seen_ua.as_deref(), Some(BROWSER_USER_AGENT));
        ws.send(Message::text(r#"{"type":"hello"}"#)).await.unwrap();
        let _ = ws.next().await;
    });

    let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-hdr"));
    cfg.base_url = format!("ws://{addr}/link/");
    cfg.d_cookie = Some("xoxd-cookie-val".to_owned());
    let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
    let drv = tokio::spawn(driver);

    for _ in 0..4 {
        if matches!(
            next_event(&mut rx).await,
            RtmEvent::State(ConnectionState::Connected { .. })
        ) {
            break;
        }
    }
    client.close();
    let _ = tokio::time::timeout(Duration::from_secs(2), drv).await;
    server.await.unwrap();
}

#[tokio::test]
async fn close_aborts_backoff_wait_promptly() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = sock.read(&mut buf).await;
        let _ = sock
            .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
            .await;
        let _ = sock.shutdown().await;
    });

    let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-wait"));
    cfg.base_url = format!("ws://{addr}/link/");
    cfg.backoff = Backoff {
        initial: Duration::from_secs(30),
        max: Duration::from_secs(30),
    };
    let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
    let drv = tokio::spawn(driver);

    assert_eq!(
        next_event(&mut rx).await,
        RtmEvent::State(ConnectionState::Connecting)
    );
    assert!(matches!(
        next_event(&mut rx).await,
        RtmEvent::State(ConnectionState::Reconnecting { .. })
    ));

    let started = Instant::now();
    client.close();
    assert!(matches!(
        next_event(&mut rx).await,
        RtmEvent::State(ConnectionState::Disconnected { .. })
    ));
    tokio::time::timeout(Duration::from_secs(1), drv)
        .await
        .expect("close should abort long backoff immediately")
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    server.await.unwrap();
}

#[tokio::test]
async fn presence_sub_during_backoff_replays_server_close_reconnects_healthy_resets_and_ids_monotonic()
 {
    use tokio_tungstenite::tungstenite::protocol::CloseFrame;
    use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        // Session 1: short-lived, receives typing (id=1) & tickle (id=2), closes with CloseFrame.
        let (s1, _) = listener.accept().await.unwrap();
        let mut ws1 = tokio_tungstenite::accept_async(s1).await.unwrap();
        ws1.send(Message::text(r#"{"type":"hello"}"#))
            .await
            .unwrap();
        let f1: serde_json::Value =
            serde_json::from_str(&ws1.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
        let f2: serde_json::Value =
            serde_json::from_str(&ws1.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
        assert_eq!(f1["id"], 1);
        assert_eq!(f2["id"], 2);
        ws1.send(Message::Close(Some(CloseFrame {
            code: CloseCode::Restart,
            reason: "restarting".into(),
        })))
        .await
        .unwrap();

        // Session 2: stays alive >= healthy_after (80ms), verifies presence_sub queued during
        // backoff replays with id=3, then closes with goodbye.
        let (s2, _) = listener.accept().await.unwrap();
        let mut ws2 = tokio_tungstenite::accept_async(s2).await.unwrap();
        ws2.send(Message::text(r#"{"type":"hello"}"#))
            .await
            .unwrap();
        let sub: serde_json::Value =
            serde_json::from_str(&ws2.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
        assert_eq!(sub["type"], "presence_sub");
        assert_eq!(sub["ids"], serde_json::json!(["U999"]));
        assert_eq!(sub["id"], 3);

        tokio::time::sleep(Duration::from_millis(90)).await;
        ws2.send(Message::text(r#"{"type":"goodbye"}"#))
            .await
            .unwrap();
        let _ = ws2.close(None).await;

        // Session 3: receives replayed presence_sub with id=4 (monotonic across reconnects).
        let (s3, _) = listener.accept().await.unwrap();
        let mut ws3 = tokio_tungstenite::accept_async(s3).await.unwrap();
        ws3.send(Message::text(r#"{"type":"hello"}"#))
            .await
            .unwrap();
        let sub2: serde_json::Value =
            serde_json::from_str(&ws3.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
        assert_eq!(sub2["type"], "presence_sub");
        assert_eq!(sub2["id"], 4);
        let _ = ws3.next().await;
    });

    let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-lifecycle"));
    cfg.base_url = format!("ws://{addr}/link/");
    cfg.healthy_after = Duration::from_millis(70);
    cfg.backoff = Backoff {
        initial: Duration::from_millis(40),
        max: Duration::from_millis(80),
    };
    let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
    let drv = tokio::spawn(driver);

    // Wait for Connected #1, send 2 frames (ids 1, 2).
    for _ in 0..4 {
        if matches!(
            next_event(&mut rx).await,
            RtmEvent::State(ConnectionState::Connected { .. })
        ) {
            break;
        }
    }
    client.user_typing(ChannelId::from("C1"), None).unwrap();
    client.tickle(TickleReason::Focus).unwrap();

    // Wait for Reconnecting #1 (from Message::Close), then queue presence_sub during backoff wait.
    let mut attempts = Vec::new();
    let mut saw_server_close = false;
    let mut connected = 1;
    for _ in 0..20 {
        match next_event(&mut rx).await {
            RtmEvent::State(ConnectionState::Reconnecting {
                attempt, reason, ..
            }) => {
                attempts.push(attempt);
                if attempts.len() == 1 {
                    saw_server_close = reason.contains("closed by server");
                    client.presence_sub(vec![UserId::from("U999")]).unwrap();
                }
            }
            RtmEvent::State(ConnectionState::Connected { .. }) => {
                connected += 1;
                if connected == 3 {
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(saw_server_close);
    // Attempt 1 after short session 1; after healthy session 2 (>= 70ms), attempt resets to 1!
    assert_eq!(attempts, vec![1, 1]);
    assert_eq!(connected, 3);

    client.close();
    let _ = tokio::time::timeout(Duration::from_secs(2), drv).await;
    server.await.unwrap();
}

#[test]
fn debug_redacts_token_and_d_cookie() {
    let params = ConnectParams::new("xoxc-super-secret-token");
    let params_dbg = format!("{params:?}");
    assert!(
        !params_dbg.contains("xoxc-super-secret-token"),
        "{params_dbg}"
    );
    assert!(params_dbg.contains("<redacted>"), "{params_dbg}");

    let mut cfg = RtmConfig::new(params);
    cfg.d_cookie = Some("xoxd-super-secret-cookie".to_owned());
    let cfg_dbg = format!("{cfg:?}");
    assert!(!cfg_dbg.contains("xoxc-super-secret-token"), "{cfg_dbg}");
    assert!(!cfg_dbg.contains("xoxd-super-secret-cookie"), "{cfg_dbg}");
    assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");
}

#[test]
fn is_trusted_cookie_host_allows_slack_and_base_host_and_rejects_foreign_hosts() {
    let prod = "wss://wss-primary.slack.com/";
    assert!(is_trusted_cookie_host(prod, "wss://slack.com/link"));
    assert!(is_trusted_cookie_host(
        prod,
        "wss://wss-secondary.slack.com/link"
    ));
    assert!(is_trusted_cookie_host(
        prod,
        "wss://a.b.slack.com/link?foo=1"
    ));
    assert!(is_trusted_cookie_host(
        prod,
        "wss://WSS-SECONDARY.SLACK.COM/link"
    ));

    // Reject plaintext downgrade, non-ws schemes, lookalike domains, userinfo tricks, and foreign hosts.
    assert!(!is_trusted_cookie_host(prod, "ws://slack.com/link"));
    assert!(!is_trusted_cookie_host(
        prod,
        "ws://wss-primary.slack.com/link"
    ));
    assert!(!is_trusted_cookie_host(prod, "https://slack.com/link"));
    assert!(!is_trusted_cookie_host(prod, "wss://evilslack.com/link"));
    assert!(!is_trusted_cookie_host(
        prod,
        "wss://slack.com.attacker.com/link"
    ));
    assert!(!is_trusted_cookie_host(prod, "wss://attacker.com/link"));
    assert!(!is_trusted_cookie_host(
        prod,
        "wss://slack.com@attacker.com/link"
    ));
    assert!(!is_trusted_cookie_host(prod, "not a valid url"));

    // Local mock WebSocket base hosts: allow same host (even across ephemeral ports), reject others.
    let local_ip = "ws://127.0.0.1:41234/link/";
    assert!(is_trusted_cookie_host(
        local_ip,
        "ws://127.0.0.1:54321/custom-path"
    ));
    assert!(is_trusted_cookie_host(
        local_ip,
        "wss://127.0.0.1:54321/custom-path"
    ));
    assert!(is_trusted_cookie_host(
        local_ip,
        "wss://wss-secondary.slack.com/link"
    ));
    assert!(!is_trusted_cookie_host(
        local_ip,
        "ws://localhost:54321/custom-path"
    ));
    assert!(!is_trusted_cookie_host(
        local_ip,
        "ws://127.0.0.2:54321/custom-path"
    ));
    assert!(!is_trusted_cookie_host(local_ip, "ws://attacker.com/steal"));

    let local_name = "ws://localhost:41234/link/";
    assert!(is_trusted_cookie_host(
        local_name,
        "ws://localhost:54321/custom-path"
    ));
    assert!(!is_trusted_cookie_host(
        local_name,
        "ws://127.0.0.1:54321/custom-path"
    ));
}

#[tokio::test]
async fn handshake_encodes_raw_d_cookie_and_preserves_already_encoded_cookie() {
    for (input_cookie, expected_header) in [
        (
            "xoxd-raw/val+with=chars ",
            "d=xoxd-raw%2Fval%2Bwith%3Dchars+",
        ),
        (
            "xoxd-enc%2Fval%2Bwith%3Dchars",
            "d=xoxd-enc%2Fval%2Bwith%3Dchars",
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (s, _) = listener.accept().await.unwrap();
            let mut seen_cookie = None;
            #[allow(clippy::result_large_err)]
            let mut ws = tokio_tungstenite::accept_hdr_async(
                s,
                |req: &tungstenite::handshake::server::Request,
                 resp: tungstenite::handshake::server::Response| {
                    seen_cookie = req
                        .headers()
                        .get(COOKIE)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    Ok(resp)
                },
            )
            .await
            .unwrap();
            assert_eq!(seen_cookie.as_deref(), Some(expected_header));
            ws.send(Message::text(r#"{"type":"hello"}"#)).await.unwrap();
            let _ = ws.next().await;
        });

        let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-cookie"));
        cfg.base_url = format!("ws://{addr}/link/");
        cfg.d_cookie = Some(input_cookie.to_owned());
        let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
        let drv = tokio::spawn(driver);

        for _ in 0..4 {
            if matches!(
                next_event(&mut rx).await,
                RtmEvent::State(ConnectionState::Connected { .. })
            ) {
                break;
            }
        }
        client.close();
        let _ = tokio::time::timeout(Duration::from_secs(2), drv).await;
        server.await.unwrap();
    }
}

#[tokio::test]
async fn untrusted_reconnect_url_is_rejected_and_never_receives_d_cookie() {
    // Defense-in-depth check on Driver::request directly: foreign host never receives Cookie.
    {
        let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-test"));
        cfg.d_cookie = Some("xoxd-secret/val=".to_owned());
        let (_cmd_tx, cmd_rx) = mpsc::channel(4);
        let (_close_tx, close_rx) = watch::channel(false);
        let (event_tx, _event_rx) = mpsc::channel(4);
        let driver = Driver {
            tls: tls_connector().unwrap(),
            config: cfg,
            commands: cmd_rx,
            close: close_rx,
            events: event_tx,
            pending: Default::default(),
            next_id: 1,
            reconnect_url: None,
            presence_sub: None,
        };
        let foreign_req = driver.request("wss://attacker.com/steal").unwrap();
        assert!(
            foreign_req.headers().get(COOKIE).is_none(),
            "d cookie must never be sent to an untrusted host"
        );
        let trusted_req = driver
            .request("wss://wss-secondary.slack.com/link")
            .unwrap();
        assert_eq!(
            trusted_req.headers().get(COOKIE).unwrap(),
            "d=xoxd-secret%2Fval%3D"
        );
    }

    // End-to-end mock WS check: base_url uses 127.0.0.1, untrusted reconnect_url points to a
    // different host (`localhost` on a separate listener). The client must reject it, never
    // dial the untrusted listener, and fall back to base_url with the d cookie.
    let base_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_addr = base_listener.local_addr().unwrap();
    let evil_listener = TcpListener::bind("localhost:0").await.unwrap();
    let evil_port = evil_listener.local_addr().unwrap().port();
    let evil_contacted = Arc::new(AtomicBool::new(false));
    let evil_flag = evil_contacted.clone();

    let evil_task = tokio::spawn(async move {
        if evil_listener.accept().await.is_ok() {
            evil_flag.store(true, Ordering::SeqCst);
        }
    });

    let server = tokio::spawn(async move {
        // Session 1 on base_listener: send untrusted reconnect_url, then goodbye.
        let (s1, _) = base_listener.accept().await.unwrap();
        let mut ws1 = tokio_tungstenite::accept_async(s1).await.unwrap();
        ws1.send(Message::text(r#"{"type":"hello"}"#))
            .await
            .unwrap();
        ws1.send(Message::text(format!(
            r#"{{"type":"reconnect_url","url":"ws://localhost:{evil_port}/steal"}}"#
        )))
        .await
        .unwrap();
        ws1.send(Message::text(r#"{"type":"goodbye"}"#))
            .await
            .unwrap();
        let _ = ws1.close(None).await;

        // Session 2 MUST fall back to base_listener (ignoring untrusted reconnect_url) and
        // send the encoded d cookie.
        let (s2, _) = base_listener.accept().await.unwrap();
        let mut seen_cookie = None;
        #[allow(clippy::result_large_err)]
        let mut ws2 = tokio_tungstenite::accept_hdr_async(
            s2,
            |req: &tungstenite::handshake::server::Request,
             resp: tungstenite::handshake::server::Response| {
                seen_cookie = req
                    .headers()
                    .get(COOKIE)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                Ok(resp)
            },
        )
        .await
        .unwrap();
        assert_eq!(seen_cookie.as_deref(), Some("d=xoxd-secret%2Fcookie%3D"));
        ws2.send(Message::text(r#"{"type":"hello"}"#))
            .await
            .unwrap();
        let _ = ws2.next().await;
    });

    let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-sec"));
    cfg.base_url = format!("ws://{base_addr}/link/");
    cfg.d_cookie = Some("xoxd-secret/cookie=".to_owned());
    cfg.backoff = Backoff {
        initial: Duration::from_millis(10),
        max: Duration::from_millis(20),
    };
    let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();
    let drv = tokio::spawn(driver);

    let mut connected = 0;
    for _ in 0..15 {
        if matches!(
            next_event(&mut rx).await,
            RtmEvent::State(ConnectionState::Connected { .. })
        ) {
            connected += 1;
            if connected == 2 {
                break;
            }
        }
    }
    assert_eq!(connected, 2);

    client.close();
    let _ = tokio::time::timeout(Duration::from_secs(2), drv).await;
    server.await.unwrap();
    evil_task.abort();
    assert!(
        !evil_contacted.load(Ordering::SeqCst),
        "untrusted reconnect_url host must never be dialed"
    );
}

#[tokio::test]
async fn bounded_command_channel_returns_queue_full_and_close_still_succeeds() {
    let mut cfg = RtmConfig::new(ConnectParams::new("xoxc-bounded"));
    cfg.command_buffer = 2;
    let (client, mut rx, driver) = RtmClient::new(cfg).unwrap();

    // Driver is not spawned yet, so queue fills at capacity 2 and returns QueueFull.
    assert!(client.tickle(TickleReason::Focus).is_ok());
    assert!(client.tickle(TickleReason::Focus).is_ok());
    assert!(matches!(
        client.tickle(TickleReason::Focus),
        Err(RtmError::QueueFull)
    ));

    // Closing must still take effect immediately even though the command queue is full.
    client.close();
    let drv = tokio::spawn(driver);
    assert!(matches!(
        next_event(&mut rx).await,
        RtmEvent::State(ConnectionState::Disconnected { .. })
    ));
    tokio::time::timeout(Duration::from_secs(2), drv)
        .await
        .expect("driver should exit promptly on close")
        .unwrap();
}
