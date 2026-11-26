//! End-to-end client tests against a local mock server. Never touches the real Slack network.

#![allow(clippy::unwrap_used)]

use futures::TryStreamExt;
use serde_json::json;
use slackless_api::types::{Block, ChannelId, MessageSubtype, SectionKind, Ts, UserId};
use slackless_api::{
    ApiError, DownloadError, FilesFilter, HistoryOpts, MessageContent, SlackApi, paginate,
};
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
async fn post_message_sends_a_given_client_msg_id_instead_of_minting_one() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/chat.postMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true, "channel": "C1", "ts": "1772325870.309319"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let first_attempt_id = "6a1f3c2e-8b1d-4c3e-9f0a-1b2c3d4e5f60";
    let posted = api
        .chat_post_message_with_client_msg_id(
            &ChannelId::from("C1"),
            &MessageContent::plain("retried"),
            None,
            false,
            Some(first_attempt_id),
        )
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let sent = form(&requests[0]);
    assert_eq!(field(&sent, "client_msg_id"), Some(first_attempt_id));
    assert_eq!(posted.client_msg_id.as_deref(), Some(first_attempt_id));
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
        .expect(2)
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

    // Streamed to disk: the same checks, and a refused download leaves no file behind.
    let dir = std::env::temp_dir().join(format!(
        "slackless-api-file-download-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let written = api
        .download_file_to_path(
            &format!("{}/files-pri/T1-F1/a.png", server.uri()),
            &dir.join("a.png"),
        )
        .await
        .unwrap();
    assert_eq!(written, 4);
    assert_eq!(std::fs::read(dir.join("a.png")).unwrap(), b"\x89PNG");
    let err = api
        .download_file_to_path(
            &format!("{}/files-pri/T1-F2/b.png", server.uri()),
            &dir.join("b.png"),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, DownloadError::Api(ApiError::FileAuthRejected)),
        "{err:?}"
    );
    let err = api
        .download_file_to_path("https://evil.example/x.png", &dir.join("x.png"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, DownloadError::Api(ApiError::UntrustedUrl(_))),
        "{err:?}"
    );
    let names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["a.png"]);
    let _ = std::fs::remove_dir_all(&dir);

    let requests = server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|req| req.headers.get("authorization").is_none())
    );
}

#[tokio::test]
async fn download_html_file_accepts_canvas_html_and_rejects_signin_interstitial() {
    let (server, api) = setup().await;
    Mock::given(method("GET"))
        .and(path("/files-pri/T1-F0CANVAS/canvas"))
        .and(header("cookie", COOKIE_HEADER))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<!DOCTYPE html><html><body><h1 id='temp:C:1'>Roadmap</h1><p>Q4</p></body></html>",
            "text/html; charset=utf-8",
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/files-pri/T1-F0UNAUTH/canvas"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<!DOCTYPE html><html><head><title>Sign in | Slack</title></head><body><form id=\"signin_form\"></form></body></html>",
            "text/html; charset=utf-8",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let html = api
        .download_html_file(&format!("{}/files-pri/T1-F0CANVAS/canvas", server.uri()))
        .await
        .expect("canvas HTML download should succeed");
    assert!(html.contains("<h1 id='temp:C:1'>Roadmap</h1>"));

    let err = api
        .download_html_file(&format!("{}/files-pri/T1-F0UNAUTH/canvas", server.uri()))
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::FileAuthRejected), "{err:?}");
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

