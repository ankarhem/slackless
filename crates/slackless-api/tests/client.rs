//! End-to-end client tests against a local mock server. Never touches the real Slack network.

use futures::TryStreamExt;
use serde_json::json;
use slackless_api::types::{Block, ChannelId, MessageSubtype, SectionKind, Ts, UserId};
use slackless_api::{ApiError, HistoryOpts, MessageContent, SlackApi, paginate};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const TOKEN: &str = "xoxc-1-2-3-deadbeef";
const COOKIE: &str = "xoxd-abc/def+ghi=";
const COOKIE_HEADER: &str = "d=xoxd-abc%2Fdef%2Bghi%3D";
const HISTORY_FIXTURE: &str = include_str!("fixtures/conversations_history.json");

async fn setup() -> (MockServer, SlackApi) {
    let server = MockServer::start().await;
    let api = SlackApi::new(&format!("{}/api/", server.uri()), TOKEN, COOKIE).unwrap();
    (server, api)
}

fn form(req: &Request) -> Vec<(String, String)> {
    url::form_urlencoded::parse(&req.body)
        .into_owned()
        .collect()
}

fn field<'a>(form: &'a [(String, String)], key: &str) -> Option<&'a str> {
    form.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

#[tokio::test]
async fn sends_token_in_form_and_d_cookie_header() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/auth.test"))
        .and(header("cookie", COOKIE_HEADER))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(body_string_contains("token=xoxc-1-2-3-deadbeef"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true, "url": "https://acme.slack.com/", "team": "Acme",
            "user": "grace", "team_id": "T1", "user_id": "U1", "is_enterprise_install": false
        })))
        .expect(1)
        .mount(&server)
        .await;

    let me = api.auth_test().await.unwrap();
    assert_eq!(me.user_id.unwrap().as_str(), "U1");
    assert_eq!(me.team.as_deref(), Some("Acme"));
}

#[tokio::test]
async fn parses_recorded_history_into_typed_messages() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/conversations.history"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HISTORY_FIXTURE))
        .mount(&server)
        .await;

    let page = api
        .conversations_history(&ChannelId::from("C1"), &HistoryOpts::default(), None)
        .await
        .unwrap();

    assert_eq!(
        page.next_cursor.as_deref(),
        Some("bmV4dF90czoxNzAwMDAwMDAwMDAwMTAw")
    );
    let m = &page.items;
    assert_eq!(m.len(), 6, "no message may be dropped");

    // A subtype this build does not know is kept, with its unknown fields.
    assert_eq!(
        m[0].subtype,
        Some(MessageSubtype::Other("brand_new_subtype_2027".into()))
    );
    assert!(m[0].extra.contains_key("sparkles"));

    assert_eq!(m[1].user.as_ref().unwrap().as_str(), "U0AAAAAAA");
    assert!(matches!(&m[1].blocks[0], Block::RichText(_)));
    assert_eq!(m[1].reactions[0].name, "eyes");
    assert!(m[1].edited.is_some());

    assert_eq!(m[2].reply_count, Some(2));
    assert_eq!(m[2].thread_ts, Some(Ts::from("1700000300.000400")));
    assert!(!m[2].is_thread_reply());

    assert_eq!(m[3].subtype, Some(MessageSubtype::BotMessage));
    assert_eq!(
        m[3].attachments[0].title.as_deref(),
        Some("Deployed v1.2.3")
    );

    assert_eq!(m[4].subtype, Some(MessageSubtype::ChannelJoin));
    assert_eq!(m[5].files[0].id.as_str(), "F0EEEEEEE");
}

#[tokio::test]
async fn invalid_auth_maps_to_auth_expired() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"ok": false, "error": "invalid_auth"})),
        )
        .mount(&server)
        .await;

    let err = api.client_counts().await.unwrap_err();
    assert!(
        matches!(&err, ApiError::AuthExpired(e) if e == "invalid_auth"),
        "{err:?}"
    );
    assert!(err.is_auth_expired());
}

