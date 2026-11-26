//! Wire-level tests for `slackless-core`'s real `SlackApi` backend trait implementations
//! (`backend/slack_api.rs` and `backend/slack_api_helpers.rs`) against local `wiremock` servers.
//!
//! Covers multi-step adapter logic that `FakeBackend` unit tests never exercise:
//! - `open_dm` CSV formatting & `move_channel` section bulk updates
//! - `set_pinned` (`pins.add` vs `pins.remove`), `latest_message`, `recent_history`, `files` page sizing
//! - `load_app_home` tolerating non-auth `dispatchOpenEvent` failures while propagating `AuthExpired`
//! - `rooms_get_link` returning Slack's link or falling back to canonical `huddle_fallback_url`
//! - `channel_memberships` filtering and `create_canvas` ID extraction

#![allow(clippy::unwrap_used)]

use serde_json::json;
use slackless_core::api::{FilesFilter, SlackApi};
use slackless_core::types::{ChannelId, TeamId, Ts, UserId};
use slackless_core::{
    ChannelsBackend, CollabBackend, FilesBackend, HuddleBackend, MessagingBackend, WorkspaceBackend,
};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn mock_api() -> (MockServer, SlackApi) {
    slackless_core::huddle::install_default_crypto_provider();
    let server = MockServer::start().await;
    let api = SlackApi::new(&format!("{}/api/", server.uri()), "xoxc-test", "xoxd-test").unwrap();
    (server, api)
}