#[tokio::test]
async fn thread_view_parses_roots_and_sends_web_client_params() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/subscriptions.thread.getView"))
        .and(body_string_contains("limit=50"))
        .and(body_string_contains("fetch_threads_state=true"))
        .and(body_string_contains("priority_mode=all"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "total_unread_replies": 3,
            "new_threads_count": 1,
            "has_more": true,
            "max_ts": "1772325870.309319",
            "threads": [
                {
                    "root_msg": {
                        "channel": "C1",
                        "ts": "1700000300.000400",
                        "thread_ts": "1700000300.000400",
                        "user": "U1",
                        "text": "root",
                        "reply_count": 2,
                        "last_read": "1700000300.000500",
                        "subscribed": true
                    },
                    "latest_replies": [
                        {"ts": "1700000300.000600", "user": "U2", "text": "newest"}
                    ],
                    "unread_replies": [
                        {"ts": "1700000300.000500", "user": "U2", "text": "unread"}
                    ]
                },
                {
                    "root_msg": null,
                    "latest_replies": [{"ts": "1.0", "text": "orphan"}]
                }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let view = api.subscriptions_thread_get_view(50).await.unwrap();
    assert_eq!(view.total_unread_replies, 3);
    assert!(view.has_more);
    assert_eq!(view.threads.len(), 2, "a null root keeps its thread entry");
    let thread = &view.threads[0];
    assert_eq!(thread.root.channel.as_ref().unwrap().as_str(), "C1");
    assert_eq!(thread.root.reply_count, Some(2));
    assert_eq!(thread.root.subscribed, Some(true));
    assert_eq!(
        thread.root.last_read.as_ref().unwrap().as_str(),
        "1700000300.000500"
    );
    assert_eq!(thread.latest_replies[0].text, "newest");
    assert_eq!(thread.unread_replies.len(), 1);
    assert_eq!(view.threads[1].root.ts.as_str(), "", "a null root defaults");
}

#[tokio::test]
async fn subscriptions_thread_mark_posts_channel_thread_ts_and_read() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/subscriptions.thread.mark"))
        .and(body_string_contains("channel=C1"))
        .and(body_string_contains("thread_ts=1700000300.000400"))
        .and(body_string_contains("ts=1700000300.000600"))
        .and(body_string_contains("read=1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;

    api.subscriptions_thread_mark(
        &ChannelId::from("C1"),
        &Ts::from("1700000300.000400"),
        &Ts::from("1700000300.000600"),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn conversations_open_parses_the_channel() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/conversations.open"))
        .and(body_string_contains("users=U2"))
        .and(body_string_contains("return_im=true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "no_op": true,
            "already_open": true,
            "channel": {"id": "D1", "is_im": true, "user": "U2", "is_open": true}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dm = api.conversations_open("U2").await.unwrap();
    assert_eq!(dm.id.as_str(), "D1");
    assert_eq!(dm.user.as_ref().unwrap().as_str(), "U2");
    assert!(dm.is_im);
}

#[tokio::test]
async fn channel_section_create_sends_name_and_emoji_key() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/users.channelSections.create"))
        .and(body_string_contains("name=Priority"))
        .and(body_string_contains("emoji="))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "channel_section_id": "L0B12LBBCTD"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let created = api
        .users_channel_sections_create("Priority", "")
        .await
        .unwrap();
    assert_eq!(created.channel_section_id, "L0B12LBBCTD");
}

#[tokio::test]
async fn channel_section_set_sends_id_name_and_emoji_together() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/users.channelSections.set"))
        .and(body_string_contains("channel_section_id=L2"))
        .and(body_string_contains("name=Renamed"))
        .and(body_string_contains("emoji=rocket"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;

    api.users_channel_sections_set("L2", "Renamed", "rocket")
        .await
        .unwrap();
}

#[tokio::test]
async fn channel_section_delete_sends_the_section_id() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/users.channelSections.delete"))
        .and(body_string_contains("channel_section_id=L2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;

    api.users_channel_sections_delete("L2").await.unwrap();
}

#[tokio::test]
async fn channel_section_bulk_update_sends_json_sides_and_omits_empty_ones() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/users.channelSections.channels.bulkUpdate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(2)
        .mount(&server)
        .await;

    // A move between sections: insert (target) and remove (source) in one call.
    api.users_channel_sections_channels_bulk_update(
        Some(("L2", ChannelId::from("C1"))),
        Some(("L1", ChannelId::from("C1"))),
    )
    .await
    .unwrap();
    // A plain add: no remove side at all, as the observed client sends it.
    api.users_channel_sections_channels_bulk_update(Some(("L2", ChannelId::from("C9"))), None)
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    // The sides are JSON strings; compare them parsed (map key order is not part of the wire).
    let side = |form: &[(String, String)], key: &str| -> Option<serde_json::Value> {
        field(form, key).map(|value| serde_json::from_str(value).unwrap())
    };
    let move_call = form(&requests[0]);
    assert_eq!(
        side(&move_call, "insert").unwrap(),
        json!([{"channel_section_id": "L2", "channel_ids": ["C1"]}])
    );
    assert_eq!(
        side(&move_call, "remove").unwrap(),
        json!([{"channel_section_id": "L1", "channel_ids": ["C1"]}])
    );
    let add_call = form(&requests[1]);
    assert_eq!(
        side(&add_call, "insert").unwrap(),
        json!([{"channel_section_id": "L2", "channel_ids": ["C9"]}])
    );
    assert!(
        field(&add_call, "remove").is_none(),
        "an empty side is omitted, not sent as []"
    );
}

#[tokio::test]
async fn pins_list_posts_channel_and_parses_message_and_file_items() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/pins.list"))
        .and(body_string_contains("channel=C1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "items": [
                {"type": "message", "channel": "C1", "created": 1508881078,
                 "created_by": "U1",
                 "message": {"type": "message", "ts": "1508197641.000151", "user": "U1",
                             "text": "pinned!",
                             "permalink": "https://acme.slack.com/archives/C1/p1508197641000151"}},
                {"type": "file", "created": 1508881079, "created_by": "U1",
                 "file": {"id": "F1", "name": "spec.pdf", "mimetype": "application/pdf"}},
                {"type": "file", "created": 1508881080, "created_by": "U1"}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let pins = api.pins_list(&ChannelId::from("C1")).await.unwrap();

    assert_eq!(pins.items.len(), 3, "no pin may be dropped");
    let message = pins.items[0].message.as_ref().unwrap();
    assert_eq!(message.text, "pinned!");
    assert_eq!(message.ts, Ts::from("1508197641.000151"));
    assert_eq!(
        pins.items[0].created_by.as_ref().map(|u| u.as_str()),
        Some("U1")
    );
    let file = pins.items[1].file.as_ref().unwrap();
    assert_eq!(file.id.as_str(), "F1");
    assert!(
        pins.items[2].file.is_none(),
        "a file pin without its file stays lenient"
    );
}

#[tokio::test]
async fn pins_add_and_remove_post_channel_and_timestamp() {
    let (server, api) = setup().await;
    for endpoint in ["pins.add", "pins.remove"] {
        Mock::given(method("POST"))
            .and(path(format!("/api/{endpoint}")))
            .and(body_string_contains("channel=C1"))
            .and(body_string_contains("timestamp=1508197641.000151"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
    }

    let channel = ChannelId::from("C1");
    let ts = Ts::from("1508197641.000151");
    api.pins_add(&channel, &ts).await.unwrap();
    api.pins_remove(&channel, &ts).await.unwrap();
}

#[tokio::test]
async fn dnd_snooze_and_scheduled_message_methods_match_wire_format() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/dnd.setSnooze"))
        .and(body_string_contains("num_minutes=60"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "snooze_enabled": true,
            "snooze_endtime": 1760003600
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/dnd.endSnooze"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "snooze_enabled": false
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/chat.scheduleMessage"))
        .and(body_string_contains("channel=C1"))
        .and(body_string_contains("post_at=1760010000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "scheduled_message_id": "Q123",
            "channel": "C1",
            "post_at": 1760010000
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/chat.scheduledMessages.list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "scheduled_messages": [
                {"id": "Q123", "channel_id": "C1", "post_at": 1760010000, "date_created": 1760000000, "text": "hello later"}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/chat.deleteScheduledMessage"))
        .and(body_string_contains("channel=C1"))
        .and(body_string_contains("scheduled_message_id=Q123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;

    let snoozed = api.dnd_set_snooze(60).await.unwrap();
    assert!(snoozed.snooze_enabled);
    let resumed = api.dnd_end_snooze().await.unwrap();
    assert!(!resumed.snooze_enabled);

    let channel = ChannelId::from("C1");
    let created = api
        .chat_schedule_message(
            &channel,
            1760010000,
            &MessageContent::plain("hello later"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(created.scheduled_message_id, "Q123");
    let list = api.chat_scheduled_messages_list(None).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].text, "hello later");
    api.chat_delete_scheduled_message(&channel, "Q123")
        .await
        .unwrap();
}

#[tokio::test]
async fn scheduled_messages_fall_back_to_drafts_api_on_not_allowed_token_type() {
    let (server, api) = setup().await;
    // Public `chat.scheduleMessage` rejects `xoxc-` web-client tokens with `not_allowed_token_type`.
    Mock::given(method("POST"))
        .and(path("/api/chat.scheduleMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": false,
            "error": "not_allowed_token_type"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/drafts.create"))
        .and(body_string_contains("date_scheduled=1760010000"))
        .and(body_string_contains("is_from_composer=true"))
        .and(body_string_contains("file_ids=%5B%5D"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "draft": {
                "id": "Dr01SCHED",
                "date_scheduled": 1760010000,
                "last_updated_ts": "1760000123.5"
            }
        })))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/drafts.list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "drafts": [
                {
                    "id": "Dr01SCHED",
                    "last_updated_ts": "1760000123.5",
                    "date_scheduled": 1760010000,
                    "date_created": 1760000123,
                    "is_deleted": false,
                    "is_sent": false,
                    "destinations": [{"channel_id": "C1"}],
                    "blocks": [{"type": "rich_text", "elements": [{"type": "rich_text_section", "elements": [{"type": "text", "text": "hello at 11:00"}]}]}]
                },
                {
                    "id": "Dr02PLAIN",
                    "last_updated_ts": "1760000100.0",
                    "date_scheduled": 0,
                    "destinations": [{"channel_id": "C1"}],
                    "blocks": [{"type": "rich_text", "elements": [{"type": "rich_text_section", "elements": [{"type": "text", "text": "ordinary draft"}]}]}]
                }
            ]
        })))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/drafts.delete"))
        .and(body_string_contains("draft_id=Dr01SCHED"))
        .and(body_string_contains(
            "client_last_updated_ts=1760000123.5000000",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;

    let channel = ChannelId::from("C1");
    // 1st call hits `chat.scheduleMessage`, sees `not_allowed_token_type`, remembers fallback,
    // and transparently creates a scheduled draft via `drafts.create`.
    let created = api
        .chat_schedule_message(
            &channel,
            1_760_010_000,
            &MessageContent::plain("hello at 11:00"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(created.scheduled_message_id, "Dr01SCHED");
    assert_eq!(created.post_at, 1_760_010_000);

    // 2nd call skips `chat.scheduleMessage` completely (`expect(1)` above proves it!).
    let created2 = api
        .chat_schedule_message(
            &channel,
            1_760_010_000,
            &MessageContent::plain("second scheduled"),
            Some(&Ts::from("1760000000.000100")),
        )
        .await
        .unwrap();
    assert_eq!(created2.scheduled_message_id, "Dr01SCHED");

    // Listing scheduled messages goes straight to `drafts.list` and excludes plain drafts.
    let list = api.chat_scheduled_messages_list(None).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "Dr01SCHED");
    assert_eq!(list[0].text, "hello at 11:00");

    // Deleting a scheduled draft looks up and pads `client_last_updated_ts` and calls `drafts.delete`.
    api.chat_delete_scheduled_message(&channel, "Dr01SCHED")
        .await
        .unwrap();
}

#[tokio::test]
async fn channel_managers_reads_the_manager_role_on_the_channel() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/admin.roles.entity.listAssignments"))
        .and(body_string_contains("entity_id=C1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "role_assignments": [
                {"role_id": "Rl0A", "users": ["U1", "U2"]},
                {"role_id": "Rl0B", "users": ["U3"]}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let managers = api.channel_managers(&ChannelId::from("C1")).await.unwrap();
    assert_eq!(managers, [UserId::from("U1"), UserId::from("U2")]);
}

#[tokio::test]
async fn thread_subscription_adds_and_removes_with_the_read_cursor() {
    let (server, api) = setup().await;
    for endpoint in ["subscriptions.thread.add", "subscriptions.thread.remove"] {
        Mock::given(method("POST"))
            .and(path(format!("/api/{endpoint}")))
            .and(body_string_contains("channel=C1"))
            .and(body_string_contains("thread_ts=5.0"))
            .and(body_string_contains("last_read=7.0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
    }

    let (channel, thread, read) = (ChannelId::from("C1"), Ts::from("5.0"), Ts::from("7.0"));
    api.subscriptions_thread_set(&channel, &thread, &read, true)
        .await
        .unwrap();
    api.subscriptions_thread_set(&channel, &thread, &read, false)
        .await
        .unwrap();
}

#[tokio::test]
async fn bookmarks_list_and_add_use_their_distinct_param_names() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/bookmarks.list"))
        .and(body_string_contains("channel=C1"))
        .and(body_string_contains("include_folders=true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "bookmarks": [
                {"id": "Bk1", "channel_id": "C1", "title": "Runbook", "type": "link",
                 "link": "https://acme.test/runbook", "emoji": ":clap:", "rank": "g",
                 "parent_id": "", "date_created": 1644956055, "date_updated": 0},
                {"id": "Bk2", "channel_id": "C1", "title": "Docs", "type": "folder",
                 "link": null, "parent_id": ""},
                {"id": "Bk3", "channel_id": "C1", "title": "Nested", "type": "link",
                 "link": "https://acme.test/nested", "parent_id": "Bk2"}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/bookmarks.add"))
        .and(body_string_contains("channel_id=C1"))
        .and(body_string_contains("title=Roadmap"))
        .and(body_string_contains("type=link"))
        .and(body_string_contains(
            "link=https%3A%2F%2Facme.test%2Froadmap",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": true, "bookmark": {"id": "Bk9"}})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let list = api.bookmarks_list(&ChannelId::from("C1")).await.unwrap();
    assert_eq!(list.bookmarks.len(), 3);
    assert_eq!(list.bookmarks[0].title, "Runbook");
    assert_eq!(
        list.bookmarks[0].link.as_deref(),
        Some("https://acme.test/runbook")
    );
    assert_eq!(list.bookmarks[1].kind.as_deref(), Some("folder"));
    assert!(
        list.bookmarks[1].link.is_none(),
        "a folder bookmark has no link"
    );
    assert_eq!(list.bookmarks[2].parent_id.as_deref(), Some("Bk2"));

    api.bookmarks_add_link(
        &ChannelId::from("C1"),
        "Roadmap",
        "https://acme.test/roadmap",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn files_list_pages_by_count_and_page() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/files.list"))
        .and(body_string_contains("channel=C1"))
        .and(body_string_contains("count=100"))
        .and(body_string_contains("page=1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "files": [
                {"id": "F1", "name": "shot.png", "filetype": "png", "size": 2048,
                 "user": "U1", "created": 1700000001,
                 "permalink": "https://acme.slack.com/files-pri/T1-F1/shot.png"},
                {"id": "F2", "name": "spec.pdf", "filetype": "pdf", "size": 999999,
                 "created": 1700000000}
            ],
            "paging": {"count": 100, "total": 2, "page": 1, "pages": 1}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let page = api
        .files_list(&FilesFilter::channel(ChannelId::from("C1")), 100, 1)
        .await
        .unwrap();

    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].id.as_str(), "F1");
    assert_eq!(page.items[0].size, Some(2048));
    assert!(
        page.next_cursor.is_none(),
        "page-based paging carries no cursor"
    );
}

#[tokio::test]
async fn files_list_without_a_channel_sends_only_the_given_filters() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/files.list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "files": []})))
        .expect(1)
        .mount(&server)
        .await;

    let filter = FilesFilter {
        user: Some(UserId::from("U1")),
        types: Some("canvas".to_owned()),
        ..FilesFilter::workspace()
    };
    api.files_list(&filter, 200, 1).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    let sent = form(&requests[0]);
    let param = |key: &str| {
        sent.iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(
        param("channel"),
        None,
        "a workspace-wide list names no channel"
    );
    assert_eq!(param("user"), Some("U1"));
    assert_eq!(param("types"), Some("canvas"));
    assert_eq!(param("count"), Some("200"));
}

#[tokio::test]
async fn conversations_canvases_create_sends_channel_id_and_returns_canvas_id() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/conversations.canvases.create"))
        .and(body_string_contains("channel_id=C1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"ok": true, "canvas_id": "F07NEW"})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let created = api
        .conversations_canvases_create(&ChannelId::from("C1"))
        .await
        .unwrap();
    assert_eq!(created.canvas_id, "F07NEW");
}

#[tokio::test]
async fn usergroups_list_parses_groups() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/usergroups.list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "usergroups": [
                {
                    "id": "S0614TZR7",
                    "name": "Engineering",
                    "handle": "eng",
                    "description": "Eng team",
                    "user_count": 12,
                    "date_delete": 0
                }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let groups = api.usergroups_list().await.unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].id.as_str(), "S0614TZR7");
    assert_eq!(groups[0].handle, "eng");
    assert_eq!(groups[0].user_count, Some(12));
    assert!(!groups[0].is_deleted());
}

#[tokio::test]
async fn usergroups_disable_enable_and_users_parity() {
    use slackless_api::types::{UserId, UsergroupId};

    let (server, api) = setup().await;
    let ug_id = UsergroupId::from("S0614TZR7");

    Mock::given(method("POST"))
        .and(path("/api/usergroups.disable"))
        .and(body_string_contains("usergroup=S0614TZR7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "usergroup": {
                "id": "S0614TZR7",
                "name": "Engineering",
                "handle": "eng",
                "date_delete": 1700000000
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/usergroups.enable"))
        .and(body_string_contains("usergroup=S0614TZR7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "usergroup": {
                "id": "S0614TZR7",
                "name": "Engineering",
                "handle": "eng",
                "date_delete": 0
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/usergroups.users.list"))
        .and(body_string_contains("usergroup=S0614TZR7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "users": ["U01", "U02"]
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/usergroups.users.update"))
        .and(body_string_contains("usergroup=S0614TZR7"))
        .and(body_string_contains("users=U01%2CU03"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "usergroup": {
                "id": "S0614TZR7",
                "name": "Engineering",
                "handle": "eng",
                "user_count": 2,
                "date_delete": 0
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let disabled = api.usergroups_disable(&ug_id).await.expect("disable");
    assert_eq!(disabled.id, ug_id);
    assert!(disabled.is_deleted());

    let enabled = api.usergroups_enable(&ug_id).await.expect("enable");
    assert_eq!(enabled.id, ug_id);
    assert!(!enabled.is_deleted());

    let members = api.usergroups_users_list(&ug_id).await.expect("users list");
    assert_eq!(members, vec![UserId::from("U01"), UserId::from("U02")]);

    let updated = api
        .usergroups_users_update(&ug_id, &[UserId::from("U01"), UserId::from("U03")])
        .await
        .expect("users update");
    assert_eq!(updated.user_count, Some(2));
}

#[tokio::test]
async fn upload_files_runs_get_url_raw_post_and_complete_external() {
    let (server, api) = setup().await;
    let upload_url = format!("{}/upload/v1/slot1", server.uri());

    Mock::given(method("POST"))
        .and(path("/api/files.getUploadURLExternal"))
        .and(body_string_contains("filename=hello.txt"))
        .and(body_string_contains("length=5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "file_id": "F09UP1",
            "upload_url": upload_url,
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/upload/v1/slot1"))
        .respond_with(ResponseTemplate::new(200).set_body_string("OK - 5"))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/files.completeUploadExternal"))
        .and(body_string_contains("channel_id=C0ENG"))
        .and(body_string_contains("F09UP1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "files": [{ "id": "F09UP1", "name": "hello.txt", "size": 5 }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let files = vec![slackless_api::UploadFile {
        filename: "hello.txt".into(),
        bytes: bytes::Bytes::from_static(b"hello"),
    }];
    let uploaded = api
        .upload_files(
            &ChannelId::from("C0ENG"),
            None,
            Some(&MessageContent::plain("check this out")),
            &files,
        )
        .await
        .unwrap();
    assert_eq!(uploaded.len(), 1);
    assert_eq!(uploaded[0].id.as_str(), "F09UP1");
}

#[tokio::test]
async fn rooms_join_validates_chime_credentials_and_fallbacks() {
    let ch = ChannelId::from("C1");

    // 1. Missing `call` block
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/rooms.join"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&server)
            .await;
        let err = api.rooms_join(&ch, None, None).await.unwrap_err();
        assert!(
            matches!(err, ApiError::UnsupportedHuddleBackend(_)),
            "{err:?}"
        );
    }

    // 2. Non-free_willy media_backend_type
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/rooms.join"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "call": {
                    "call_id": "R1",
                    "huddle": { "id": "R1", "media_backend_type": "screenhero" }
                }
            })))
            .mount(&server)
            .await;
        let err = api.rooms_join(&ch, None, None).await.unwrap_err();
        assert!(
            matches!(&err, ApiError::UnsupportedHuddleBackend(msg) if msg == "screenhero"),
            "{err:?}"
        );
    }

    // 3. Missing free_willy credentials
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/rooms.join"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "call": {
                    "call_id": "R1",
                    "huddle": { "id": "R1", "media_backend_type": "free_willy" }
                }
            })))
            .mount(&server)
            .await;
        let err = api.rooms_join(&ch, None, None).await.unwrap_err();
        assert!(
            matches!(err, ApiError::UnsupportedHuddleBackend(_)),
            "{err:?}"
        );
    }

    // 4. Empty SignalingUrl or JoinToken
    for (sig, tok) in [("", "tok-1"), ("wss://signal.chime.aws/x", "")] {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/rooms.join"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "call": {
                    "call_id": "R1",
                    "free_willy": {
                        "attendee": { "JoinToken": tok },
                        "meeting": { "MediaPlacement": { "SignalingUrl": sig } }
                    },
                    "huddle": { "id": "R1", "media_backend_type": "free_willy" }
                }
            })))
            .mount(&server)
            .await;
        let err = api.rooms_join(&ch, None, None).await.unwrap_err();
        assert!(
            matches!(err, ApiError::UnsupportedHuddleBackend(_)),
            "{err:?}"
        );
    }

    // 5. Empty call_id falls back to call.huddle.id, passes thread_ts & regions
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/rooms.join"))
            .and(body_string_contains("thread_ts=1700000000.000100"))
            .and(body_string_contains("regions=eu-central-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "call": {
                    "call_id": "",
                    "free_willy": {
                        "attendee": { "AttendeeId": "a1", "JoinToken": "tok-ok" },
                        "meeting": {
                            "MeetingId": "m1",
                            "MediaPlacement": { "SignalingUrl": "wss://signal.chime.aws/ok" }
                        }
                    },
                    "huddle": {
                        "id": "R_FALLBACK",
                        "media_backend_type": "free_willy",
                        "participants": ["U1"]
                    }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let joined = api
            .rooms_join(
                &ch,
                Some(&Ts::from("1700000000.000100")),
                Some("eu-central-1"),
            )
            .await
            .unwrap();
        assert_eq!(joined.call_id, "R_FALLBACK");
        assert_eq!(joined.chime.attendee.join_token, "tok-ok");
    }
}

#[tokio::test]
async fn slack_lists_dual_endpoint_fallbacks_work() {
    use slackless_api::{ListCellInput, ListCellInputValue, SlackListCellValue};

    let (server, api) = setup().await;

    // fetch_list: files.info has no schema, lists.records.list fails -> falls back to
    // slackLists.items.list and backfills title/description/columns from `list`. The second
    // page follows `response_metadata.next_cursor` on the same fallback method.
    Mock::given(method("POST"))
        .and(path("/api/files.info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "file": { "id": "F_LIST" }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/lists.records.list"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": false, "error": "unknown_method"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/slackLists.items.list"))
        .and(body_string_contains("cursor=items2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "items": [
                {
                    "id": "It2",
                    "fields": [{ "column_id": "Col1", "key": "name", "text": "Second page item" }]
                }
            ],
            "response_metadata": { "next_cursor": "" }
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/slackLists.items.list"))
        .and(body_string_contains("include_list=true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "list": {
                "title": "Backfilled Title",
                "list_metadata": {
                    "description": "From items.list",
                    "schema": [
                        { "id": "Col1", "key": "name", "name": "Task", "type": "text", "is_primary_column": true }
                    ]
                }
            },
            "items": [
                {
                    "id": "It1",
                    "fields": [{ "column_id": "Col1", "key": "name", "text": "Fallback item" }]
                }
            ],
            "response_metadata": { "next_cursor": "items2" }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let data = api.fetch_list("F_LIST").await.unwrap();
    assert_eq!(data.title, "Backfilled Title");
    assert_eq!(data.description, "From items.list");
    assert_eq!(data.columns.len(), 1);
    let cells: Vec<_> = data.rows.iter().map(|row| row.cells.get("Col1")).collect();
    assert_eq!(
        cells,
        [
            Some(&SlackListCellValue::Text("Fallback item".into())),
            Some(&SlackListCellValue::Text("Second page item".into())),
        ]
    );

    // lists_records_create: private endpoint fails -> falls back to slackLists.items.create
    // and renames `parent_record_id` -> `parent_item_id`.
    Mock::given(method("POST"))
        .and(path("/api/lists.records.create"))
        .and(body_string_contains("parent_record_id=RecParent"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": false, "error": "unknown_method"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/slackLists.items.create"))
        .and(body_string_contains("parent_item_id=RecParent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "item": {
                "id": "ItNew",
                "fields": [{ "column_id": "Col1", "text": "Created via fallback" }]
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let created = api
        .lists_records_create(
            "F_LIST",
            &[ListCellInput {
                column_id: "Col1".into(),
                value: ListCellInputValue::Text("Created via fallback".into()),
            }],
            Some("RecParent"),
        )
        .await
        .unwrap();
    assert_eq!(created.id, "ItNew");
    assert_eq!(
        created.cells.get("Col1"),
        Some(&SlackListCellValue::Text("Created via fallback".into()))
    );

    // lists_cells_update fallback -> slackLists.items.update
    Mock::given(method("POST"))
        .and(path("/api/lists.cells.update"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": false, "error": "unknown_method"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/slackLists.items.update"))
        .and(body_string_contains("list_id=F_LIST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;

    api.lists_cells_update(
        "F_LIST",
        "ItNew",
        &[ListCellInput {
            column_id: "Col2".into(),
            value: ListCellInputValue::Checkbox(true),
        }],
    )
    .await
    .unwrap();

    // lists_records_delete fallback -> slackLists.items.delete
    Mock::given(method("POST"))
        .and(path("/api/lists.records.delete"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ok": false, "error": "unknown_method"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/slackLists.items.delete"))
        .and(body_string_contains("id=ItNew"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;

    api.lists_records_delete("F_LIST", "ItNew").await.unwrap();
}

#[tokio::test]
async fn http_error_truncation_decode_auth_expired_and_download_edge_cases() {
    // 1. Non-2xx HTML body -> ApiError::Status truncated to 512 chars
    {
        let (server, api) = setup().await;
        let long_html = format!("<html>{}</html>", "x".repeat(700));
        Mock::given(method("POST"))
            .and(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(502).set_body_raw(long_html, "text/html"))
            .mount(&server)
            .await;

        let err = api.auth_test().await.unwrap_err();
        match err {
            ApiError::Status { status, body } => {
                assert_eq!(status, 502);
                assert_eq!(body.chars().count(), 512);
                assert!(body.starts_with("<html>"));
            }
            other => panic!("expected ApiError::Status, got {other:?}"),
        }
    }

    // 2. 200 invalid JSON -> ApiError::Decode
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{not json"))
            .mount(&server)
            .await;
        let err = api.auth_test().await.unwrap_err();
        assert!(matches!(err, ApiError::Decode(_)), "{err:?}");
    }

    // 3. download_file non-2xx -> ApiError::Status, mixed-case Text/HTML -> FileAuthRejected
    {
        let (server, api) = setup().await;
        Mock::given(method("GET"))
            .and(path("/files/missing.png"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/files/signin.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("<html>login</html>", "Text/HTML; charset=UTF-8"),
            )
            .mount(&server)
            .await;

        let err = api
            .download_file(&format!("{}/files/missing.png", server.uri()))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ApiError::Status { status: 404, ref body } if body == "not found"),
            "{err:?}"
        );

        let err = api
            .download_file(&format!("{}/files/signin.png", server.uri()))
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::FileAuthRejected), "{err:?}");
    }

    // 4. All AUTH_EXPIRED_ERRORS map to AuthExpired, and missing `error` maps to "unknown_error"
    for code in [
        "invalid_auth",
        "not_authed",
        "token_revoked",
        "token_expired",
        "account_inactive",
    ] {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/auth.test"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"ok": false, "error": code})),
            )
            .mount(&server)
            .await;
        let err = api.auth_test().await.unwrap_err();
        assert!(
            matches!(&err, ApiError::AuthExpired(e) if e == code),
            "expected AuthExpired({code}), got {err:?}"
        );
    }

    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": false})))
            .mount(&server)
            .await;
        let err = api.auth_test().await.unwrap_err();
        assert!(
            matches!(&err, ApiError::Slack(e) if e == "unknown_error"),
            "{err:?}"
        );
    }
}

#[tokio::test]
async fn users_set_photo_multipart_framing_sanitization_mime_and_errors() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/users.setPhoto"))
        .and(header("cookie", COOKIE_HEADER))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(5)
        .mount(&server)
        .await;

    let cases = [
        ("bad\"\r\nname.jpg", "bad___name.jpg", "image/jpeg"),
        ("pic.JPEG", "pic.JPEG", "image/jpeg"),
        ("anim.gif", "anim.gif", "image/gif"),
        ("shot.webp", "shot.webp", "image/webp"),
        ("avatar.bin", "avatar.bin", "image/png"),
    ];
    for (filename, _, _) in &cases {
        api.users_set_photo(filename, b"\x89PNGDATA").await.unwrap();
    }

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), cases.len());
    let mut boundaries = std::collections::HashSet::new();
    for (req, (_, safe_name, expected_mime)) in requests.iter().zip(cases.iter()) {
        let ct = req.headers.get("content-type").unwrap().to_str().unwrap();
        let boundary = ct
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap_or_else(|| panic!("unexpected content-type: {ct}"));
        boundaries.insert(boundary.to_owned());
        let body = String::from_utf8_lossy(&req.body);
        assert!(body.contains("name=\"token\"\r\n\r\nxoxc-1-2-3-deadbeef\r\n"));
        assert!(
            body.contains(&format!("name=\"image\"; filename=\"{safe_name}\"")),
            "body did not sanitize filename {safe_name}: {body}"
        );
        assert!(
            body.contains(&format!("Content-Type: {expected_mime}\r\n\r\n")),
            "body missing MIME {expected_mime}: {body}"
        );
        assert!(
            req.body
                .windows(b"\x89PNGDATA".len())
                .any(|w| w == b"\x89PNGDATA"),
            "raw binary payload must be preserved verbatim"
        );
    }
    assert!(
        boundaries.len() > 1,
        "multipart uploads must use random per-request boundaries, got: {boundaries:?}"
    );

    // Error decoding on users.setPhoto (Slack error, non-2xx status, 200 bad JSON)
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/users.setPhoto"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"ok": false, "error": "bad_image"})),
            )
            .mount(&server)
            .await;
        let err = api.users_set_photo("a.png", b"x").await.unwrap_err();
        assert!(
            matches!(err, ApiError::Slack(ref e) if e == "bad_image"),
            "{err:?}"
        );
    }
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/users.setPhoto"))
            .respond_with(ResponseTemplate::new(500).set_body_string("server boom"))
            .mount(&server)
            .await;
        let err = api.users_set_photo("a.png", b"x").await.unwrap_err();
        assert!(
            matches!(err, ApiError::Status { status: 500, .. }),
            "{err:?}"
        );
    }
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/users.setPhoto"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
            .mount(&server)
            .await;
        let err = api.users_set_photo("a.png", b"x").await.unwrap_err();
        assert!(matches!(err, ApiError::Decode(_)), "{err:?}");
    }
}

#[tokio::test]
async fn messages_list_chunks_into_batches_of_eight_channels() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/messages.list"))
        .respond_with(|req: &Request| {
            let pairs = form(req);
            let raw = field(&pairs, "message_ids").unwrap();
            let list: Vec<serde_json::Value> = serde_json::from_str(raw).unwrap();
            let mut data = serde_json::Map::new();
            for entry in list {
                let ch = entry["channel"].as_str().unwrap().to_owned();
                let ts = entry["timestamps"][0].as_str().unwrap().to_owned();
                data.insert(
                    ch.clone(),
                    json!({ "messages": [{ "ts": ts, "text": format!("msg-{ch}") }] }),
                );
            }
            ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "messages_data": data }))
        })
        .expect(3)
        .mount(&server)
        .await;

    let ids: Vec<(ChannelId, Vec<Ts>)> = (0..17)
        .map(|i| {
            (
                ChannelId::from(format!("C{i}")),
                vec![Ts::from(format!("1.{i:06}"))],
            )
        })
        .collect();
    let result = api.messages_list(&ids).await.unwrap();
    assert_eq!(result.len(), 17);
    assert_eq!(result[&ChannelId::from("C16")][0].text, "msg-C16");

    let requests = server.received_requests().await.unwrap();
    let sizes: Vec<usize> = requests
        .iter()
        .map(|r| {
            let pairs = form(r);
            let arr: Vec<serde_json::Value> =
                serde_json::from_str(field(&pairs, "message_ids").unwrap()).unwrap();
            arr.len()
        })
        .collect();
    assert_eq!(sizes, vec![8, 8, 1]);
}

#[tokio::test]
async fn edge_channels_and_users_search_use_edge_base_and_mark_membership() {
    use slackless_api::types::TeamId;

    let server = MockServer::start().await;
    let api = SlackApi::new(&format!("{}/api/", server.uri()), TOKEN, COOKIE)
        .unwrap()
        .with_edge_base(&format!("{}/cache", server.uri()))
        .unwrap();

    Mock::given(method("POST"))
        .and(path("/cache/T1/channels/search"))
        .and(header("cookie", COOKIE_HEADER))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "member_channels": ["C_JOINED"],
            "results": [
                { "id": "C_JOINED", "name": "general", "is_member": false },
                { "id": "C_OTHER", "name": "random", "is_member": false }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/cache/T1/users/search"))
        .and(header("cookie", COOKIE_HEADER))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "results": [{ "id": "U1", "name": "alice" }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let channels = api
        .edge_channels_search(&TeamId::from("T1"), "gen", 10)
        .await
        .unwrap();
    assert_eq!(channels.len(), 2);
    assert!(
        channels[0].is_member,
        "member_channels marks is_member=true"
    );
    assert!(!channels[1].is_member);

    let users = api
        .edge_users_search(&TeamId::from("T1"), "ali", 10)
        .await
        .unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].id.as_str(), "U1");

    let reqs = server.received_requests().await.unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(payload["token"], TOKEN);
    assert_eq!(reqs[0].url.query(), Some("_x_app_name=client"));
}

#[tokio::test]
async fn blocks_suggestions_upload_errors_canvas_html_workflows_and_asset_client() {
    use slackless_api::types::BotId;
    use slackless_api::{ActionContainer, AssetClient, UploadFile};

    // 1. blocks_suggestions merges flat options and option_groups (object + string text)
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/blocks.suggestions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "options": [
                    { "text": { "type": "plain_text", "text": "Opt A" }, "value": "a" },
                    { "text": "Opt B", "value": "b" }
                ],
                "option_groups": [
                    {
                        "label": { "type": "plain_text", "text": "Group" },
                        "options": [
                            { "text": { "type": "plain_text", "text": "Opt C" }, "value": "c" }
                        ]
                    }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let container = ActionContainer {
            channel: ChannelId::from("C1"),
            message_ts: Ts::from("1.0"),
            attachment_id: None,
            is_ephemeral: false,
            view_id: None,
        };
        let opts = api
            .blocks_suggestions_for(&slackless_api::BlockSuggestionsRequest {
                bot: &BotId::from("B1"),
                app_id: "A1",
                team: &slackless_api::types::TeamId::from("T1"),
                action_id: "act",
                block_id: "blk",
                value: "query",
                container: &container,
            })
            .await
            .expect("blocks_suggestions_for");
        assert_eq!(
            opts,
            vec![
                ("Opt A".to_owned(), "a".to_owned()),
                ("Opt B".to_owned(), "b".to_owned()),
                ("Opt C".to_owned(), "c".to_owned()),
            ]
        );
    }

    // 2. upload_files empty-file & mid-flow error
    {
        let (server, api) = setup().await;
        let err = api
            .upload_files(
                &ChannelId::from("C1"),
                None,
                None,
                &[UploadFile {
                    filename: "empty.txt".into(),
                    bytes: bytes::Bytes::new(),
                }],
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, ApiError::Slack(ref e) if e == "empty_file"),
            "{err:?}"
        );
        assert!(server.received_requests().await.unwrap().is_empty());

        let upload_url = format!("{}/upload/fail", server.uri());
        Mock::given(method("POST"))
            .and(path("/api/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "file_id": "F1",
                "upload_url": upload_url
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/upload/fail"))
            .respond_with(ResponseTemplate::new(500).set_body_string("upload failed"))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/files.completeUploadExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(0)
            .mount(&server)
            .await;

        let err = api
            .upload_files(
                &ChannelId::from("C1"),
                None,
                None,
                &[UploadFile {
                    filename: "a.txt".into(),
                    bytes: bytes::Bytes::from_static(b"abc"),
                }],
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, ApiError::Status { status: 500, .. }),
            "{err:?}"
        );
    }

    // 3. fetch_canvas_html fallback (name + url_private_download, and missing URL error)
    {
        let (server, api) = setup().await;
        let dl_url = format!("{}/files-pri/T1-FC1/canvas.html", server.uri());
        Mock::given(method("POST"))
            .and(path("/api/files.info"))
            .and(body_string_contains("file=FC1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "file": {
                    "id": "FC1",
                    "title": "",
                    "name": "Fallback Name",
                    "url_private_download": dl_url
                }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/files-pri/T1-FC1/canvas.html"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("<main>canvas body</main>", "application/octet-stream"),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/files.info"))
            .and(body_string_contains("file=FC_NO_URL"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "file": { "id": "FC_NO_URL" }
            })))
            .mount(&server)
            .await;

        let (title, html) = api.fetch_canvas_html("FC1").await.unwrap();
        assert_eq!(title, "Fallback Name");
        assert_eq!(html, "<main>canvas body</main>");

        let err = api.fetch_canvas_html("FC_NO_URL").await.unwrap_err();
        assert!(
            matches!(err, ApiError::Slack(ref e) if e == "canvas_no_private_url"),
            "{err:?}"
        );
    }

    // 4. workflows_triggers_trip rejects non-HTTPS and non-Slack hosts, accepts Slack HTTPS
    {
        let (server, api) = setup().await;
        for untrusted in [
            "http://slack.com/shortcuts/Ft1",
            "https://evil.example/shortcuts/Ft1",
            "https://slack.com.evil.example/shortcuts/Ft1",
            "https://evilslack.com/shortcuts/Ft1",
        ] {
            let err = api
                .workflows_triggers_trip(untrusted, &json!({}))
                .await
                .unwrap_err();
            assert!(
                matches!(err, ApiError::UntrustedUrl(_)),
                "expected UntrustedUrl for {untrusted}, got {err:?}"
            );
        }

        Mock::given(method("POST"))
            .and(path("/api/workflows.triggers.trip"))
            .and(body_string_contains("Ft123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        api.workflows_triggers_trip("https://slack.com/shortcuts/Ft123", &json!({"a": 1}))
            .await
            .unwrap();
    }

    // 5. AssetClient::get
    {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/avatar.png"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"PNG!".to_vec()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/404.png"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;

        let client = AssetClient::new();
        let bytes = client
            .get(&format!("{}/avatar.png", server.uri()))
            .await
            .unwrap();
        assert_eq!(bytes, b"PNG!");
        let reqs = server.received_requests().await.unwrap();
        assert!(reqs[0].headers.get("cookie").is_none());
        assert!(
            reqs[0]
                .headers
                .get("user-agent")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|ua| ua.contains("Mozilla/5.0")),
            "AssetClient should send browser User-Agent"
        );

        let err = client
            .get(&format!("{}/404.png", server.uri()))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ApiError::Status { status: 404, .. }),
            "{err:?}"
        );
    }
}