#[tokio::test]
async fn other_slack_errors_keep_the_error_string() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": false, "error": "channel_not_found"})),
        )
        .mount(&server)
        .await;

    let err = api
        .conversations_info(&ChannelId::from("C404"))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ApiError::Slack(e) if e == "channel_not_found"),
        "{err:?}"
    );
}

#[tokio::test]
async fn retries_after_429_then_succeeds() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/emoji.list"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "0")
                .set_body_json(json!({"ok": false, "error": "ratelimited"})),
        )
        .up_to_n_times(2)
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/emoji.list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true, "emoji": {"party": "https://emoji.example/party.png", "yay": "alias:party"}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let emoji = api.emoji_list().await.unwrap();
    assert_eq!(emoji["yay"], "alias:party");
}

#[tokio::test]
async fn gives_up_after_bounded_429_retries() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
        .expect(u64::from(slackless_api::MAX_RATE_LIMIT_RETRIES) + 1)
        .mount(&server)
        .await;

    let err = api.emoji_list().await.unwrap_err();
    assert!(matches!(err, ApiError::RateLimited { .. }), "{err:?}");
}

#[tokio::test]
async fn post_message_sends_rich_text_blocks_and_client_msg_id() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/chat.postMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true, "channel": "C1", "ts": "1772325870.309319",
            "message": {"type": "message", "text": "hello", "user": "U1", "ts": "1772325870.309319"}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let posted = api
        .chat_post_message(
            &ChannelId::from("C1"),
            &MessageContent::plain("hello"),
            Some(&Ts::from("1700000000.000100")),
            false,
        )
        .await
        .unwrap();
    assert_eq!(posted.ts.as_str(), "1772325870.309319");

    let requests = server.received_requests().await.unwrap();
    let sent = form(&requests[0]);
    assert_eq!(field(&sent, "token"), Some(TOKEN));
    assert_eq!(field(&sent, "channel"), Some("C1"));
    assert_eq!(field(&sent, "text"), Some("hello"));
    assert_eq!(field(&sent, "thread_ts"), Some("1700000000.000100"));
    assert_eq!(
        field(&sent, "client_msg_id"),
        posted.client_msg_id.as_deref()
    );
    assert_eq!(field(&sent, "client_msg_id").unwrap().len(), 36);
    let blocks: serde_json::Value = serde_json::from_str(field(&sent, "blocks").unwrap()).unwrap();
    assert_eq!(
        blocks,
        json!([{"type": "rich_text", "elements": [
            {"type": "rich_text_section", "elements": [{"type": "text", "text": "hello"}]}
        ]}])
    );
}

#[tokio::test]
async fn paginate_follows_cursors_until_exhausted() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/users.list"))
        .and(body_string_contains("cursor=page2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true, "members": [{"id": "U3"}],
            "response_metadata": {"next_cursor": ""}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/users.list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true, "members": [{"id": "U1"}, {"id": "U2"}, {"not_a_user": true}],
            "response_metadata": {"next_cursor": "page2"}
        })))
        .mount(&server)
        .await;

    let users = api.users_list_all().await.unwrap();
    let ids: Vec<_> = users.iter().map(|u| u.id.as_str()).collect();
    assert_eq!(ids, ["U1", "U2", "U3"]);

    // The generic helper works for any page-returning call.
    let history: Vec<_> = paginate(|c| api.users_list(Some(2), c))
        .try_collect()
        .await
        .unwrap();
    assert_eq!(history.len(), 3);
}