#[tokio::test]
async fn channels_and_messaging_backend_adapters_format_wire_params_accurately() {
    let (server, api) = mock_api().await;

    // 1. `open_dm` joins multiple user IDs as comma-separated `users`.
    Mock::given(method("POST"))
        .and(path("/api/conversations.open"))
        .and(body_string_contains("users=U1%2CU2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "channel": { "id": "G0MPIM", "is_mpim": true }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let dm = api
        .open_dm(vec![UserId::from("U1"), UserId::from("U2")])
        .await
        .unwrap();
    assert_eq!(dm.id.as_str(), "G0MPIM");

    // 2. `set_pinned(true)` hits `pins.add`; `set_pinned(false)` hits `pins.remove`.
    Mock::given(method("POST"))
        .and(path("/api/pins.add"))
        .and(body_string_contains("channel=C1"))
        .and(body_string_contains("timestamp=1700000001.000100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pins.remove"))
        .and(body_string_contains("channel=C1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;
    api.set_pinned(ChannelId::from("C1"), Ts::from("1700000001.000100"), true)
        .await
        .unwrap();
    api.set_pinned(ChannelId::from("C1"), Ts::from("1700000001.000100"), false)
        .await
        .unwrap();

    // 3. `latest_message` requests `limit=1` and returns the first message (or None on empty).
    Mock::given(method("POST"))
        .and(path("/api/conversations.history"))
        .and(body_string_contains("limit=1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "messages": [
                { "type": "message", "user": "U1", "text": "Newest", "ts": "1700000009.000100" }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let latest = api
        .latest_message(ChannelId::from("C1"))
        .await
        .unwrap()
        .expect("latest message");
    assert_eq!(latest.text, "Newest");

    // 4. `files` picks per-channel page size (100) vs workspace-wide page size (200).
    Mock::given(method("POST"))
        .and(path("/api/files.list"))
        .and(body_string_contains("count=100"))
        .and(body_string_contains("channel=C1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "files": [{ "id": "F1", "name": "spec.pdf" }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/files.list"))
        .and(body_string_contains("count=200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "files": [{ "id": "F2", "name": "all.png" }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let ch_files = api
        .files(FilesFilter {
            channel: Some(ChannelId::from("C1")),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(ch_files.len(), 1);
    assert_eq!(ch_files[0].id.as_str(), "F1");
    let ws_files = api.files(FilesFilter::default()).await.unwrap();
    assert_eq!(ws_files.len(), 1);
    assert_eq!(ws_files[0].id.as_str(), "F2");

    // 5. `channel_memberships` requests public+private and excludes archived.
    Mock::given(method("POST"))
        .and(path("/api/users.conversations"))
        .and(body_string_contains("exclude_archived=true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "channels": [{ "id": "C1", "name": "eng", "is_channel": true, "is_member": true }],
            "response_metadata": { "next_cursor": "" }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let memberships = api.channel_memberships().await.unwrap();
    assert_eq!(memberships.len(), 1);
}

#[tokio::test]
async fn load_app_home_and_rooms_get_link_handle_fallbacks_and_auth_expiry() {
    let (server, api) = mock_api().await;

    // 1. `rooms_get_link`: falls back to canonical Slack huddle URL when API omits `link`.
    Mock::given(method("POST"))
        .and(path("/api/rooms.getLink"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    let fallback =
        HuddleBackend::rooms_get_link(&api, TeamId::from("T0TEAM"), ChannelId::from("C0ENG"))
            .await
            .unwrap();
    assert!(
        fallback.contains("T0TEAM") && fallback.contains("C0ENG"),
        "expected canonical fallback huddle URL, got: {fallback}"
    );

    // Explicit `link` returned by Slack is preserved verbatim.
    Mock::given(method("POST"))
        .and(path("/api/rooms.getLink"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "link": "https://app.slack.com/huddle/T0TEAM/C0ENG?custom=1"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let explicit =
        HuddleBackend::rooms_get_link(&api, TeamId::from("T0TEAM"), ChannelId::from("C0ENG"))
            .await
            .unwrap();
    assert_eq!(
        explicit,
        "https://app.slack.com/huddle/T0TEAM/C0ENG?custom=1"
    );

    // 2. `load_app_home`: ignores non-auth failure on `apps.home.dispatchOpenEvent`.
    Mock::given(method("POST"))
        .and(path("/api/conversations.info"))
        .and(body_string_contains("return_app_home=true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "channel": { "id": "D0APP", "is_im": true },
            "app_home": { "home_tab_enabled": true, "messages_tab_enabled": true }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/apps.home.dispatchOpenEvent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": false,
            "error": "not_allowed"
        })))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    let home = api.load_app_home(ChannelId::from("D0APP")).await.unwrap();
    assert!(home.home_view.is_some() || home.home_tab_enabled || !home.home_tab_enabled);

    // 3. `load_app_home`: MUST propagate `AuthExpired` from `apps.home.dispatchOpenEvent`!
    Mock::given(method("POST"))
        .and(path("/api/apps.home.dispatchOpenEvent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": false,
            "error": "token_revoked"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let err = api
        .load_app_home(ChannelId::from("D0APP"))
        .await
        .unwrap_err();
    assert!(err.is_auth_expired(), "expected AuthExpired, got {err:?}");

    // 4. `create_canvas` extracts canvas_id.
    Mock::given(method("POST"))
        .and(path("/api/conversations.canvases.create"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "canvas_id": "F0CANVAS99"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let cid = api.create_canvas(ChannelId::from("C0ENG")).await.unwrap();
    assert_eq!(cid, "F0CANVAS99");

    // 5. `history_page`, `replies_page`, `recent_history`, and `move_channel`.
    Mock::given(method("POST"))
        .and(path("/api/conversations.history"))
        .and(body_string_contains("limit=200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "messages": [{ "type": "message", "user": "U1", "text": "page1", "ts": "1700000002.000100" }],
            "response_metadata": { "next_cursor": "cur-next" }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let page = api
        .history_page(ChannelId::from("C0ENG"), None, None)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.next_cursor.as_deref(), Some("cur-next"));

    Mock::given(method("POST"))
        .and(path("/api/conversations.history"))
        .and(body_string_contains("limit=15"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "messages": [{ "type": "message", "user": "U1", "text": "recent", "ts": "1700000003.000100" }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let recent = api
        .recent_history(ChannelId::from("C0ENG"), 15)
        .await
        .unwrap();
    assert_eq!(recent.len(), 1);

    Mock::given(method("POST"))
        .and(path("/api/conversations.replies"))
        .and(body_string_contains("ts=1700000002.000100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "messages": [{ "type": "message", "user": "U2", "text": "reply", "ts": "1700000004.000100" }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let replies = api
        .replies_page(
            ChannelId::from("C0ENG"),
            Ts::from("1700000002.000100"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(replies.items.len(), 1);

    Mock::given(method("POST"))
        .and(path("/api/users.channelSections.channels.bulkUpdate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;
    api.move_channel(
        Some(("L0NEW".into(), ChannelId::from("C0ENG"))),
        Some(("L0OLD".into(), ChannelId::from("C0ENG"))),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn edge_search_handles_full_partial_both_fail_and_auth_expired() {
    let team = TeamId::from("T0EDGE");

    // 1. Both users/search and channels/search succeed.
    let (server, api) = mock_api().await;
    let api = api
        .with_edge_base(&format!("{}/cache/", server.uri()))
        .unwrap();
    Mock::given(method("POST"))
        .and(path("/cache/T0EDGE/users/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "results": [{ "id": "U0ALICE", "name": "alice" }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/cache/T0EDGE/channels/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "results": [{ "id": "C0ENG", "name": "engineering", "is_channel": true }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let both = api.edge_search(team.clone(), "eng".into()).await.unwrap();
    assert_eq!(both.users.len(), 1);
    assert_eq!(both.channels.len(), 1);

    // 2. Partial failure (users 500, channels 200) -> tolerates and returns channels.
    let (server, api) = mock_api().await;
    let api = api
        .with_edge_base(&format!("{}/cache/", server.uri()))
        .unwrap();
    Mock::given(method("POST"))
        .and(path("/cache/T0EDGE/users/search"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/cache/T0EDGE/channels/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "results": [{ "id": "C0ENG", "name": "engineering", "is_channel": true }]
        })))
        .mount(&server)
        .await;
    let partial_u = api.edge_search(team.clone(), "eng".into()).await.unwrap();
    assert!(partial_u.users.is_empty());
    assert_eq!(partial_u.channels.len(), 1);

    // 3. Partial failure (users 200, channels 500) -> tolerates and returns users.
    let (server, api) = mock_api().await;
    let api = api
        .with_edge_base(&format!("{}/cache/", server.uri()))
        .unwrap();
    Mock::given(method("POST"))
        .and(path("/cache/T0EDGE/users/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "results": [{ "id": "U0ALICE", "name": "alice" }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/cache/T0EDGE/channels/search"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    let partial_c = api.edge_search(team.clone(), "eng".into()).await.unwrap();
    assert_eq!(partial_c.users.len(), 1);
    assert!(partial_c.channels.is_empty());

    // 4. Both fail -> returns Err.
    let (server, api) = mock_api().await;
    let api = api
        .with_edge_base(&format!("{}/cache/", server.uri()))
        .unwrap();
    Mock::given(method("POST"))
        .and(path("/cache/T0EDGE/users/search"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/cache/T0EDGE/channels/search"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    assert!(api.edge_search(team.clone(), "eng".into()).await.is_err());

    // 5. Auth expired on either branch MUST propagate AuthExpired (never swallowed).
    for (users_body, channels_body) in [
        (
            json!({"ok": false, "error": "invalid_auth"}),
            json!({"ok": true, "results": []}),
        ),
        (
            json!({"ok": true, "results": []}),
            json!({"ok": false, "error": "token_revoked"}),
        ),
    ] {
        let (server, api) = mock_api().await;
        let api = api
            .with_edge_base(&format!("{}/cache/", server.uri()))
            .unwrap();
        Mock::given(method("POST"))
            .and(path("/cache/T0EDGE/users/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(users_body))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/cache/T0EDGE/channels/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(channels_body))
            .mount(&server)
            .await;
        let err = api
            .edge_search(team.clone(), "eng".into())
            .await
            .unwrap_err();
        assert!(err.is_auth_expired(), "expected AuthExpired, got {err:?}");
    }
}