#[tokio::test]
async fn canvas_access_delete_quip_threads_meta_and_comments_round_trips() {
    use slackless_api::CanvasAccessLevel;

    // 1. canvases_access_set (channels, users, and XOR validation)
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/canvases.access.set"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(2)
            .mount(&server)
            .await;

        // Invalid XOR cases fail locally before sending HTTP requests.
        let both_empty = api
            .canvases_access_set("F1", CanvasAccessLevel::Read, &[], &[])
            .await
            .unwrap_err();
        assert!(matches!(both_empty, ApiError::Slack(ref e) if e == "invalid_parameters"));

        let both_non_empty = api
            .canvases_access_set(
                "F1",
                CanvasAccessLevel::Write,
                &[ChannelId::from("C1")],
                &[UserId::from("U1")],
            )
            .await
            .unwrap_err();
        assert!(matches!(both_non_empty, ApiError::Slack(ref e) if e == "invalid_parameters"));

        api.canvases_access_set(
            "F1",
            CanvasAccessLevel::Write,
            &[ChannelId::from("C1"), ChannelId::from("C2")],
            &[],
        )
        .await
        .unwrap();
        api.canvases_access_set("F1", CanvasAccessLevel::Owner, &[], &[UserId::from("U9")])
            .await
            .unwrap();

        let reqs = server.received_requests().await.unwrap();
        assert_eq!(reqs.len(), 2);
        let f0 = form(&reqs[0]);
        assert_eq!(field(&f0, "canvas_id"), Some("F1"));
        assert_eq!(field(&f0, "access_level"), Some("write"));
        assert_eq!(field(&f0, "channel_ids"), Some(r#"["C1","C2"]"#));
        assert_eq!(field(&f0, "user_ids"), None);

        let f1 = form(&reqs[1]);
        assert_eq!(field(&f1, "canvas_id"), Some("F1"));
        assert_eq!(field(&f1, "access_level"), Some("owner"));
        assert_eq!(field(&f1, "user_ids"), Some(r#"["U9"]"#));
        assert_eq!(field(&f1, "channel_ids"), None);
    }

    // 2. canvases_access_delete & canvases_delete
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/canvases.access.delete"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/canvases.delete"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;

        let bad = api
            .canvases_access_delete("F1", &[], &[])
            .await
            .unwrap_err();
        assert!(matches!(bad, ApiError::Slack(ref e) if e == "invalid_parameters"));

        api.canvases_access_delete("F1", &[ChannelId::from("C1")], &[])
            .await
            .unwrap();
        api.canvases_access_delete("F1", &[], &[UserId::from("U2")])
            .await
            .unwrap();
        api.canvases_delete("F1").await.unwrap();

        let reqs = server.received_requests().await.unwrap();
        let f0 = form(&reqs[0]);
        assert_eq!(field(&f0, "canvas_id"), Some("F1"));
        assert_eq!(field(&f0, "channel_ids"), Some(r#"["C1"]"#));
        let f1 = form(&reqs[1]);
        assert_eq!(field(&f1, "user_ids"), Some(r#"["U2"]"#));
        let f2 = form(&reqs[2]);
        assert_eq!(field(&f2, "canvas_id"), Some("F1"));
    }

    // 3. quip_thread_archive / unarchive / archive_all
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/quip.thread.archive"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/quip.thread.unarchive"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/quip.thread.archiveAll"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;

        api.quip_thread_archive(&ChannelId::from("C0CANVAS"), &Ts::from("1700.1"))
            .await
            .unwrap();
        api.quip_thread_unarchive(&ChannelId::from("C0CANVAS"), &Ts::from("1700.1"))
            .await
            .unwrap();
        api.quip_thread_archive_all(&ChannelId::from("C0CANVAS"))
            .await
            .unwrap();

        let reqs = server.received_requests().await.unwrap();
        assert_eq!(field(&form(&reqs[0]), "channel_id"), Some("C0CANVAS"));
        assert_eq!(field(&form(&reqs[0]), "thread_ts"), Some("1700.1"));
        assert_eq!(field(&form(&reqs[1]), "channel_id"), Some("C0CANVAS"));
        assert_eq!(field(&form(&reqs[1]), "thread_ts"), Some("1700.1"));
        assert_eq!(field(&form(&reqs[2]), "channel_id"), Some("C0CANVAS"));
    }

    // 4. fetch_canvas_with_meta (both url_private and canvases.getContent fallback)
    {
        let (server, api) = setup().await;
        let export_url = format!("{}/files-pri/T1-FMETA1/canvas.html", server.uri());
        Mock::given(method("POST"))
            .and(path("/api/files.info"))
            .and(body_string_contains("file=FMETA1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "file": {
                    "id": "FMETA1",
                    "title": "Design Doc",
                    "permalink": "https://acme.slack.com/docs/T1/FMETA1",
                    "url_private": export_url,
                    "editable": true,
                    "access": "write",
                    "org_or_workspace_access": "read",
                    "canvas_printing_enabled": false,
                    "canvas_creator_id": "U1",
                    "editors": ["U1", "U2"],
                    "quip_thread_id": "QTHREAD1"
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/files-pri/T1-FMETA1/canvas.html"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("<h1>Design Doc</h1>", "application/octet-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let (meta, html) = api.fetch_canvas_with_meta("FMETA1").await.unwrap();
        assert_eq!(meta.file_id, "FMETA1");
        assert_eq!(meta.title, "Design Doc");
        assert!(meta.editable);
        assert_eq!(meta.access.as_deref(), Some("write"));
        assert!(!meta.canvas_printing_enabled);
        assert_eq!(meta.comment_channel_id, ChannelId::from("CMETA1"));
        assert_eq!(html, "<h1>Design Doc</h1>");

        // Fallback to canvases.getContent(content_type="html") when url_private is absent.
        Mock::given(method("POST"))
            .and(path("/api/files.info"))
            .and(body_string_contains("file=FMETA2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "file": {
                    "id": "FMETA2",
                    "title": "API Fallback Canvas",
                    "editable": false,
                    "access": "read"
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/canvases.getContent"))
            .and(body_string_contains("canvas_id=FMETA2"))
            .and(body_string_contains("content_type=html"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "content": "<p>Fetched via getContent</p>"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let (meta2, html2) = api.fetch_canvas_with_meta("FMETA2").await.unwrap();
        assert_eq!(meta2.title, "API Fallback Canvas");
        assert!(!meta2.editable);
        assert!(meta2.canvas_printing_enabled);
        assert_eq!(html2, "<p>Fetched via getContent</p>");
    }

    // 5. canvas_comments_list (every history and reply page, and channel_not_found -> empty) &
    //    canvas_comment_post
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/conversations.history"))
            .and(body_string_contains("cursor=hist2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": [
                    { "ts": "1700.3", "user": "U4", "text": "Typo in the title", "reply_count": 1 }
                ],
                "response_metadata": { "next_cursor": "" }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/conversations.history"))
            .and(body_string_contains("channel=C0CMT1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": [
                    {
                        "ts": "1700.1",
                        "user": "U1",
                        "text": "Please clarify section 2",
                        "reply_count": 1,
                        "document_comment": { "is_archived": false }
                    },
                    {
                        "ts": "1700.2",
                        "user": "U2",
                        "text": "Resolved note",
                        "is_archived": true
                    }
                ],
                "response_metadata": { "next_cursor": "hist2" }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/conversations.replies"))
            .and(body_string_contains("ts=1700.1"))
            .and(body_string_contains("cursor=rep2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": [
                    { "ts": "1700.12", "thread_ts": "1700.1", "user": "U1", "text": "Thanks" }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/conversations.replies"))
            .and(body_string_contains("channel=C0CMT1"))
            .and(body_string_contains("ts=1700.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": [
                    { "ts": "1700.1", "user": "U1", "text": "Please clarify section 2" },
                    { "ts": "1700.11", "thread_ts": "1700.1", "user": "U3", "text": "Updated!" }
                ],
                "response_metadata": { "next_cursor": "rep2" }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/conversations.replies"))
            .and(body_string_contains("ts=1700.3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": [
                    { "ts": "1700.3", "user": "U4", "text": "Typo in the title" },
                    { "ts": "1700.31", "thread_ts": "1700.3", "user": "U1", "text": "Fixed" }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/conversations.history"))
            .and(body_string_contains("channel=C0EMPTY"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "channel_not_found"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/chat.postMessage"))
            .and(body_string_contains("channel=C0CMT1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "channel": "C0CMT1",
                "ts": "1700.99"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let threads = api.canvas_comments_list("F0CMT1").await.unwrap();
        let roots: Vec<_> = threads.iter().map(|t| t.root_ts.as_str()).collect();
        assert_eq!(roots, ["1700.1", "1700.2", "1700.3"], "both history pages");
        let replies = |i: usize| -> Vec<&str> {
            threads[i].replies.iter().map(|r| r.text.as_str()).collect()
        };
        assert!(!threads[0].resolved);
        assert_eq!(replies(0), ["Updated!", "Thanks"], "both reply pages");
        assert!(threads[1].resolved);
        assert_eq!(replies(2), ["Fixed"]);

        let empty = api.canvas_comments_list("F0EMPTY").await.unwrap();
        assert!(empty.is_empty());

        let posted = api
            .canvas_comment_post("F0CMT1", "Looks good", Some(&Ts::from("1700.1")))
            .await
            .unwrap();
        assert_eq!(posted.ts, Ts::from("1700.99"));
    }
}

#[tokio::test]
async fn canvas_create_and_delete_fall_back_on_not_allowed_token_type() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/canvases.create"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": false,
            "error": "not_allowed_token_type"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/files.createCanvas"))
        .and(body_string_contains("title=Roadmap"))
        .and(body_string_contains("loosen_validation=true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "file_id": "F0PRIVCANVAS"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/canvases.delete"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": false,
            "error": "not_allowed_token_type"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/files.delete"))
        .and(body_string_contains("file=F0PRIVCANVAS"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let created = api
        .canvases_create(Some("Roadmap"), Some("# Plan"), None)
        .await
        .expect("files.createCanvas fallback should succeed");
    assert_eq!(created.canvas_id, "F0PRIVCANVAS");

    api.canvases_delete("F0PRIVCANVAS")
        .await
        .expect("files.delete fallback should succeed");
}

#[tokio::test]
async fn lists_do_not_swallow_auth_expired_and_search_modules_files_uses_files_filter() {
    use slackless_api::types::{BotId, TeamId};
    use slackless_api::{
        ActionContainer, BlockSuggestionsRequest, ListCellInput, ListCellInputValue, SearchSort,
    };

    // 1. lists endpoints propagate AuthExpired instead of falling back.
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/files.info"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "file": { "id": "F_LIST" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/lists.records.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "invalid_auth"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/slackLists.items.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(0)
            .mount(&server)
            .await;

        let err = api.fetch_list("F_LIST").await.unwrap_err();
        assert!(err.is_auth_expired(), "expected AuthExpired, got {err:?}");

        Mock::given(method("POST"))
            .and(path("/api/lists.cells.update"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "token_revoked"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/slackLists.items.update"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(0)
            .mount(&server)
            .await;

        let err = api
            .lists_cells_update(
                "F_LIST",
                "R1",
                &[ListCellInput {
                    column_id: "C1".into(),
                    value: ListCellInputValue::Checkbox(true),
                }],
            )
            .await
            .unwrap_err();
        assert!(err.is_auth_expired(), "expected AuthExpired, got {err:?}");
    }

    // 2. search_modules_files sends search_tab_filter=files (not messages).
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/search.modules.files"))
            .and(body_string_contains("search_tab_filter=files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "pagination": { "total_count": 0, "page": 1, "page_count": 0 },
                "items": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let page = api
            .search_modules_files("quarterly", SearchSort::Score, 1)
            .await
            .unwrap();
        assert_eq!(page.total, 0);
    }

    // 3. blocks_suggestions_for works with BlockSuggestionsRequest.
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/blocks.suggestions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "options": [{ "text": "Item 1", "value": "v1" }]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let container = ActionContainer {
            channel: ChannelId::from("C1"),
            message_ts: Ts::from("1.0"),
            attachment_id: None,
            is_ephemeral: false,
            view_id: None,
        };
        let bot = BotId::from("B1");
        let team = TeamId::from("T1");
        let opts = api
            .blocks_suggestions_for(&BlockSuggestionsRequest {
                bot: &bot,
                app_id: "A1",
                team: &team,
                action_id: "act",
                block_id: "blk",
                value: "q",
                container: &container,
            })
            .await
            .unwrap();
        assert_eq!(opts, vec![("Item 1".to_owned(), "v1".to_owned())]);
    }
}

#[tokio::test]
async fn modal_views_workflows_and_calls_info_endpoints() {
    use slackless_api::{ViewSubmitResponse, extract_workflow_trigger_id};

    // 1. views.submit response_action variants: push, update, clear, errors (ok:true and ok:false), close
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/views.submit"))
            .and(body_string_contains("view_id=V_PUSH"))
            .and(body_string_contains("_x_reason=submit-view"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "response_action": "push",
                "view": {
                    "id": "V_STEP2",
                    "root_view_id": "V_PUSH",
                    "previous_view_id": "V_PUSH",
                    "type": "modal",
                    "title": { "type": "plain_text", "text": "Step 2" }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let pushed = api
            .views_submit("V_PUSH", Some("h1"), &json!({}))
            .await
            .unwrap();
        match pushed {
            ViewSubmitResponse::Push(view) => {
                assert_eq!(view["id"], "V_STEP2");
                assert_eq!(view["previous_view_id"], "V_PUSH");
            }
            other => panic!("expected Push, got {other:?}"),
        }

        Mock::given(method("POST"))
            .and(path("/api/views.submit"))
            .and(body_string_contains("view_id=V_ERR_TRUE"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "response_action": "errors",
                "errors": { "title_block": "Title must be at least 5 characters" }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let errs = api
            .views_submit("V_ERR_TRUE", None, &json!({}))
            .await
            .unwrap();
        match errs {
            ViewSubmitResponse::Errors(map) => {
                assert_eq!(
                    map.get("title_block").map(String::as_str),
                    Some("Title must be at least 5 characters")
                );
            }
            other => panic!("expected Errors, got {other:?}"),
        }

        Mock::given(method("POST"))
            .and(path("/api/views.submit"))
            .and(body_string_contains("view_id=V_ERR_FALSE"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "validation_errors",
                "errors": { "email_block": "Invalid work email" }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let errs2 = api
            .views_submit("V_ERR_FALSE", None, &json!({}))
            .await
            .unwrap();
        match errs2 {
            ViewSubmitResponse::Errors(map) => {
                assert_eq!(
                    map.get("email_block").map(String::as_str),
                    Some("Invalid work email")
                );
            }
            other => panic!("expected Errors on ok:false validation_errors, got {other:?}"),
        }

        Mock::given(method("POST"))
            .and(path("/api/views.submit"))
            .and(body_string_contains("view_id=V_AUTH_ERR"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "invalid_auth"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let auth_err = api
            .views_submit("V_AUTH_ERR", None, &json!({}))
            .await
            .unwrap_err();
        assert!(
            auth_err.is_auth_expired(),
            "views.submit invalid_auth must classify as AuthExpired, got {auth_err:?}"
        );

        Mock::given(method("POST"))
            .and(path("/api/views.submit"))
            .and(body_string_contains("view_id=V_CLEAR"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "response_action": "clear",
                "view": null
            })))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            api.views_submit("V_CLEAR", None, &json!({})).await.unwrap(),
            ViewSubmitResponse::Clear
        );

        Mock::given(method("POST"))
            .and(path("/api/views.submit"))
            .and(body_string_contains("view_id=V_UPD"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "response_action": "update",
                "view": { "id": "V_UPD", "type": "modal", "hash": "h2" }
            })))
            .expect(1)
            .mount(&server)
            .await;
        assert!(matches!(
            api.views_submit("V_UPD", None, &json!({})).await.unwrap(),
            ViewSubmitResponse::Update(v) if v["hash"] == "h2"
        ));

        Mock::given(method("POST"))
            .and(path("/api/views.close"))
            .and(body_string_contains("view_id=V_STEP2"))
            .and(body_string_contains("root_view_id=V_PUSH"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;
        api.views_close("V_STEP2", Some("V_PUSH"), Some("h2"))
            .await
            .unwrap();
    }

    // 2. workflows.triggers.info + trip metadata + trigger_id extraction
    {
        assert_eq!(
            extract_workflow_trigger_id("https://slack.com/shortcuts/Ft0123ABC/deadbeef"),
            Some("Ft0123ABC".to_owned())
        );
        assert_eq!(
            extract_workflow_trigger_id("https://acme.slack.com/shortcuts/Ft999"),
            Some("Ft999".to_owned())
        );
        assert_eq!(
            extract_workflow_trigger_id("https://evil.example/shortcuts/Ft999"),
            None
        );

        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/workflows.triggers.info"))
            .and(body_string_contains("trigger_id=Ft0123ABC"))
            .and(body_string_contains("include_workflow_details=true"))
            .and(body_string_contains("_x_reason=hermes_triggers_ufm"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "trigger": {
                    "id": "Ft0123ABC",
                    "name": "Deploy to Production",
                    "description": "Collects release notes and triggers deploy",
                    "type": "shortcut",
                    "shortcut_url": "https://slack.com/shortcuts/Ft0123ABC/hash",
                    "workflow": {
                        "id": "Wf001",
                        "app_id": "A0WF",
                        "is_published": true,
                        "collaborators": ["U1", "U2"],
                        "steps": [
                            { "title": "Collect form responses", "description": "Ask for release tag" },
                            { "function": { "id": "FnDeploy", "title": "Trigger CI deploy" } }
                        ]
                    }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let info = api.workflows_triggers_info("Ft0123ABC").await.unwrap();
        assert_eq!(info.trigger_id, "Ft0123ABC");
        assert_eq!(info.name, "Deploy to Production");
        assert_eq!(info.workflow_id.as_deref(), Some("Wf001"));
        assert_eq!(info.steps.len(), 2);
        assert_eq!(info.steps[1].title, "Trigger CI deploy");
        assert_eq!(info.collaborators, vec!["U1", "U2"]);
        assert_eq!(info.is_published, Some(true));

        Mock::given(method("POST"))
            .and(path("/api/workflows.triggers.trip"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "is_slow_workflow": true,
                "contains_ai_steps": true
            })))
            .expect(1)
            .mount(&server)
            .await;
        let trip = api
            .workflows_triggers_trip("https://slack.com/shortcuts/Ft0123ABC/hash", &json!({}))
            .await
            .unwrap();
        assert!(trip.is_slow_workflow);
        assert!(trip.contains_ai_steps);
    }

    // 3. calls.info hydration
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/calls.info"))
            .and(body_string_contains("id=R0CALL123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "call": {
                    "id": "R0CALL123",
                    "title": "Incident Bridge",
                    "date_start": 1700000000,
                    "join_url": "https://acme.zoom.us/j/987654321",
                    "desktop_app_join_url": "zoommtg://zoom.us/join?confno=987654321",
                    "users": [
                        { "slack_id": "U1" },
                        { "external_id": "ext-2", "display_name": "Guest Bob" }
                    ],
                    "channels": ["C1"]
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let call = api.calls_info("R0CALL123").await.unwrap();
        assert_eq!(call.id, "R0CALL123");
        assert_eq!(call.title.as_deref(), Some("Incident Bridge"));
        assert_eq!(call.users.len(), 2);
        assert_eq!(call.users[1].display_name.as_deref(), Some("Guest Bob"));
    }
}

#[tokio::test]
async fn slack_lists_access_meta_threads_and_team_connections() {
    use slackless_api::types::UserId;
    use slackless_api::{ListAccessLevel, format_list_error, list_comment_channel_id};

    // 1. slackLists.access.set & slackLists.access.delete (XOR validation + JSON array encoding + partial failures)
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/slackLists.access.set"))
            .and(body_string_contains("list_id=F0LIST1"))
            .and(body_string_contains("access_level=write"))
            .and(body_string_contains("channel_ids=%5B%22C01%22%5D"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/slackLists.access.set"))
            .and(body_string_contains("list_id=F0LIST1"))
            .and(body_string_contains("access_level=owner"))
            .and(body_string_contains("user_ids=%5B%22U01%22%5D"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/slackLists.access.delete"))
            .and(body_string_contains("list_id=F0LIST1"))
            .and(body_string_contains("user_ids=%5B%22U01%22%2C%22U02%22%5D"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "failed_to_update_user_ids": ["U02"]
            })))
            .expect(1)
            .mount(&server)
            .await;

        api.lists_access_set(
            "F0LIST1",
            ListAccessLevel::Write,
            &[ChannelId::from("C01")],
            &[],
        )
        .await
        .unwrap();
        api.lists_access_set(
            "F0LIST1",
            ListAccessLevel::Owner,
            &[],
            &[UserId::from("U01")],
        )
        .await
        .unwrap();

        // Passing both channel_ids and user_ids (or neither) violates XOR client-side before hitting Slack.
        let xor_err = api
            .lists_access_set(
                "F0LIST1",
                ListAccessLevel::Read,
                &[ChannelId::from("C01")],
                &[UserId::from("U01")],
            )
            .await
            .unwrap_err();
        assert!(matches!(xor_err, ApiError::Slack(ref s) if s == "invalid_parameters"));
        let empty_err = api
            .lists_access_delete("F0LIST1", &[], &[])
            .await
            .unwrap_err();
        assert!(matches!(empty_err, ApiError::Slack(ref s) if s == "invalid_parameters"));

        let del = api
            .lists_access_delete("F0LIST1", &[], &[UserId::from("U01"), UserId::from("U02")])
            .await
            .unwrap();
        assert_eq!(del.failed_to_update_user_ids, vec![UserId::from("U02")]);
        assert!(format_list_error("list_not_found").contains("not found"));
    }

    // 2. fetch_list_with_meta extracts permissions, owners/editors, channels & item limits
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/files.info"))
            .and(body_string_contains("file=F0LISTMETA"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "file": {
                    "id": "F0LISTMETA",
                    "title": "Q4 Launch",
                    "permalink": "https://acme.slack.com/lists/T1/F0LISTMETA",
                    "editable": true,
                    "access": "owner",
                    "org_or_workspace_access": "read",
                    "editors": ["U1", "U2"],
                    "channels": ["C1"],
                    "list_limits": {
                        "row_count": 42,
                        "row_count_limit": 5000,
                        "column_count": 6,
                        "column_count_limit": 40,
                        "over_row_maximum": false
                    },
                    "list_metadata": {
                        "description": "Launch tracker",
                        "schema": [
                            { "id": "col_name", "key": "name", "name": "Task", "type": "text", "is_primary_column": true }
                        ]
                    }
                }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/lists.records.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "records": [
                    { "id": "Rec01", "fields": [{ "column_id": "col_name", "text": "Ship it" }] }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let (data, meta) = api.fetch_list_with_meta("F0LISTMETA").await.unwrap();
        assert_eq!(data.title, "Q4 Launch");
        assert_eq!(data.rows.len(), 1);
        assert_eq!(meta.file_id, "F0LISTMETA");
        assert_eq!(meta.access.as_deref(), Some("owner"));
        assert_eq!(meta.org_or_workspace_access.as_deref(), Some("read"));
        assert_eq!(meta.editors, vec![UserId::from("U1"), UserId::from("U2")]);
        assert_eq!(meta.shared_channels, vec![ChannelId::from("C1")]);
        assert_eq!(meta.row_count, Some(42));
        assert_eq!(meta.row_count_limit, Some(5000));
        assert_eq!(
            meta.comment_channel_id,
            list_comment_channel_id("F0LISTMETA")
        );
    }

    // 3. lists.getThreadsForRecord: nested threads + fallback to conversations.replies on C-channel
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/lists.getThreadsForRecord"))
            .and(body_string_contains("record_id=Rec01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "threads": [{
                    "root": { "ts": "1700.1", "user": "U1", "text": "Root comment" },
                    "latest_replies": [
                        { "ts": "1700.2", "thread_ts": "1700.1", "user": "U2", "text": "Reply 1" }
                    ]
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let msgs = api
            .lists_get_threads_for_record("F0LIST1", "Rec01")
            .await
            .unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].text, "Root comment");
        assert_eq!(msgs[1].text, "Reply 1");

        // Fallback when lists.getThreadsForRecord errors -> every conversations.replies page on C0LIST1.
        Mock::given(method("POST"))
            .and(path("/api/lists.getThreadsForRecord"))
            .and(body_string_contains("record_id=RecFallback"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": false,
                "error": "unknown_method"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/conversations.replies"))
            .and(body_string_contains("channel=C0LIST1"))
            .and(body_string_contains("ts=F0LIST1-RecFallback"))
            .and(body_string_contains("cursor=p2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": [
                    { "ts": "1701.2", "user": "U3", "text": "Fallback reply" }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/conversations.replies"))
            .and(body_string_contains("channel=C0LIST1"))
            .and(body_string_contains("ts=F0LIST1-RecFallback"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "messages": [
                    { "ts": "1701.1", "user": "U3", "text": "Fallback comment" }
                ],
                "response_metadata": { "next_cursor": "p2" }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let fb = api
            .lists_get_threads_for_record("F0LIST1", "RecFallback")
            .await
            .unwrap();
        assert_eq!(fb.len(), 2);
        assert_eq!(fb[0].text, "Fallback comment");
        assert_eq!(fb[1].text, "Fallback reply");
    }

    // 4. conversations.teamConnections parses connected and pending organizations
    {
        let (server, api) = setup().await;
        Mock::given(method("POST"))
            .and(path("/api/conversations.teamConnections"))
            .and(body_string_contains("channel=C0PARTNER"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "connections": [
                    { "id": "T0KLARNA", "name": "Klarna", "domain": "klarna", "is_verified": true }
                ],
                "pending_connections": [
                    { "id": "T0PEND", "name": "Pending Org", "domain": "pending" }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let conns = api
            .conversations_team_connections(&ChannelId::from("C0PARTNER"))
            .await
            .unwrap();
        assert_eq!(conns.connections.len(), 1);
        assert!(conns.connections[0].is_verified);
        assert_eq!(conns.connections[0].name, "Klarna");
        assert_eq!(conns.pending_connections.len(), 1);
        assert_eq!(conns.pending_connections[0].name, "Pending Org");
    }
}

#[tokio::test]
async fn saved_list_follows_cursors_until_the_limit_is_reached() {
    let (server, api) = setup().await;
    Mock::given(method("POST"))
        .and(path("/api/saved.list"))
        .and(body_string_contains("cursor=p2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "saved_items": [
                { "item_id": "C1", "ts": "3.0" },
                { "item_id": "C1", "ts": "4.0" }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/saved.list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "saved_items": [
                { "item_id": "C1", "ts": "1.0" },
                { "item_id": "C1", "ts": "2.0" }
            ],
            "response_metadata": { "next_cursor": "p2" }
        })))
        // Both reads fetch the first page; only the one allowing four items asks for `p2`.
        .expect(2)
        .mount(&server)
        .await;

    let all = api.saved_list("saved", 4).await.unwrap();
    let stamps: Vec<&str> = all.iter().map(|item| item.ts.as_str()).collect();
    assert_eq!(stamps, ["1.0", "2.0", "3.0", "4.0"]);
    let two = api.saved_list("saved", 2).await.unwrap();
    assert_eq!(two.len(), 2);
}