#[tokio::test]
async fn channel_sections_follow_the_top_level_cursor() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/users.channelSections.list"))
        .and(body_string_contains("cursor=page2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true, "count": 1, "cursor": "", "last_updated": 1777720183,
            "channel_sections": [{
                "channel_section_id": "L2", "name": "Priority", "type": "standard",
                "emoji": "rocket", "next_channel_section_id": "", "is_redacted": false,
                "channel_ids_page": {"channel_ids": ["C1", "D1"], "count": 2, "cursor": ""}
            }, {
                "channel_section_id": "L1", "type": "salesforce_records",
                "next_channel_section_id": "L2",
                "channel_ids_page": {"channel_ids": ["C10", "C9"]}
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/users.channelSections.list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true, "count": 2, "cursor": "page2",
            "channel_sections": [{
                "channel_section_id": "L1", "type": "salesforce_records",
                "next_channel_section_id": "L2",
                "channel_ids_page": {"channel_ids": ["C9"]}
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let sections = api.users_channel_sections_list().await.unwrap();
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].id, "L1");
    assert_eq!(sections[0].name, "", "missing fields default");
    assert_eq!(
        sections[0].kind,
        SectionKind::Other("salesforce_records".into())
    );
    assert_eq!(sections[0].next_id.as_deref(), Some("L2"));
    assert_eq!(
        sections[0].channel_ids,
        [ChannelId::from("C9"), ChannelId::from("C10")],
        "a section repeated on a later page merges its new channel ids"
    );
    assert_eq!(sections[1].id, "L2");
    assert_eq!(sections[1].kind, SectionKind::Standard);
    assert_eq!(sections[1].emoji, "rocket");
    assert_eq!(sections[1].next_id, None, "empty next id becomes None");
    assert_eq!(
        sections[1].channel_ids,
        [ChannelId::from("C1"), ChannelId::from("D1")]
    );

    let requests = server.received_requests().await.unwrap();
    let second = form(&requests[1]);
    assert_eq!(field(&second, "cursor"), Some("page2"));
}

#[tokio::test]
async fn channel_sections_stop_when_the_cursor_repeats() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/users.channelSections.list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true, "cursor": "same",
            "channel_sections": [{"channel_section_id": "L1", "type": "channels"}]
        })))
        .expect(2)
        .mount(&server)
        .await;

    let sections = api.users_channel_sections_list().await.unwrap();
    let ids: Vec<_> = sections.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        ["L1"],
        "two pages (the mock expects both), then the loop guard stops paging; the repeated \
         section is merged, not duplicated"
    );
}

#[tokio::test]
async fn download_file_sends_only_the_cookie() {
    let (server, api) = setup().await;
    Mock::given(method("GET"))
        .and(path("/files-pri/T1-F1/a.png"))
        .and(header("cookie", COOKIE_HEADER))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(b"\x89PNG".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/files-pri/T1-F2/b.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw("<html>sign in</html>", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let bytes = api
        .download_file(&format!("{}/files-pri/T1-F1/a.png", server.uri()))
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"\x89PNG");
    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].headers.get("authorization").is_none());

    let err = api
        .download_file(&format!("{}/files-pri/T1-F2/b.png", server.uri()))
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::FileAuthRejected), "{err:?}");

    let err = api
        .download_file("https://evil.example/x.png")
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::UntrustedUrl(_)), "{err:?}");
}

#[tokio::test]
async fn fetches_a_users_dnd_and_presence() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/dnd.info"))
        .and(body_string_contains("user=U2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "dnd_enabled": true,
            "next_dnd_start_ts": 1700000000,
            "next_dnd_end_ts": 1700003600,
            "snooze_enabled": false,
            "snooze_endtime": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/users.getPresence"))
        .and(body_string_contains("user=U2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "presence": "active",
            "online": true,
            "connection_count": 1,
            "last_activity": 1700000100
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dnd = api.dnd_info(&UserId::from("U2")).await.unwrap();
    assert!(dnd.paused(1700000000));
    assert!(!dnd.paused(1700003600));

    let presence = api.users_get_presence(&UserId::from("U2")).await.unwrap();
    assert_eq!(presence.presence, slackless_api::types::Presence::Active);
    assert_eq!(presence.online, Some(true));
}
