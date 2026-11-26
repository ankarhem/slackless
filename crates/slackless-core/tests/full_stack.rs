//! Full-stack cross-layer integration tests exercising the REAL `Client`, REAL `SlackApi`
//! (`slack_api.rs`), REAL `RtmClient`, and REAL SQLite `Store` against local `wiremock` HTTP +
//! `tokio-tungstenite` WebSocket servers.
//!
//! Unlike unit tests using `FakeBackend`, these tests catch drift across HTTP wire formatting,
//! `SlackApi` trait glue, RTM WebSocket parsing, SQLite persistence, and durable outbox replay.

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use futures::StreamExt as _;
use futures_util::SinkExt as _;
use serde_json::json;
use slackless_core::api::MessageContent;
use slackless_core::auth::Session;
use slackless_core::rtm::ConnectionState;
use slackless_core::store::{OutboxStatus, Store};
use slackless_core::types::{ChannelId, TeamId, Ts, UserId};
use slackless_core::{Client, Event, HuddleCommand, MessagingCommand};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "xoxc-fullstack-123";
const COOKIE: &str = "xoxd-fullstack-cookie";

fn test_session() -> Session {
    Session {
        team_id: TeamId::from("T0FULL"),
        user_id: UserId::from("U0ME"),
        workspace_url: "https://fullstack.slack.com".into(),
        xoxc_token: TOKEN.into(),
        d_cookie: COOKIE.into(),
    }
}

async fn mount_hydration_mocks(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/client.userBoot"))
        .and(header("cookie", format!("d={COOKIE}")))
        .and(body_string_contains(format!("token={TOKEN}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "self": { "id": "U0ME", "name": "me", "real_name": "Me User" },
            "team": { "id": "T0FULL", "name": "FullStack Corp", "domain": "fullstack" },
            "channels": [
                {
                    "id": "C0ENG",
                    "name": "engineering",
                    "is_channel": true,
                    "is_member": true
                }
            ],
            "ims": []
        })))
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/client.counts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "channels": [
                { "id": "C0ENG", "last_read": "1700000000.000100", "latest": "1700000000.000100", "mention_count": 0, "has_unreads": false }
            ],
            "mpims": [],
            "ims": []
        })))
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/users.list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "members": [
                { "id": "U0ME", "name": "me", "real_name": "Me User", "profile": {} },
                { "id": "U0ALICE", "name": "alice", "real_name": "Alice Dev", "profile": {} }
            ],
            "response_metadata": { "next_cursor": "" }
        })))
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/users.channelSections.list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "channel_sections": []
        })))
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/emoji.list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "emoji": {}
        })))
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/client.dms"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "ims": [],
            "response_metadata": { "next_cursor": "" }
        })))
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/users.conversations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "channels": [
                { "id": "C0ENG", "name": "engineering", "is_channel": true, "is_member": true }
            ],
            "response_metadata": { "next_cursor": "" }
        })))
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/client.appCommands"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "commands": []
        })))
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/dnd.info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "dnd_enabled": false,
            "snooze_enabled": false
        })))
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/activity.feed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "items": []
        })))
        .mount(server)
        .await;
}

async fn wait_for_event(
    label: &str,
    events: &mut slackless_core::EventReceiver,
    mut pred: impl FnMut(&Event) -> bool,
) -> Event {
    let mut seen = Vec::new();
    match tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(ev) = events.next().await {
            if pred(&ev) {
                return ev;
            }
            seen.push(format!("{ev:?}"));
        }
        panic!("core event stream ended before {label}; seen={seen:?}");
    })
    .await
    {
        Ok(ev) => ev,
        Err(_) => panic!("timed out waiting for {label}; seen={seen:?}"),
    }
}

