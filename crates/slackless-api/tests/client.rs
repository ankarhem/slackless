//! End-to-end client tests against a local mock server. Never touches the real Slack network.

use futures::TryStreamExt;
use serde_json::json;
use slackless_api::types::{Block, ChannelId, MessageSubtype, SectionKind, Ts, UserId};
use slackless_api::{ApiError, FilesFilter, HistoryOpts, MessageContent, SlackApi, paginate};
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