#[tokio::test]
async fn real_client_hydrates_receives_rtm_messages_and_executes_api_commands() {
    slackless_core::huddle::install_default_crypto_provider();
    let server = MockServer::start().await;
    mount_hydration_mocks(&server).await;

    Mock::given(method("POST"))
        .and(path("/api/chat.postMessage"))
        .and(header("cookie", format!("d={COOKIE}")))
        .and(body_string_contains("channel=C0ENG"))
        .and(body_string_contains("text=Hello+from+fullstack"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "channel": "C0ENG",
            "ts": "1700000100.000100",
            "message": {
                "type": "message",
                "user": "U0ME",
                "text": "Hello from fullstack",
                "ts": "1700000100.000100"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/reactions.add"))
        .and(body_string_contains("name=rocket"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
        .expect(1)
        .mount(&server)
        .await;

    // Local RTM WebSocket server that sends `hello`, then a live `message` + `message_changed`.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_addr = listener.local_addr().unwrap();
    let (push_tx, mut push_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let ws_server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        ws.send(WsMessage::text(r#"{"type":"hello"}"#))
            .await
            .unwrap();
        while let Some(frame) = push_rx.recv().await {
            if ws.send(WsMessage::text(frame)).await.is_err() {
                break;
            }
        }
        let _ = ws.close(None).await;
    });

    let store = Store::open_in_memory().unwrap();
    let mut handle = Client::start_with_endpoints(
        test_session(),
        store,
        format!("{}/api/", server.uri()),
        format!("ws://{ws_addr}/link/"),
    )
    .unwrap();

    // 1. Wait for both RTM Connected and Hydration (`SidebarChanged` + `Counts`) from real wiremock SlackApi.
    let mut hydrated = false;
    let mut connected = false;
    wait_for_event("hydrated+connected", handle.events(), |ev| {
        match ev {
            Event::SidebarChanged => hydrated = true,
            Event::Connection(ConnectionState::Connected { .. }) => connected = true,
            _ => {}
        }
        hydrated && connected
    })
    .await;

    // Verify SQLite Store actually holds the hydrated channel & users.
    {
        let s = handle.store().lock().unwrap();
        let conv = s
            .conversation(&TeamId::from("T0FULL"), &ChannelId::from("C0ENG"))
            .unwrap()
            .expect("C0ENG hydrated into SQLite");
        assert_eq!(conv.name.as_deref(), Some("engineering"));
        assert!(
            s.user(&TeamId::from("T0FULL"), &UserId::from("U0ALICE"))
                .unwrap()
                .is_some()
        );
    }

    // 2. Push a live RTM message + edit over the real WebSocket and verify Store + revision log.
    push_tx
        .send(
            json!({
                "type": "message",
                "channel": "C0ENG",
                "user": "U0ALICE",
                "text": "Deploy starting",
                "ts": "1700000050.000100"
            })
            .to_string(),
        )
        .unwrap();
    wait_for_event("hydrated+connected", handle.events(), |ev| {
        matches!(ev, Event::MessagePosted { channel, message }
            if channel.as_str() == "C0ENG" && message.ts.as_str() == "1700000050.000100")
    })
    .await;

    push_tx
        .send(
            json!({
                "type": "message",
                "subtype": "message_changed",
                "channel": "C0ENG",
                "ts": "1700000055.000100",
                "message": {
                    "type": "message",
                    "user": "U0ALICE",
                    "text": "Deploy finished cleanly",
                    "ts": "1700000050.000100",
                    "edited": { "user": "U0ALICE", "ts": "1700000055.000100" }
                }
            })
            .to_string(),
        )
        .unwrap();
    wait_for_event(
        "hydrated+connected",
        handle.events(),
        |ev| matches!(ev, Event::StoreChanged { channel } if channel.as_str() == "C0ENG"),
    )
    .await;

    {
        let s = handle.store().lock().unwrap();
        let row = s
            .message(
                &TeamId::from("T0FULL"),
                &ChannelId::from("C0ENG"),
                &Ts::from("1700000050.000100"),
            )
            .unwrap()
            .expect("edited message present in SQLite");
        assert_eq!(row.message.text, "Deploy finished cleanly");
        let revs = s
            .revisions(
                &TeamId::from("T0FULL"),
                &ChannelId::from("C0ENG"),
                &Ts::from("1700000050.000100"),
            )
            .unwrap();
        assert_eq!(
            revs.len(),
            2,
            "immutable audit log must keep both revisions"
        );
    }

    // 3. Send a message & reaction through the real SlackApi HTTP adapter.
    handle
        .send(MessagingCommand::SendMessage {
            channel: ChannelId::from("C0ENG"),
            content: MessageContent::plain("Hello from fullstack"),
            thread_ts: None,
            broadcast: false,
        })
        .unwrap();
    wait_for_event(
        "hydrated+connected",
        handle.events(),
        |ev| matches!(ev, Event::StoreChanged { channel } if channel.as_str() == "C0ENG"),
    )
    .await;

    handle
        .send(MessagingCommand::AddReaction {
            channel: ChannelId::from("C0ENG"),
            ts: Ts::from("1700000100.000100"),
            name: "rocket".into(),
        })
        .unwrap();
    // Wait briefly for the reaction request to settle before asserting wiremock expectations.
    tokio::time::sleep(Duration::from_millis(120)).await;

    drop(push_tx);
    handle.shutdown().unwrap();
    let _ = ws_server.await;
}

#[tokio::test]
async fn real_client_enqueues_durable_outbox_on_503_and_propagates_auth_expired() {
    slackless_core::huddle::install_default_crypto_provider();
    let server = MockServer::start().await;
    mount_hydration_mocks(&server).await;

    // First chat.postMessage fails transiently with HTTP 503 -> must enqueue into V13 outbox.
    Mock::given(method("POST"))
        .and(path("/api/chat.postMessage"))
        .respond_with(ResponseTemplate::new(503).set_body_string("service unavailable"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    // Second chat.postMessage (manual FlushOutbox) succeeds!
    Mock::given(method("POST"))
        .and(path("/api/chat.postMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "channel": "C0ENG",
            "ts": "1700000200.000100",
            "message": {
                "type": "message",
                "user": "U0ME",
                "text": "Queued then sent",
                "ts": "1700000200.000100"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    // rooms.join returns invalid_auth -> must propagate Event::AuthExpired through real SlackApi.
    Mock::given(method("POST"))
        .and(path("/api/rooms.join"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"ok": false, "error": "invalid_auth"})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_addr = listener.local_addr().unwrap();
    let ws_server = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await
        {
            let _ = ws.send(WsMessage::text(r#"{"type":"hello"}"#)).await;
            while let Some(Ok(_)) = ws.next().await {}
        }
    });

    let store = Store::open_in_memory().unwrap();
    let mut handle = Client::start_with_endpoints(
        test_session(),
        store,
        format!("{}/api/", server.uri()),
        format!("ws://{ws_addr}/link/"),
    )
    .unwrap();

    let mut hydrated = false;
    let mut connected = false;
    wait_for_event("SidebarChanged+Connected", handle.events(), |ev| {
        match ev {
            Event::SidebarChanged => hydrated = true,
            Event::Connection(ConnectionState::Connected { .. }) => connected = true,
            _ => {}
        }
        hydrated && connected
    })
    .await;

    // 1. Trigger transient 503 on SendMessage -> expect durable outbox row in Pending state.
    handle
        .send(MessagingCommand::SendMessage {
            channel: ChannelId::from("C0ENG"),
            content: MessageContent::plain("Queued then sent"),
            thread_ts: None,
            broadcast: false,
        })
        .unwrap();
    let queued = wait_for_event("OutboxUpdated(Pending)", handle.events(), |ev| {
        matches!(ev, Event::OutboxUpdated(items) if items.len() == 1 && items[0].status == OutboxStatus::Pending)
    })
    .await;
    assert!(
        matches!(queued, Event::OutboxUpdated(ref items) if items[0].payload_json.contains("Queued then sent"))
    );

    // 2. FlushOutbox retries against the now-healthy endpoint and clears the outbox.
    handle.send(MessagingCommand::FlushOutbox).unwrap();
    wait_for_event(
        "OutboxUpdated(empty)",
        handle.events(),
        |ev| matches!(ev, Event::OutboxUpdated(items) if items.is_empty()),
    )
    .await;

    // 3. Huddle join hitting `invalid_auth` propagates `Event::AuthExpired("invalid_auth")`.
    handle
        .send(HuddleCommand::Join {
            channel: ChannelId::from("C0ENG"),
            audio: Default::default(),
        })
        .unwrap();
    let auth_ev = wait_for_event("AuthExpired", handle.events(), |ev| {
        matches!(ev, Event::AuthExpired(_))
    })
    .await;
    assert!(matches!(auth_ev, Event::AuthExpired(ref code) if code == "invalid_auth"));

    handle.shutdown().unwrap();
    let _ = ws_server.await;
}
